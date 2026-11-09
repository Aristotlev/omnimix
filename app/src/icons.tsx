import {
  Bluetooth, Cable, Headphones, Headset, Laptop, Monitor, Speaker, Usb, Waypoints, Wifi, Zap,
} from 'lucide-react';
import type { ConnectionKind, FormFactor } from './types';

type P = { size?: number; strokeWidth?: number; className?: string };

/** USB Type-C receptacle: a stadium with the tongue inside. */
export function UsbCIcon({ size = 20, strokeWidth = 1.75, className }: P) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={strokeWidth} strokeLinecap="round" strokeLinejoin="round" className={className}>
      <rect x="2.5" y="8" width="19" height="8" rx="4" />
      <path d="M7.5 12h9" />
    </svg>
  );
}

/** 3.5 mm TRS plug. */
export function JackIcon({ size = 20, strokeWidth = 1.75, className }: P) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={strokeWidth} strokeLinecap="round" strokeLinejoin="round" className={className}>
      <path d="M12 2.5v6" />
      <path d="M10.6 5.2h2.8M10.6 7.4h2.8" />
      <rect x="9.25" y="8.5" width="5.5" height="7" rx="1.2" />
      <path d="M12 15.5v3.5a2.5 2.5 0 0 1-2.5 2.5H8" />
    </svg>
  );
}

/** Optical (TOSLINK) square connector. */
export function OpticalIcon({ size = 20, strokeWidth = 1.75, className }: P) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={strokeWidth} strokeLinecap="round" strokeLinejoin="round" className={className}>
      <path d="M6 5h12l2 3v11H4V8z" />
      <circle cx="12" cy="13" r="2.5" />
    </svg>
  );
}

export function ConnIcon({ kind, ...p }: P & { kind: ConnectionKind }) {
  switch (kind) {
    case 'usb_c': return <UsbCIcon {...p} />;
    case 'usb': return <Usb size={p.size ?? 20} strokeWidth={p.strokeWidth ?? 1.75} className={p.className} />;
    case 'bluetooth': return <Bluetooth size={p.size ?? 20} strokeWidth={p.strokeWidth ?? 1.75} className={p.className} />;
    case 'aux': return <JackIcon {...p} />;
    case 'display': return <Monitor size={p.size ?? 20} strokeWidth={p.strokeWidth ?? 1.75} className={p.className} />;
    case 'thunderbolt': return <Zap size={p.size ?? 20} strokeWidth={p.strokeWidth ?? 1.75} className={p.className} />;
    case 'spdif': return <OpticalIcon {...p} />;
    case 'built_in': return <Laptop size={p.size ?? 20} strokeWidth={p.strokeWidth ?? 1.75} className={p.className} />;
    case 'virtual': return <Waypoints size={p.size ?? 20} strokeWidth={p.strokeWidth ?? 1.75} className={p.className} />;
    case 'network': return <Wifi size={p.size ?? 20} strokeWidth={p.strokeWidth ?? 1.75} className={p.className} />;
    default: return <Cable size={p.size ?? 20} strokeWidth={p.strokeWidth ?? 1.75} className={p.className} />;
  }
}

export function FormIcon({ form, ...p }: P & { form: FormFactor }) {
  const s = p.size ?? 22, w = p.strokeWidth ?? 1.6;
  switch (form) {
    case 'headphones': return <Headphones size={s} strokeWidth={w} className={p.className} />;
    case 'headset': return <Headset size={s} strokeWidth={w} className={p.className} />;
    case 'display': return <Monitor size={s} strokeWidth={w} className={p.className} />;
    default: return <Speaker size={s} strokeWidth={w} className={p.className} />;
  }
}

export const CONN_LABEL: Record<ConnectionKind, string> = {
  usb_c: 'USB-C',
  usb: 'USB',
  bluetooth: 'Bluetooth',
  aux: 'AUX',
  display: 'HDMI / DP',
  thunderbolt: 'Thunderbolt',
  spdif: 'S/PDIF',
  built_in: 'Built-in',
  virtual: 'Virtual',
  network: 'Network',
  unknown: 'Other',
};

export const CONN_ORDER: ConnectionKind[] = ['usb_c', 'usb', 'bluetooth', 'aux', 'display', 'thunderbolt', 'spdif', 'built_in', 'virtual', 'network', 'unknown'];

/** Brand mark: five EQ bars, the center one in Ember. */
export function Logo({ size = 26 }: { size?: number }) {
  const h = [0.36, 0.62, 0.86, 0.54, 0.3];
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-label="OmniMix">
      <rect x="1" y="1" width="30" height="30" rx="8" fill="var(--ink-3)" />
      {h.map((v, i) => {
        const bh = v * 20;
        return <rect key={i} x={6 + i * 4.4} y={16 - bh / 2} width="2.6" height={bh} rx="1.3" fill={i === 2 ? 'var(--ember)' : 'var(--bone)'} />;
      })}
    </svg>
  );
}
