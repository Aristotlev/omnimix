// Transport to the core mixer. Inside the desktop app it is Tauri IPC.
// In a plain browser (npm run dev) a demo engine mirrors the Rust mock rig.
import type { Cmd, Levels, Notice, UiState } from './types';
import demoState from './demo-state.json';

export interface Transport {
  demo: boolean;
  getState(): Promise<UiState>;
  send(cmd: Cmd): void;
  onState(cb: (s: UiState) => void): void;
  onLevels(cb: (l: Levels) => void): void;
  onNotice(cb: (n: Notice) => void): void;
}

const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export async function connect(): Promise<Transport> {
  return inTauri ? tauriTransport() : demoTransport();
}

async function tauriTransport(): Promise<Transport> {
  const { invoke } = await import('@tauri-apps/api/core');
  const { listen } = await import('@tauri-apps/api/event');
  return {
    demo: false,
    getState: () => invoke<UiState>('get_state'),
    send: (cmd) => {
      invoke('command', { cmd }).catch((e) => console.error('command failed', e));
    },
    onState: (cb) => void listen<UiState>('omx://state', (e) => cb(e.payload)),
    onLevels: (cb) => void listen<Levels>('omx://levels', (e) => cb(e.payload)),
    onNotice: (cb) => void listen<Notice>('omx://notice', (e) => cb(e.payload)),
  };
}

function demoTransport(): Transport {
  const st: UiState = JSON.parse(JSON.stringify(demoState));
  st.icons = {};
  const stateCbs: ((s: UiState) => void)[] = [];
  const levelCbs: ((l: Levels) => void)[] = [];
  const noticeCbs: ((n: Notice) => void)[] = [];
  const emit = () => {
    const copy: UiState = JSON.parse(JSON.stringify(st));
    stateCbs.forEach((cb) => cb(copy));
  };
  const reconcile = () => {
    let streams = 0;
    for (const s of st.sources) {
      const dev = st.devices.find((d) => d.id === s.deviceId);
      const want = !s.system && (s.eq.enabled || !!dev?.eq.enabled);
      s.status = want ? 'processing' : 'direct';
      if (want) streams++;
    }
    for (const d of st.devices) d.processing = d.eq.enabled || st.sources.some((s) => s.deviceId === d.id && s.status === 'processing');
    st.dsp.streams = streams;
  };
  const dev = (id: string) => st.devices.find((d) => d.id === id);
  const src = (id: string) => st.sources.find((s) => s.id === id);
  const t0 = performance.now();
  setInterval(() => {
    const t = (performance.now() - t0) / 1000;
    const s: [string, number[]][] = st.sources.map((x, i) => {
      const beat = Math.exp(-((t * 2.1 + i * 0.37) % 1) * 6);
      const wob = 0.5 + 0.5 * Math.sin(t * 1.3 + i) * Math.cos(t * 0.37 + i * 2);
      const base = x.active && !x.muted ? Math.min(1, (i % 2 ? 0.35 * wob + 0.1 : 0.5 + 0.35 * beat) * x.volume) : 0;
      const boost = x.status === 'processing' ? 1.15 : 1;
      return [x.id, x.channels.map((c, ci) => base * c.volume * boost * (ci ? 0.92 + 0.08 * Math.sin(t * 3) : 1))];
    });
    const d: [string, number[]][] = st.devices
      .filter((x) => x.state === 'active')
      .map((x) => {
        const lvl = x.muted ? 0 : Math.max(0, ...s.filter(([id]) => src(id)?.deviceId === x.id).map(([, p]) => Math.max(...p))) * Math.sqrt(x.volume);
        return [x.id, x.channels.map((c, i) => lvl * c.volume * (0.85 + 0.15 * Math.sin(t * 2 + i)))];
      });
    levelCbs.forEach((cb) => cb({ d, s }));
  }, 33);
  const notice = (n: Notice) => noticeCbs.forEach((cb) => cb(n));
  return {
    demo: true,
    getState: async () => {
      reconcile();
      return JSON.parse(JSON.stringify(st));
    },
    send: (c) => {
      switch (c.type) {
        case 'setDeviceVolume': { const d = dev(c.id); if (d) d.volume = c.volume; break; }
        case 'setDeviceMute': { const d = dev(c.id); if (d) d.muted = c.muted; break; }
        case 'setDeviceChannelVolume': { const ch = dev(c.id)?.channels[c.channel]; if (ch) { ch.volume = c.volume; ch.muted = c.volume <= 0.0001; } break; }
        case 'setDeviceChannelMute': { const ch = dev(c.id)?.channels[c.channel]; if (ch) { ch.muted = c.muted; ch.volume = c.muted ? 0 : 1; } break; }
        case 'setDefaultDevice': st.devices.forEach((d) => (d.isDefault = d.id === c.id)); break;
        case 'setSourceVolume': { const s = src(c.group); if (s) s.volume = c.volume; break; }
        case 'setSourceMute': { const s = src(c.group); if (s) s.muted = c.muted; break; }
        case 'setSourceChannelMute': { const ch = src(c.group)?.channels[c.channel]; if (ch) { ch.muted = c.muted; ch.volume = c.muted ? 0 : 1; } break; }
        case 'setDeviceEq': { const d = dev(c.id); if (d) d.eq = c.eq; break; }
        case 'setSourceEq': st.sources.filter((s) => s.key === c.key).forEach((s) => (s.eq = c.eq)); break;
        case 'savePreset':
          st.presets = st.presets.filter((p) => p.name !== c.name);
          st.presets.push({ name: c.name, builtin: false, eq: { ...c.eq, preset: c.name } });
          notice({ level: 'info', text: `Saved preset “${c.name}”` });
          break;
        case 'deletePreset': st.presets = st.presets.filter((p) => p.name !== c.name); break;
        case 'setPrefs': st.prefs = c.prefs; break;
        case 'refresh': break;
      }
      reconcile();
      emit();
    },
    onState: (cb) => stateCbs.push(cb),
    onLevels: (cb) => levelCbs.push(cb),
    onNotice: (cb) => noticeCbs.push(cb),
  };
}
