// 30 Hz meter feed, kept outside React so meters repaint without re-rendering the tree.
import type { Levels } from './types';

type Cb = (peaks: number[]) => void;
const subs = new Map<string, Set<Cb>>();
const last = new Map<string, number[]>();

export function pushLevels(l: Levels) {
  const seen = new Set<string>();
  for (const [id, p] of l.d) {
    fire(`d:${id}`, p);
    seen.add(`d:${id}`);
  }
  for (const [id, p] of l.s) {
    fire(`s:${id}`, p);
    seen.add(`s:${id}`);
  }
  for (const k of subs.keys()) if (!seen.has(k)) fire(k, []);
}

function fire(key: string, p: number[]) {
  last.set(key, p);
  subs.get(key)?.forEach((cb) => cb(p));
}

export function subscribe(key: string, cb: Cb): () => void {
  let set = subs.get(key);
  if (!set) subs.set(key, (set = new Set()));
  set.add(cb);
  const l = last.get(key);
  if (l) cb(l);
  return () => {
    set!.delete(cb);
  };
}

/** Linear peak -> 0..1 display position over a 54 dB window. */
export const norm = (p: number) => (p <= 0 ? 0 : Math.min(1, Math.max(0, (20 * Math.log10(p) + 54) / 54)));
