// Mirrors omnimix-core's serialized UI contract (mixer::UiState).

export type ConnectionKind =
  | 'usb_c' | 'usb' | 'bluetooth' | 'aux' | 'display' | 'thunderbolt' | 'spdif' | 'built_in' | 'virtual' | 'network' | 'unknown';
export type FormFactor =
  | 'speakers' | 'headphones' | 'headset' | 'line_out' | 'spdif' | 'display' | 'network' | 'handset' | 'unknown';
export type DeviceState = 'active' | 'unplugged' | 'disabled' | 'not_present';
export type FilterKind = 'peak' | 'low_shelf' | 'high_shelf' | 'low_cut' | 'high_cut' | 'notch';
export type SourceStatus = 'direct' | 'starting' | 'processing' | 'blocked';
export type LatencyMode = 'low' | 'balanced' | 'safe';

export interface Channel { index: number; label: string; volume: number; muted: boolean }

export interface Band { label: string; kind: FilterKind; freq: number; gainDb: number; q: number; enabled: boolean }

export interface EqSettings {
  enabled: boolean;
  preset: string | null;
  preampDb: number;
  autoPreamp: boolean;
  width: number;
  bands: Band[];
}

export interface ConnectionInfo {
  kind: ConnectionKind;
  detail: string[];
  bus: string | null;
  usb: { vid: number; pid: number } | null;
  jackColor: number | null;
}

export interface UiDevice {
  id: string;
  name: string;
  label: string;
  adapter: string;
  formFactor: FormFactor;
  connection: ConnectionInfo;
  state: DeviceState;
  isDefault: boolean;
  isDefaultComms: boolean;
  format: { sampleRate: number; channels: number; bits: number; channelMask: number } | null;
  volume: number;
  muted: boolean;
  channels: Channel[];
  eq: EqSettings;
  isSink: boolean;
  processing: boolean;
  nickname: string | null;
}

export interface UiSource {
  id: string;
  key: string;
  name: string;
  exe: string | null;
  deviceId: string;
  physicalDeviceId: string;
  pids: number[];
  sessionIds: string[];
  active: boolean;
  system: boolean;
  volume: number;
  muted: boolean;
  channels: Channel[];
  eq: EqSettings;
  status: SourceStatus;
  note: string | null;
}

export interface Preset { name: string; builtin: boolean; eq: EqSettings }

export interface Prefs { sink: string | null; latency: LatencyMode; showInactive: boolean; closeToTray: boolean }

export interface UiState {
  version: string;
  platform: { os: string; backend: string; accent: string | null; accentDarkUi: string | null; darkMode: boolean };
  dsp: {
    available: boolean;
    reason: string | null;
    method: string;
    needsSink: boolean;
    sink: string | null;
    sinkCandidates: string[];
    latencyMs: number;
    streams: number;
    note: string | null;
  };
  devices: UiDevice[];
  sources: UiSource[];
  presets: Preset[];
  prefs: Prefs;
  icons: Record<string, string>;
}

export interface Levels { d: [string, number[]][]; s: [string, number[]][] }
export interface Notice { level: 'info' | 'warn' | 'error'; text: string }

export type Cmd =
  | { type: 'setDeviceVolume'; id: string; volume: number }
  | { type: 'setDeviceMute'; id: string; muted: boolean }
  | { type: 'setDeviceChannelVolume'; id: string; channel: number; volume: number }
  | { type: 'setDeviceChannelMute'; id: string; channel: number; muted: boolean }
  | { type: 'setDefaultDevice'; id: string }
  | { type: 'setSourceVolume'; group: string; volume: number }
  | { type: 'setSourceMute'; group: string; muted: boolean }
  | { type: 'setSourceChannelMute'; group: string; channel: number; muted: boolean }
  | { type: 'setDeviceEq'; id: string; eq: EqSettings }
  | { type: 'setSourceEq'; key: string; eq: EqSettings }
  | { type: 'savePreset'; name: string; eq: EqSettings }
  | { type: 'deletePreset'; name: string }
  | { type: 'setPrefs'; prefs: Prefs }
  | { type: 'refresh' };

export type EqTarget = { kind: 'device'; id: string; title: string; subtitle: string } | { kind: 'source'; key: string; title: string; subtitle: string };
