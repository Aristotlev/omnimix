import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AudioLines, Download, Info, RefreshCw, Settings, SlidersHorizontal, SquareParking, Star, TriangleAlert, X } from 'lucide-react';
import type { Update } from '@tauri-apps/plugin-updater';
import { checkForUpdate, inTauri, installUpdate } from './updates';
import type { Transport } from './api';
import { pushLevels } from './levels';
import type { Cmd, EqSettings, EqTarget, Notice, Prefs, UiDevice, UiSource, UiState } from './types';
import { CONN_LABEL, CONN_ORDER, ConnIcon, FormIcon, Logo } from './icons';
import { Chip, Meter, MiniCurve, Monogram, MuteButton, Slider, Toggle, pct } from './ui';
import { EqModal } from './EqModal';
import { isFlat } from './eq';

type Toast = Notice & { id: number };

function useThrottledSend(t: Transport) {
  const timers = useRef(new Map<string, { at: number; pending?: Cmd; timer?: number }>());
  return useCallback(
    (key: string, cmd: Cmd, final = false) => {
      const slot = timers.current.get(key) ?? { at: 0 };
      timers.current.set(key, slot);
      const now = performance.now();
      const flush = () => {
        if (slot.pending) t.send(slot.pending);
        slot.pending = undefined;
        slot.at = performance.now();
        slot.timer = undefined;
      };
      slot.pending = cmd;
      if (final || now - slot.at > 45) {
        if (slot.timer) clearTimeout(slot.timer);
        flush();
      } else if (!slot.timer) {
        slot.timer = window.setTimeout(flush, 45 - (now - slot.at));
      }
    },
    [t],
  );
}

function hexToRgb(h: string): [number, number, number] | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(h);
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function fmtFormat(d: UiDevice) {
  if (!d.format) return [];
  const out = [`${(d.format.sampleRate / 1000).toFixed(d.format.sampleRate % 1000 ? 1 : 0)} kHz`];
  if (d.format.bits) out.push(`${d.format.bits}-bit`);
  out.push(d.channels.length > 2 ? `${d.channels.length} ch` : d.channels.length === 1 ? 'Mono' : 'Stereo');
  return out;
}

function stateLabel(d: UiDevice) {
  return d.state === 'unplugged' ? 'Unplugged' : d.state === 'disabled' ? 'Disabled' : d.state === 'not_present' ? 'Not present' : '';
}

function DeviceRow({ d, selected, apps, onSelect, onMute }: { d: UiDevice; selected: boolean; apps: number; onSelect: () => void; onMute: (m: boolean) => void }) {
  const active = d.state === 'active';
  return (
    <div className={`dev-row ${selected ? 'sel' : ''} ${active ? '' : 'inactive'}`} onClick={onSelect} role="button" tabIndex={0} onKeyDown={(e) => e.key === 'Enter' && onSelect()}>
      <span className="dev-icon"><ConnIcon kind={d.connection.kind} size={19} /></span>
      <span className="dev-text">
        <span className="dev-name">
          {d.nickname ?? d.label}
          {d.isDefault && <Star size={12} className="star" fill="currentColor" aria-label="Default output" />}
        </span>
        <span className="dev-sub">{d.adapter || d.name}</span>
        <span className="dev-tags">
          {!active && <span className="tag">{stateLabel(d)}</span>}
          {active && d.isSink && <span className="tag ember">Parking</span>}
          {active && d.eq.enabled && <span className="tag accent">EQ</span>}
          {active && apps > 0 && <span className="tag">{apps} app{apps > 1 ? 's' : ''}</span>}
        </span>
        {active && <Meter levelKey={`d:${d.id}`} channels={1} thin dim={d.muted} />}
      </span>
      {active && (
        <span onClick={(e) => e.stopPropagation()}>
          <MuteButton muted={d.muted} onChange={onMute} size={17} label={d.label} disabled={d.isSink} />
        </span>
      )}
    </div>
  );
}

function SourceCard({ s, icon, onVolume, onMute, onChMute, onEq, onEqToggle }: {
  s: UiSource; icon?: string; onVolume: (v: number, final?: boolean) => void; onMute: (m: boolean) => void; onChMute: (ch: number, m: boolean) => void; onEq: () => void; onEqToggle: (on: boolean) => void;
}) {
  const statusChip =
    s.system ? <Chip>System</Chip>
    : s.status === 'processing' ? <Chip tone="accent">Processing</Chip>
    : s.status === 'starting' ? <Chip>Starting…</Chip>
    : s.status === 'blocked' ? <Chip tone="ember" title={s.note ?? ''}>Can't process</Chip>
    : s.active ? <Chip>Playing</Chip> : <Chip>Idle</Chip>;
  return (
    <div className={`card src ${s.muted ? 'muted' : ''}`}>
      <div className="src-head">
        <Monogram name={s.name} src={icon} />
        <div className="src-title">
          <div className="src-name" title={s.exe ?? s.name}>{s.name}</div>
          <div className="src-meta">{statusChip}</div>
        </div>
        <MuteButton muted={s.muted} onChange={onMute} label={s.name} />
      </div>
      <Meter levelKey={`s:${s.id}`} channels={Math.max(1, Math.min(2, s.channels.length || 2))} dim={s.muted} />
      <div className="src-vol">
        <Slider value={s.volume} onChange={(v) => onVolume(v)} label={`${s.name} volume`} />
        <span className="val">{pct(s.volume)}</span>
      </div>
      <div className="src-foot">
        <div className="ch-chips">
          {s.channels.slice(0, 8).map((c) => (
            <button key={c.index} className={`ch-chip ${c.muted ? 'off' : ''}`} title={c.muted ? `Unmute ${c.label}` : `Mute ${c.label}`} aria-pressed={c.muted} onClick={() => onChMute(c.index, !c.muted)}>
              {c.label}
            </button>
          ))}
        </div>
        {!s.system && (
          <div className="eq-mini">
            <button className="mini-curve-btn" onClick={onEq} title="Open EQ" aria-label={`Open ${s.name} EQ`}>
              <MiniCurve eq={s.eq} width={86} height={28} />
            </button>
            <Toggle on={s.eq.enabled} label={`${s.name} EQ`} onChange={onEqToggle} />
          </div>
        )}
      </div>
      {s.status === 'blocked' && s.note && <div className="src-note"><Info size={13} /> {s.note}</div>}
    </div>
  );
}

function DevicePanel({ d, st, send, openEq }: { d: UiDevice; st: UiState; send: (k: string, c: Cmd, f?: boolean) => void; openEq: (t: EqTarget, eq: EqSettings, level: string) => void }) {
  const apps = st.sources.filter((s) => s.deviceId === d.id);
  const active = d.state === 'active';
  const chips = [...d.connection.detail, ...fmtFormat(d)];
  const jack = d.connection.jackColor != null ? `#${d.connection.jackColor.toString(16).padStart(6, '0')}` : null;
  const devEq = () => openEq({ kind: 'device', id: d.id, title: d.nickname ?? d.label, subtitle: d.adapter }, d.eq, `d:${d.id}`);
  return (
    <section className="panel">
      <div className="card dev-head">
        <div className="big-icon"><FormIcon form={d.formFactor} size={30} /></div>
        <div className="dev-head-text">
          <div className="eyebrow">{CONN_LABEL[d.connection.kind]} output</div>
          <h1>{d.nickname ?? d.label}<span className="sub">{d.adapter}</span></h1>
          <div className="chip-row">
            <Chip tone="solid"><ConnIcon kind={d.connection.kind} size={14} /> {CONN_LABEL[d.connection.kind]}</Chip>
            {chips.map((c) => <Chip key={c}>{c}</Chip>)}
            {jack && <Chip><span className="jack-dot" style={{ background: jack }} /> Jack color</Chip>}
            {d.connection.usb && <Chip title="USB vendor:product">{d.connection.usb.vid.toString(16).padStart(4, '0')}:{d.connection.usb.pid.toString(16).padStart(4, '0')}</Chip>}
            {d.isSink && <Chip tone="ember" title="OmniMix parks apps here while it processes them. Kept muted on purpose."><SquareParking size={13} /> Parking output</Chip>}
          </div>
        </div>
        <div className="dev-head-actions">
          {d.isDefault ? (
            <Chip tone="accent"><Star size={12} fill="currentColor" /> Default output</Chip>
          ) : (
            <button className="btn" disabled={!active} onClick={() => send('default', { type: 'setDefaultDevice', id: d.id }, true)}>Make default</button>
          )}
        </div>
      </div>

      {!active ? (
        <div className="card empty">This output is {stateLabel(d).toLowerCase()}. Connect or enable it in Windows to control it.</div>
      ) : (
        <>
          <div className="row-2">
            <div className="card master">
              <div className="card-title">Output volume</div>
              <div className="master-row">
                <MuteButton muted={d.muted} onChange={(m) => send(`dm:${d.id}`, { type: 'setDeviceMute', id: d.id, muted: m }, true)} size={20} label={d.label} disabled={d.isSink} />
                <Slider value={d.volume} onChange={(v) => send(`dv:${d.id}`, { type: 'setDeviceVolume', id: d.id, volume: v })} label="Output volume" />
                <span className="val big">{pct(d.volume)}</span>
              </div>
              <Meter levelKey={`d:${d.id}`} channels={Math.min(2, Math.max(1, d.channels.length))} dim={d.muted} />
            </div>
            <div className="card dev-eq">
              <div className="card-title">
                Device EQ
                <Toggle on={d.eq.enabled} label="Device EQ" disabled={d.isSink || !st.dsp.available} onChange={(v) => send(`deq:${d.id}`, { type: 'setDeviceEq', id: d.id, eq: { ...d.eq, enabled: v } }, true)} />
              </div>
              <button className="dev-eq-body" onClick={devEq} disabled={d.isSink || !st.dsp.available}>
                <MiniCurve eq={d.eq} width={260} height={54} />
                <span className="dev-eq-meta">
                  <span>{d.eq.enabled ? d.eq.preset ?? 'Custom' : 'Off'}</span>
                  <span className="open"><SlidersHorizontal size={14} /> Edit</span>
                </span>
              </button>
            </div>
          </div>

          {d.channels.length > 0 && (
            <div className="card">
              <div className="card-title">Channels <span className="muted-text">mute or trim each speaker</span></div>
              <div className="channels">
                {d.channels.map((c) => (
                  <div key={c.index} className={`chan ${c.muted ? 'off' : ''}`}>
                    <div className="chan-top">
                      <span className="chan-label">{c.label}</span>
                      <MuteButton muted={c.muted} onChange={(m) => send(`dcm:${d.id}:${c.index}`, { type: 'setDeviceChannelMute', id: d.id, channel: c.index, muted: m }, true)} size={15} label={`channel ${c.label}`} />
                    </div>
                    <Slider value={c.volume} onChange={(v) => send(`dc:${d.id}:${c.index}`, { type: 'setDeviceChannelVolume', id: d.id, channel: c.index, volume: v })} label={`${c.label} level`} />
                    <span className="val">{pct(c.volume)}</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          <div className="section-title">
            <AudioLines size={16} /> Apps on this output <span className="count">{apps.length}</span>
          </div>
          {apps.length === 0 ? (
            <div className="card empty">Nothing is playing here. Apps appear the moment they open an audio stream.</div>
          ) : (
            <div className="src-grid">
              {apps.map((s) => (
                <SourceCard
                  key={s.id}
                  s={s}
                  icon={st.icons[s.key]}
                  onVolume={(v) => send(`sv:${s.id}`, { type: 'setSourceVolume', group: s.id, volume: v })}
                  onMute={(m) => send(`sm:${s.id}`, { type: 'setSourceMute', group: s.id, muted: m }, true)}
                  onChMute={(ch, m) => send(`scm:${s.id}:${ch}`, { type: 'setSourceChannelMute', group: s.id, channel: ch, muted: m }, true)}
                  onEq={() => openEq({ kind: 'source', key: s.key, title: s.name, subtitle: `on ${d.nickname ?? d.label}` }, s.eq, `s:${s.id}`)}
                  onEqToggle={(on) => {
                    if (on && isFlat(s.eq)) openEq({ kind: 'source', key: s.key, title: s.name, subtitle: `on ${d.nickname ?? d.label}` }, { ...s.eq, enabled: true }, `s:${s.id}`);
                    send(`seq:${s.key}`, { type: 'setSourceEq', key: s.key, eq: { ...s.eq, enabled: on } }, true);
                  }}
                />
              ))}
            </div>
          )}
        </>
      )}
    </section>
  );
}

function SettingsModal({ st, onClose, onPrefs, updateText, onCheckUpdate }: { st: UiState; onClose: () => void; onPrefs: (p: Prefs) => void; updateText: string; onCheckUpdate: () => void }) {
  const p = st.prefs;
  const devName = (id: string) => {
    const d = st.devices.find((x) => x.id === id);
    return d ? `${d.label} (${d.adapter || CONN_LABEL[d.connection.kind]})` : id;
  };
  return (
    <div className="modal-scrim" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal settings" role="dialog" aria-label="Settings">
        <header className="modal-head">
          <h2>Settings</h2>
          <button className="icon-btn" aria-label="Close" onClick={onClose}><X size={18} /></button>
        </header>
        <div className="set-list">
          {st.dsp.needsSink && (
            <div className="set-row">
              <div>
                <div className="set-name">Parking output</div>
                <div className="set-desc">Processed apps are parked on this output (muted while EQ runs) so you only hear the EQ'd version. Pick one you don't listen to: HDMI, unused speakers, or a virtual cable.</div>
              </div>
              <select className="select" value={p.sink ?? ''} onChange={(e) => onPrefs({ ...p, sink: e.target.value || null })}>
                <option value="">Automatic</option>
                {st.dsp.sinkCandidates.map((id) => <option key={id} value={id}>{devName(id)}</option>)}
              </select>
            </div>
          )}
          <div className="set-row">
            <div>
              <div className="set-name">Latency</div>
              <div className="set-desc">Buffer for processed audio. Low suits games, Safe suits weak CPUs and Bluetooth.</div>
            </div>
            <div className="seg">
              {(['low', 'balanced', 'safe'] as const).map((m) => (
                <button key={m} className={p.latency === m ? 'on' : ''} onClick={() => onPrefs({ ...p, latency: m })}>{m[0].toUpperCase() + m.slice(1)}</button>
              ))}
            </div>
          </div>
          <div className="set-row">
            <div>
              <div className="set-name">Show unplugged and disabled outputs</div>
            </div>
            <Toggle on={p.showInactive} label="Show inactive outputs" onChange={(v) => onPrefs({ ...p, showInactive: v })} />
          </div>
          <div className="set-row">
            <div>
              <div className="set-name">Updates</div>
              <div className="set-desc">OmniMix {st.version}. {updateText}</div>
            </div>
            <button className="btn" disabled={!inTauri} onClick={onCheckUpdate}><RefreshCw size={14} /> Check now</button>
          </div>
          <div className="set-row">
            <div>
              <div className="set-name">Keep running in the tray when closed</div>
              <div className="set-desc">EQ keeps working with the window closed.</div>
            </div>
            <Toggle on={p.closeToTray} label="Close to tray" onChange={(v) => onPrefs({ ...p, closeToTray: v })} />
          </div>
        </div>
        <div className="about">
          <div><span>OmniMix</span> {st.version}</div>
          <div><span>System</span> {st.platform.os}</div>
          <div><span>Audio</span> {st.platform.backend}</div>
          <div><span>EQ engine</span> {st.dsp.method}</div>
          <div><span>Engine latency</span> ~{Math.round(st.dsp.latencyMs)} ms on processed apps</div>
        </div>
      </div>
    </div>
  );
}

export default function App({ t }: { t: Transport }) {
  const [st, setSt] = useState<UiState | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const [eqOpen, setEqOpen] = useState<{ target: EqTarget; eq: EqSettings; level: string } | null>(null);
  const [settings, setSettings] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [update, setUpdate] = useState<Update | null>(null);
  const [updatePhase, setUpdatePhase] = useState<'idle' | 'checking' | 'none' | 'downloading' | 'installing' | 'error'>('idle');
  const [updatePct, setUpdatePct] = useState<number | null>(null);
  const icons = useRef<Record<string, string>>({});
  const send = useThrottledSend(t);

  useEffect(() => {
    const apply = (s: UiState) => {
      icons.current = { ...icons.current, ...s.icons };
      setSt({ ...s, icons: icons.current });
    };
    t.getState().then(apply).catch((e) => setToasts((x) => [...x, { id: Date.now(), level: 'error', text: String(e) }]));
    t.onState(apply);
    t.onLevels(pushLevels);
    t.onNotice((n) => {
      const id = Date.now() + Math.random();
      setToasts((x) => [...x.slice(-3), { ...n, id }]);
      setTimeout(() => setToasts((x) => x.filter((y) => y.id !== id)), Math.max(4500, n.text.length * 70));
    });
  }, [t]);

  const runCheck = useCallback(async (manual: boolean) => {
    if (!inTauri) return;
    setUpdatePhase('checking');
    try {
      const u = await checkForUpdate();
      setUpdate(u);
      setUpdatePhase(u ? 'idle' : 'none');
    } catch (e) {
      setUpdatePhase(manual ? 'error' : 'idle');
      if (manual) console.warn('update check failed', e);
    }
  }, []);

  useEffect(() => {
    const first = setTimeout(() => runCheck(false), 4000);
    const every = setInterval(() => runCheck(false), 6 * 3600 * 1000);
    return () => {
      clearTimeout(first);
      clearInterval(every);
    };
  }, [runCheck]);

  const startUpdate = async () => {
    if (!update) return;
    setUpdatePhase('downloading');
    try {
      await installUpdate(update, (f) => {
        setUpdatePct(f);
        if (f !== null && f >= 1) setUpdatePhase('installing');
      });
    } catch (e) {
      setUpdatePhase('error');
      setToasts((x) => [...x, { id: Date.now(), level: 'error', text: `Update failed: ${String(e)}. Restart OmniMix to keep using it.` }]);
    }
  };

  // Native accent -> CSS custom properties (the "1 native" color).
  useEffect(() => {
    const a = st?.platform.accentDarkUi ?? st?.platform.accent;
    const rgb = a ? hexToRgb(a) : null;
    if (rgb) {
      document.documentElement.style.setProperty('--accent', a!);
      document.documentElement.style.setProperty('--accent-rgb', rgb.join(','));
    }
  }, [st?.platform.accent, st?.platform.accentDarkUi]);

  const groups = useMemo(() => {
    if (!st) return [];
    const active = st.devices.filter((d) => d.state === 'active');
    const inactive = st.devices.filter((d) => d.state !== 'active');
    const out: { title: string; items: UiDevice[] }[] = [];
    for (const k of CONN_ORDER) {
      const items = active.filter((d) => d.connection.kind === k).sort((a, b) => Number(b.isDefault) - Number(a.isDefault));
      if (items.length) out.push({ title: CONN_LABEL[k], items });
    }
    if (inactive.length) out.push({ title: 'Unplugged / disabled', items: inactive });
    return out;
  }, [st]);

  if (!st) {
    return (
      <div className="boot">
        <Logo size={44} />
        <div>Starting audio engine…</div>
      </div>
    );
  }

  const selected = st.devices.find((d) => d.id === sel) ?? st.devices.find((d) => d.isDefault) ?? st.devices[0];
  const openEq = (target: EqTarget, eq: EqSettings, level: string) => setEqOpen({ target, eq, level });
  const banner = !st.dsp.available ? st.dsp.reason : st.dsp.note;

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand"><Logo /> <span>OmniMix</span></div>
        <div className={`engine ${st.dsp.available ? '' : 'off'}`} title={st.dsp.method}>
          <span className="dot" />
          {st.dsp.available ? (
            st.dsp.streams > 0 ? <>EQ engine · {st.dsp.streams} stream{st.dsp.streams > 1 ? 's' : ''} · ~{Math.round(st.dsp.latencyMs)} ms</> : <>EQ engine ready</>
          ) : (
            <>EQ unavailable</>
          )}
        </div>
        <div className="top-actions">
          {t.demo && <Chip tone="ember" title="Browser preview with simulated devices">Demo</Chip>}
          <button className="icon-btn" aria-label="Refresh" title="Rescan devices" onClick={() => t.send({ type: 'refresh' })}><RefreshCw size={17} /></button>
          <button className="icon-btn" aria-label="Settings" title="Settings" onClick={() => setSettings(true)}><Settings size={18} /></button>
        </div>
      </header>
      {update && (
        <div className="banner update">
          <Download size={15} />
          {updatePhase === 'downloading' || updatePhase === 'installing' ? (
            <span>
              {updatePhase === 'installing' ? 'Installing' : 'Downloading'} OmniMix {update.version}
              {updatePct !== null && updatePhase === 'downloading' ? ` · ${Math.round(updatePct * 100)}%` : ''}…
            </span>
          ) : (
            <>
              <span>OmniMix {update.version} is available{update.body ? `: ${update.body}` : ''}</span>
              <button className="btn" onClick={startUpdate}>Install &amp; restart</button>
            </>
          )}
        </div>
      )}
      {banner && (
        <div className="banner"><TriangleAlert size={15} /> {banner}</div>
      )}
      <div className="body">
        <aside className="sidebar">
          <div className="side-title">Outputs <span className="count">{st.devices.filter((d) => d.state === 'active').length}</span></div>
          {groups.map((g) => (
            <div key={g.title} className="dev-group">
              <div className="group-title">{g.title}</div>
              {g.items.map((d) => (
                <DeviceRow
                  key={d.id}
                  d={d}
                  selected={selected?.id === d.id}
                  apps={st.sources.filter((s) => s.deviceId === d.id).length}
                  onSelect={() => setSel(d.id)}
                  onMute={(m) => send(`dm:${d.id}`, { type: 'setDeviceMute', id: d.id, muted: m }, true)}
                />
              ))}
            </div>
          ))}
        </aside>
        <main className="main">{selected ? <DevicePanel d={selected} st={st} send={send} openEq={openEq} /> : <div className="card empty">No playback devices found.</div>}</main>
      </div>

      {eqOpen && (
        <EqModal
          target={eqOpen.target}
          eq={eqOpen.eq}
          presets={st.presets}
          levelKey={eqOpen.level}
          onClose={() => setEqOpen(null)}
          onChange={(eq, final) => {
            const tg = eqOpen.target;
            if (tg.kind === 'device') send(`deq:${tg.id}`, { type: 'setDeviceEq', id: tg.id, eq }, final);
            else send(`seq:${tg.key}`, { type: 'setSourceEq', key: tg.key, eq }, final);
          }}
          onSavePreset={(name, eq) => t.send({ type: 'savePreset', name, eq })}
          onDeletePreset={(name) => t.send({ type: 'deletePreset', name })}
        />
      )}
      {settings && (
        <SettingsModal
          st={st}
          onClose={() => setSettings(false)}
          onPrefs={(p) => t.send({ type: 'setPrefs', prefs: p })}
          onCheckUpdate={() => runCheck(true)}
          updateText={
            !inTauri ? 'Updates run in the desktop app.'
            : updatePhase === 'checking' ? 'Checking…'
            : update ? `Version ${update.version} is ready to install.`
            : updatePhase === 'none' ? 'You have the latest version.'
            : updatePhase === 'error' ? 'Could not reach the update server.'
            : 'Checks automatically on launch.'
          }
        />
      )}
      <div className="toasts" aria-live="polite">
        {toasts.map((n) => <div key={n.id} className={`toast ${n.level}`}>{n.text}</div>)}
      </div>
    </div>
  );
}
