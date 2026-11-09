import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Plus, RotateCcw, Save, Trash2, X } from 'lucide-react';
import type { Band, EqSettings, EqTarget, FilterKind, Preset } from './types';
import { bandCurve, cloneEq, curve, effectivePreamp, fmtDb, fmtFreq, fx, G_MAX, KIND_LABEL, logFreqs, usesGain, xf, F_MAX, F_MIN } from './eq';
import { Meter, Slider, Toggle, VSlider } from './ui';

const FREQ_TICKS = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
const MINOR = [30, 40, 60, 70, 80, 90, 300, 400, 600, 700, 800, 900, 3000, 4000, 6000, 7000, 8000, 9000];
const GAIN_TICKS = [-18, -12, -6, 0, 6, 12, 18];
const H = 300;
const PAD = 18;
const y = (g: number) => PAD + ((G_MAX - g) / (2 * G_MAX)) * (H - 2 * PAD);
const gOf = (py: number) => G_MAX - ((py - PAD) / (H - 2 * PAD)) * 2 * G_MAX;

function CurveEditor({ eq, sel, setSel, update, levelKey }: { eq: EqSettings; sel: number; setSel: (i: number) => void; update: (i: number, b: Partial<Band>) => void; levelKey: string }) {
  const wrap = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(860);
  useLayoutEffect(() => {
    const el = wrap.current!;
    const ro = new ResizeObserver(() => setW(Math.max(320, el.clientWidth - 32)));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const freqs = useMemo(() => logFreqs(Math.round(w / 3)), [w]);
  const total = curve(eq, freqs);
  const selBand = eq.bands[sel];
  const selCurve = selBand && selBand.enabled ? bandCurve(selBand, freqs) : null;
  const path = (ys: number[]) => 'M' + ys.map((g, i) => `${fx(freqs[i], w).toFixed(1)},${y(Math.max(-G_MAX - 4, Math.min(G_MAX + 4, g))).toFixed(1)}`).join(' L');
  const line = path(total);
  const area = `${line} L${w},${y(0)} L0,${y(0)} Z`;
  const drag = useRef<number | null>(null);

  const move = (e: React.PointerEvent<SVGSVGElement>) => {
    if (drag.current === null) return;
    const r = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
    const px = Math.min(Math.max(e.clientX - r.left, 0), w);
    const py = Math.min(Math.max(e.clientY - r.top, PAD), H - PAD);
    const b = eq.bands[drag.current];
    const freq = Math.round(Math.min(F_MAX, Math.max(F_MIN, xf(px, w))));
    const patch: Partial<Band> = { freq };
    if (usesGain(b.kind)) patch.gainDb = Math.round(gOf(py) * 10) / 10;
    update(drag.current, patch);
  };

  return (
    <div className="curve-wrap" ref={wrap}>
      <svg
        className="curve"
        width={w}
        height={H}
        onPointerMove={move}
        onPointerUp={() => (drag.current = null)}
        onPointerLeave={() => (drag.current = null)}
        onWheel={(e) => {
          if (!selBand) return;
          const q = Math.min(24, Math.max(0.1, selBand.q * Math.exp(-e.deltaY * 0.0015)));
          update(sel, { q: Math.round(q * 100) / 100 });
        }}
      >
        <defs>
          <linearGradient id="eqfill" x1="0" x2="0" y1="0" y2="1">
            <stop offset="0%" stopColor="var(--accent)" stopOpacity="0.32" />
            <stop offset="50%" stopColor="var(--accent)" stopOpacity="0.06" />
            <stop offset="100%" stopColor="var(--accent)" stopOpacity="0.32" />
          </linearGradient>
        </defs>
        {MINOR.map((f) => <line key={f} x1={fx(f, w)} x2={fx(f, w)} y1={PAD} y2={H - PAD} className="grid minor" />)}
        {FREQ_TICKS.map((f) => (
          <g key={f}>
            <line x1={fx(f, w)} x2={fx(f, w)} y1={PAD} y2={H - PAD} className="grid" />
            <text x={Math.min(w - 14, Math.max(12, fx(f, w)))} y={H - 4} className="tick" textAnchor="middle">{fmtFreq(f)}</text>
          </g>
        ))}
        {GAIN_TICKS.map((g) => (
          <g key={g}>
            <line x1={0} x2={w} y1={y(g)} y2={y(g)} className={`grid ${g === 0 ? 'zero' : ''}`} />
            <text x={6} y={y(g) - 4} className="tick">{g > 0 ? `+${g}` : g}</text>
          </g>
        ))}
        <path d={area} fill="url(#eqfill)" className={eq.enabled ? '' : 'off'} />
        {selCurve && <path d={path(selCurve)} className="band-curve" />}
        <path d={line} className={`total ${eq.enabled ? '' : 'off'}`} />
        {eq.bands.map((b, i) => {
          const cx = fx(Math.min(F_MAX, Math.max(F_MIN, b.freq)), w);
          const cy = usesGain(b.kind) ? y(b.gainDb) : y(0);
          return (
            <g
              key={i}
              className={`node ${i === sel ? 'sel' : ''} ${b.enabled ? '' : 'disabled'}`}
              onPointerDown={(e) => {
                setSel(i);
                drag.current = i;
                (e.currentTarget.ownerSVGElement as SVGSVGElement).setPointerCapture(e.pointerId);
              }}
              onDoubleClick={() => update(i, { gainDb: 0 })}
            >
              <circle cx={cx} cy={cy} r={i === sel ? 9 : 7} />
              <text x={cx} y={cy - 14} textAnchor="middle">{b.label}</text>
            </g>
          );
        })}
      </svg>
      <div className="curve-meter"><Meter levelKey={levelKey} vertical channels={2} /></div>
    </div>
  );
}

function newBand(eq: EqSettings): Band {
  const used = new Set(eq.bands.map((b) => Math.round(b.freq)));
  let f = 1500;
  while (used.has(f)) f += 250;
  return { label: `Band ${eq.bands.length + 1}`, kind: 'peak', freq: f, gainDb: 0, q: 1, enabled: true };
}

export function EqModal({
  target, eq: initial, presets, levelKey, onChange, onClose, onSavePreset, onDeletePreset,
}: {
  target: EqTarget;
  eq: EqSettings;
  presets: Preset[];
  levelKey: string;
  onChange: (eq: EqSettings, final?: boolean) => void;
  onClose: () => void;
  onSavePreset: (name: string, eq: EqSettings) => void;
  onDeletePreset: (name: string) => void;
}) {
  const [eq, setEq] = useState<EqSettings>(() => cloneEq(initial));
  const [sel, setSel] = useState(1);
  const [view, setView] = useState<'musical' | 'parametric'>('musical');
  const [saving, setSaving] = useState<string | null>(null);

  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', k);
    return () => window.removeEventListener('keydown', k);
  }, [onClose]);

  const commit = (next: EqSettings, final = false) => {
    setEq(next);
    onChange(next, final);
  };
  const update = (i: number, patch: Partial<Band>) => {
    const next = cloneEq(eq);
    next.bands[i] = { ...next.bands[i], ...patch };
    next.preset = null;
    if (!next.enabled) next.enabled = true;
    commit(next);
  };
  const applyPreset = (name: string) => {
    const p = presets.find((x) => x.name === name);
    if (!p) return;
    commit({ ...cloneEq(p.eq), enabled: true, preset: p.name, width: eq.width }, true);
  };
  const reset = () => {
    const next = cloneEq(eq);
    next.bands = next.bands.map((b) => ({ ...b, gainDb: 0 }));
    next.preampDb = 0;
    next.width = 1;
    next.preset = 'Flat';
    commit(next, true);
  };
  const userPreset = presets.find((p) => p.name === eq.preset && !p.builtin);
  const pre = effectivePreamp(eq);

  return (
    <div className="modal-scrim" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal eq-modal" role="dialog" aria-label={`${target.title} equalizer`}>
        <header className="modal-head">
          <div>
            <div className="eyebrow">{target.kind === 'device' ? 'Device EQ · everything on this output' : 'App EQ · this app only'}</div>
            <h2>{target.title}<span className="sub">{target.subtitle}</span></h2>
          </div>
          <div className="head-actions">
            <label className="inline-toggle">
              <span>{eq.enabled ? 'On' : 'Bypassed'}</span>
              <Toggle on={eq.enabled} label="Enable EQ" onChange={(v) => commit({ ...eq, enabled: v }, true)} />
            </label>
            <button className="icon-btn" aria-label="Close" onClick={onClose}><X size={18} /></button>
          </div>
        </header>

        <div className="eq-toolbar">
          <div className="seg" role="tablist">
            <button className={view === 'musical' ? 'on' : ''} onClick={() => setView('musical')}>Musical</button>
            <button className={view === 'parametric' ? 'on' : ''} onClick={() => setView('parametric')}>Parametric</button>
          </div>
          <select className="select" value={eq.preset ?? ''} onChange={(e) => applyPreset(e.target.value)} aria-label="Preset">
            <option value="" disabled>Custom</option>
            <optgroup label="Built-in">
              {presets.filter((p) => p.builtin).map((p) => <option key={p.name} value={p.name}>{p.name}</option>)}
            </optgroup>
            {presets.some((p) => !p.builtin) && (
              <optgroup label="Yours">
                {presets.filter((p) => !p.builtin).map((p) => <option key={p.name} value={p.name}>{p.name}</option>)}
              </optgroup>
            )}
          </select>
          {userPreset && (
            <button className="icon-btn" title="Delete preset" aria-label="Delete preset" onClick={() => onDeletePreset(userPreset.name)}><Trash2 size={16} /></button>
          )}
          {saving === null ? (
            <button className="btn ghost" onClick={() => setSaving('')}><Save size={15} /> Save preset</button>
          ) : (
            <form
              className="save-form"
              onSubmit={(e) => {
                e.preventDefault();
                if (saving.trim()) {
                  onSavePreset(saving.trim(), eq);
                  setEq({ ...eq, preset: saving.trim() });
                }
                setSaving(null);
              }}
            >
              <input autoFocus className="input" placeholder="Preset name" value={saving} maxLength={40} onChange={(e) => setSaving(e.target.value)} />
              <button className="btn" type="submit">Save</button>
            </form>
          )}
          <span className="spacer" />
          <button className="btn ghost" onClick={reset}><RotateCcw size={15} /> Flat</button>
        </div>

        <CurveEditor eq={eq} sel={sel} setSel={setSel} update={update} levelKey={levelKey} />
        <div className="curve-hint">Drag points for frequency and gain · scroll to change width (Q) · double-click to zero</div>

        {view === 'musical' ? (
          <div className="bands">
            {eq.bands.map((b, i) => (
              <div key={i} className={`band ${i === sel ? 'sel' : ''} ${b.enabled ? '' : 'disabled'}`} onPointerDown={() => setSel(i)}>
                <div className="band-label">{b.label}</div>
                <div className="band-freq">{fmtFreq(b.freq)} Hz</div>
                {usesGain(b.kind) ? (
                  <VSlider value={b.gainDb} min={-G_MAX} max={G_MAX} label={`${b.label} gain`} onChange={(g) => update(i, { gainDb: g })} onCommit={() => onChange(eq, true)} height={132} />
                ) : (
                  <div className="vslider placeholder" style={{ height: 132 }}>{KIND_LABEL[b.kind]}</div>
                )}
                <div className={`band-gain ${b.gainDb > 0.05 ? 'up' : b.gainDb < -0.05 ? 'down' : ''}`}>{usesGain(b.kind) ? fmtDb(b.gainDb) : '—'}</div>
              </div>
            ))}
          </div>
        ) : (
          <div className="param">
            <div className="param-row head">
              <span>On</span><span>Name</span><span>Type</span><span>Freq (Hz)</span><span>Gain (dB)</span><span>Q</span><span />
            </div>
            {eq.bands.map((b, i) => (
              <div key={i} className={`param-row ${i === sel ? 'sel' : ''}`} onPointerDown={() => setSel(i)}>
                <Toggle on={b.enabled} label={`Enable ${b.label}`} onChange={(v) => update(i, { enabled: v })} />
                <input className="input" value={b.label} maxLength={24} onChange={(e) => update(i, { label: e.target.value })} />
                <select className="select" value={b.kind} onChange={(e) => update(i, { kind: e.target.value as FilterKind })}>
                  {(Object.keys(KIND_LABEL) as FilterKind[]).map((k) => <option key={k} value={k}>{KIND_LABEL[k]}</option>)}
                </select>
                <input className="input num" type="number" min={10} max={22000} step={1} value={Math.round(b.freq)} onChange={(e) => update(i, { freq: Number(e.target.value) || b.freq })} />
                <input className="input num" type="number" min={-G_MAX} max={G_MAX} step={0.1} disabled={!usesGain(b.kind)} value={b.gainDb} onChange={(e) => update(i, { gainDb: Math.max(-G_MAX, Math.min(G_MAX, Number(e.target.value) || 0)) })} />
                <input className="input num" type="number" min={0.1} max={24} step={0.05} value={b.q} onChange={(e) => update(i, { q: Math.max(0.1, Math.min(24, Number(e.target.value) || b.q)) })} />
                <button
                  className="icon-btn"
                  aria-label={`Remove ${b.label}`}
                  disabled={eq.bands.length <= 1}
                  onClick={() => {
                    const next = cloneEq(eq);
                    next.bands.splice(i, 1);
                    setSel(Math.max(0, Math.min(sel, next.bands.length - 1)));
                    commit(next, true);
                  }}
                >
                  <Trash2 size={15} />
                </button>
              </div>
            ))}
            <button className="btn ghost add" disabled={eq.bands.length >= 16} onClick={() => commit({ ...cloneEq(eq), bands: [...eq.bands.map((b) => ({ ...b })), newBand(eq)] }, true)}>
              <Plus size={15} /> Add band
            </button>
          </div>
        )}

        <footer className="eq-foot">
          <div className="foot-item">
            <span className="foot-label">Preamp</span>
            <Slider min={-18} max={18} step={0.5} value={eq.preampDb} onChange={(v) => commit({ ...eq, preampDb: v })} label="Preamp" />
            <span className="foot-val">{fmtDb(pre)} dB</span>
          </div>
          <label className="foot-item compact">
            <span className="foot-label">Clip guard</span>
            <Toggle on={eq.autoPreamp} label="Automatic clip guard" onChange={(v) => commit({ ...eq, autoPreamp: v }, true)} />
          </label>
          <div className="foot-item">
            <span className="foot-label">Width</span>
            <Slider min={0} max={2} step={0.05} value={eq.width} onChange={(v) => commit({ ...eq, width: v })} label="Stereo width" />
            <span className="foot-val">{Math.round(eq.width * 100)}%</span>
          </div>
        </footer>
      </div>
    </div>
  );
}
