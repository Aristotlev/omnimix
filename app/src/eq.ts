// Same RBJ math as omnimix-core/src/dsp/biquad.rs, for instant curve drawing.
import type { Band, EqSettings, FilterKind } from './types';

export const SR = 48000;
export const F_MIN = 20;
export const F_MAX = 20000;
export const G_MAX = 18;

interface C { b0: number; b1: number; b2: number; a1: number; a2: number }

export function design(kind: FilterKind, freq: number, gainDb: number, q: number, sr = SR): C {
  const f = Math.min(Math.max(freq, 5), sr * 0.49);
  const Q = Math.min(Math.max(q, 0.05), 40);
  const A = Math.pow(10, gainDb / 40);
  const w0 = (2 * Math.PI * f) / sr;
  const sn = Math.sin(w0), cs = Math.cos(w0);
  const alpha = sn / (2 * Q);
  let b0 = 1, b1 = 0, b2 = 0, a0 = 1, a1 = 0, a2 = 0;
  switch (kind) {
    case 'peak':
      [b0, b1, b2, a0, a1, a2] = [1 + alpha * A, -2 * cs, 1 - alpha * A, 1 + alpha / A, -2 * cs, 1 - alpha / A];
      break;
    case 'low_shelf': {
      const s = 2 * Math.sqrt(A) * alpha;
      [b0, b1, b2] = [A * (A + 1 - (A - 1) * cs + s), 2 * A * (A - 1 - (A + 1) * cs), A * (A + 1 - (A - 1) * cs - s)];
      [a0, a1, a2] = [A + 1 + (A - 1) * cs + s, -2 * (A - 1 + (A + 1) * cs), A + 1 + (A - 1) * cs - s];
      break;
    }
    case 'high_shelf': {
      const s = 2 * Math.sqrt(A) * alpha;
      [b0, b1, b2] = [A * (A + 1 + (A - 1) * cs + s), -2 * A * (A - 1 + (A + 1) * cs), A * (A + 1 + (A - 1) * cs - s)];
      [a0, a1, a2] = [A + 1 - (A - 1) * cs + s, 2 * (A - 1 - (A + 1) * cs), A + 1 - (A - 1) * cs - s];
      break;
    }
    case 'high_cut':
      [b0, b1, b2, a0, a1, a2] = [(1 - cs) / 2, 1 - cs, (1 - cs) / 2, 1 + alpha, -2 * cs, 1 - alpha];
      break;
    case 'low_cut':
      [b0, b1, b2, a0, a1, a2] = [(1 + cs) / 2, -(1 + cs), (1 + cs) / 2, 1 + alpha, -2 * cs, 1 - alpha];
      break;
    case 'notch':
      [b0, b1, b2, a0, a1, a2] = [1, -2 * cs, 1, 1 + alpha, -2 * cs, 1 - alpha];
      break;
  }
  return { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 };
}

export function magDb(c: C, f: number, sr = SR): number {
  const w = (2 * Math.PI * f) / sr;
  const c1 = Math.cos(w), s1 = Math.sin(w), c2 = Math.cos(2 * w), s2 = Math.sin(2 * w);
  const nr = c.b0 + c.b1 * c1 + c.b2 * c2, ni = -(c.b1 * s1 + c.b2 * s2);
  const dr = 1 + c.a1 * c1 + c.a2 * c2, di = -(c.a1 * s1 + c.a2 * s2);
  return 10 * Math.log10(Math.max((nr * nr + ni * ni) / Math.max(dr * dr + di * di, 1e-300), 1e-30));
}

export const usesGain = (k: FilterKind) => k === 'peak' || k === 'low_shelf' || k === 'high_shelf';
export const effective = (b: Band) => b.enabled && (!usesGain(b.kind) || Math.abs(b.gainDb) > 0.01);

export function logFreqs(n: number, lo = F_MIN, hi = F_MAX): number[] {
  const a = Math.log(lo), b = Math.log(hi);
  return Array.from({ length: n }, (_, i) => Math.exp(a + ((b - a) * i) / (n - 1)));
}

export function bandCurve(b: Band, freqs: number[]): number[] {
  const c = design(b.kind, b.freq, b.gainDb, b.q);
  return freqs.map((f) => magDb(c, f));
}

export function curve(eq: EqSettings, freqs: number[]): number[] {
  const cs = eq.bands.filter(effective).map((b) => design(b.kind, b.freq, b.gainDb, b.q));
  return freqs.map((f) => cs.reduce((s, c) => s + magDb(c, f), 0));
}

export function peakBoost(eq: EqSettings): number {
  return Math.max(0, ...curve(eq, logFreqs(192, 20, 20000)));
}

export function effectivePreamp(eq: EqSettings): number {
  return eq.preampDb + (eq.autoPreamp ? -peakBoost(eq) : 0);
}

export const fx = (f: number, w: number) => ((Math.log(f) - Math.log(F_MIN)) / (Math.log(F_MAX) - Math.log(F_MIN))) * w;
export const xf = (x: number, w: number) => Math.exp(Math.log(F_MIN) + (x / w) * (Math.log(F_MAX) - Math.log(F_MIN)));

export function fmtFreq(f: number): string {
  if (f >= 1000) return `${(f / 1000).toFixed(f >= 10000 ? 0 : 1).replace(/\.0$/, '')}k`;
  return `${Math.round(f)}`;
}

export function fmtDb(g: number): string {
  const v = Math.abs(g) < 0.05 ? 0 : g;
  return `${v > 0 ? '+' : ''}${v.toFixed(1)}`;
}

export const KIND_LABEL: Record<FilterKind, string> = {
  peak: 'Bell',
  low_shelf: 'Low shelf',
  high_shelf: 'High shelf',
  low_cut: 'Low cut',
  high_cut: 'High cut',
  notch: 'Notch',
};

export function isFlat(eq: EqSettings): boolean {
  return !eq.bands.some(effective) && Math.abs(eq.preampDb) < 0.01 && Math.abs(eq.width - 1) < 0.001;
}

export function cloneEq(eq: EqSettings): EqSettings {
  return { ...eq, bands: eq.bands.map((b) => ({ ...b })) };
}
