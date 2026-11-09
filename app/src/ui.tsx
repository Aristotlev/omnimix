import { useEffect, useRef, type CSSProperties, type ReactNode } from 'react';
import { Volume2, VolumeX } from 'lucide-react';
import { norm, subscribe } from './levels';
import { curve, logFreqs, G_MAX } from './eq';
import type { EqSettings } from './types';

export function Meter({ levelKey, channels = 2, vertical = false, thin = false, dim = false }: { levelKey: string; channels?: number; vertical?: boolean; thin?: boolean; dim?: boolean }) {
  const masks = useRef<(HTMLDivElement | null)[]>([]);
  const vals = useRef<number[]>([]);
  useEffect(() => {
    return subscribe(levelKey, (peaks) => {
      for (let i = 0; i < channels; i++) {
        const target = norm(peaks[i] ?? peaks[0] ?? 0);
        const prev = vals.current[i] ?? 0;
        const v = target >= prev ? target : Math.max(target, prev - 0.045);
        vals.current[i] = v;
        const el = masks.current[i];
        if (el) el.style[vertical ? 'height' : 'width'] = `${(1 - v) * 100}%`;
      }
    });
  }, [levelKey, channels, vertical]);
  return (
    <div className={`meter ${vertical ? 'v' : 'h'} ${thin ? 'thin' : ''} ${dim ? 'dim' : ''}`}>
      {Array.from({ length: channels }, (_, i) => (
        <div className="meter-bar" key={i}>
          <div className="meter-mask" ref={(el) => { masks.current[i] = el; }} />
        </div>
      ))}
    </div>
  );
}

export function Slider({ value, onChange, min = 0, max = 1, step = 0.01, label, disabled, className }: { value: number; onChange: (v: number) => void; min?: number; max?: number; step?: number; label?: string; disabled?: boolean; className?: string }) {
  const pct = ((value - min) / (max - min)) * 100;
  return (
    <input
      type="range"
      className={`slider ${className ?? ''}`}
      aria-label={label}
      min={min}
      max={max}
      step={step}
      value={value}
      disabled={disabled}
      style={{ '--p': `${pct}%` } as CSSProperties}
      onChange={(e) => onChange(parseFloat(e.target.value))}
    />
  );
}

export function Toggle({ on, onChange, label, disabled }: { on: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button type="button" role="switch" aria-checked={on} aria-label={label} title={label} disabled={disabled} className={`toggle ${on ? 'on' : ''}`} onClick={() => onChange(!on)}>
      <span className="knob" />
    </button>
  );
}

export function MuteButton({ muted, onChange, size = 18, label = 'Mute', disabled }: { muted: boolean; onChange: (m: boolean) => void; size?: number; label?: string; disabled?: boolean }) {
  return (
    <button type="button" className={`icon-btn mute ${muted ? 'is-muted' : ''}`} aria-pressed={muted} aria-label={muted ? `Unmute ${label}` : `Mute ${label}`} title={muted ? 'Unmute' : 'Mute'} disabled={disabled} onClick={() => onChange(!muted)}>
      {muted ? <VolumeX size={size} strokeWidth={1.8} /> : <Volume2 size={size} strokeWidth={1.8} />}
    </button>
  );
}

export function Chip({ children, tone = 'plain', title }: { children: ReactNode; tone?: 'plain' | 'accent' | 'ember' | 'solid'; title?: string }) {
  return <span className={`chip ${tone}`} title={title}>{children}</span>;
}

export function MiniCurve({ eq, width = 180, height = 44 }: { eq: EqSettings; width?: number; height?: number }) {
  const freqs = logFreqs(72);
  const ys = curve(eq, freqs);
  const yOf = (g: number) => height / 2 - (g / G_MAX) * (height / 2 - 3);
  const pts = ys.map((g, i) => `${((i / (freqs.length - 1)) * width).toFixed(1)},${yOf(g).toFixed(1)}`);
  const line = `M${pts.join(' L')}`;
  const area = `${line} L${width},${height / 2} L0,${height / 2} Z`;
  return (
    <svg className={`mini-curve ${eq.enabled ? 'on' : ''}`} width={width} height={height} viewBox={`0 0 ${width} ${height}`} aria-hidden>
      <line x1="0" x2={width} y1={height / 2} y2={height / 2} className="zero" />
      <path d={area} className="area" />
      <path d={line} className="line" />
    </svg>
  );
}

/** Vertical slider built from pointer events (consistent across WebView2 and WebKit). */
export function VSlider({ value, min, max, onChange, onCommit, label, height = 150 }: { value: number; min: number; max: number; onChange: (v: number) => void; onCommit?: () => void; label: string; height?: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const pct = (value - min) / (max - min);
  const zero = (0 - min) / (max - min);
  const fromEvent = (e: React.PointerEvent) => {
    const r = ref.current!.getBoundingClientRect();
    const t = 1 - (e.clientY - r.top) / r.height;
    return Math.round((min + Math.min(1, Math.max(0, t)) * (max - min)) * 10) / 10;
  };
  const lo = Math.min(pct, zero), hi = Math.max(pct, zero);
  return (
    <div
      ref={ref}
      className="vslider"
      style={{ height }}
      role="slider"
      tabIndex={0}
      aria-label={label}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={value}
      onPointerDown={(e) => {
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
        onChange(fromEvent(e));
      }}
      onPointerMove={(e) => {
        if (e.buttons & 1) onChange(fromEvent(e));
      }}
      onPointerUp={() => onCommit?.()}
      onDoubleClick={() => {
        onChange(0);
        onCommit?.();
      }}
      onKeyDown={(e) => {
        const step = e.shiftKey ? 3 : 0.5;
        if (e.key === 'ArrowUp') onChange(Math.min(max, value + step));
        else if (e.key === 'ArrowDown') onChange(Math.max(min, value - step));
        else return;
        e.preventDefault();
      }}
    >
      <div className="vs-track" />
      <div className="vs-zero" style={{ bottom: `${zero * 100}%` }} />
      <div className="vs-fill" style={{ bottom: `${lo * 100}%`, height: `${(hi - lo) * 100}%` }} />
      <div className="vs-thumb" style={{ bottom: `calc(${pct * 100}% - 7px)` }} />
    </div>
  );
}

export function Monogram({ name, src, size = 34 }: { name: string; src?: string; size?: number }) {
  if (src) return <img className="app-icon" src={src} width={size} height={size} alt="" draggable={false} />;
  const letter = (name.trim()[0] ?? '?').toUpperCase();
  return (
    <span className="app-icon mono" style={{ width: size, height: size, fontSize: size * 0.44 }} aria-hidden>
      {letter}
    </span>
  );
}

export const pct = (v: number) => `${Math.round(v * 100)}`;
