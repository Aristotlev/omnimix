//! Asynchronous sample-rate converter for clock-drift correction between a
//! capture clock and a render clock that run at the same nominal rate.
//! 16-tap Kaiser-windowed sinc, 128 phases with linear phase interpolation.

use std::sync::OnceLock;

pub const TAPS: usize = 16;
const HALF: usize = TAPS / 2;
const PHASES: usize = 128;
const CUTOFF: f64 = 0.96;
const BETA: f64 = 7.5;

struct Table {
    coef: Vec<f32>,
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..64 {
        term *= q / ((k * k) as f64);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        let mut coef = vec![0.0f32; (PHASES + 1) * TAPS];
        let i0b = bessel_i0(BETA);
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            let mut row = [0.0f64; TAPS];
            let mut sum = 0.0;
            for (k, r) in row.iter_mut().enumerate() {
                let d = k as f64 - (HALF as f64 - 1.0) - frac;
                let x = CUTOFF * d;
                let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                let wx = (d / HALF as f64).clamp(-1.0, 1.0);
                let w = bessel_i0(BETA * (1.0 - wx * wx).max(0.0).sqrt()) / i0b;
                *r = CUTOFF * sinc * w;
                sum += *r;
            }
            for k in 0..TAPS {
                coef[p * TAPS + k] = (row[k] / sum) as f32;
            }
        }
        Table { coef }
    })
}

pub struct Asrc {
    ch: usize,
    inbuf: Vec<f32>,
    pos: f64,
    ratio: f64,
}

impl Asrc {
    pub fn new(channels: usize, max_frames: usize) -> Asrc {
        let ch = channels.max(1);
        let mut inbuf = Vec::with_capacity((max_frames * 2 + TAPS * 4) * ch);
        inbuf.resize(HALF * ch, 0.0);
        let _ = table();
        Asrc { ch, inbuf, pos: (HALF - 1) as f64, ratio: 1.0 }
    }

    pub fn set_ratio(&mut self, r: f64) {
        self.ratio = r.clamp(0.9, 1.1);
    }

    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// Frames buffered inside the converter (not yet consumed).
    pub fn buffered_frames(&self) -> f64 {
        (self.inbuf.len() / self.ch) as f64 - self.pos
    }

    /// How many new input frames `process(out_frames)` will pull.
    pub fn frames_needed(&self, out_frames: usize) -> usize {
        if out_frames == 0 {
            return 0;
        }
        let last = self.pos + (out_frames as f64 - 1.0) * self.ratio;
        let need = last.floor() as usize + HALF + 1;
        need.saturating_sub(self.inbuf.len() / self.ch)
    }

    /// Produce `out_frames` frames. `pull(dst, frames)` must append exactly
    /// `frames` interleaved frames to `dst` (zero-fill if the source ran dry).
    pub fn process<F: FnMut(&mut Vec<f32>, usize)>(&mut self, out: &mut [f32], out_frames: usize, mut pull: F) {
        let ch = self.ch;
        let need = self.frames_needed(out_frames);
        if need > 0 {
            pull(&mut self.inbuf, need);
        }
        let t = table();
        let input = &self.inbuf;
        let mut pos = self.pos;
        for o in 0..out_frames {
            let ip = pos.floor();
            let frac = pos - ip;
            let base = ip as usize + 1 - HALF;
            let ph = frac * PHASES as f64;
            let p0 = (ph.floor() as usize).min(PHASES - 1);
            let mix = (ph - p0 as f64) as f32;
            let r0 = &t.coef[p0 * TAPS..p0 * TAPS + TAPS];
            let r1 = &t.coef[(p0 + 1) * TAPS..(p0 + 1) * TAPS + TAPS];
            for c in 0..ch {
                let mut acc = 0.0f32;
                for k in 0..TAPS {
                    let h = r0[k] + (r1[k] - r0[k]) * mix;
                    acc += input[(base + k) * ch + c] * h;
                }
                out[o * ch + c] = acc;
            }
            pos += self.ratio;
        }
        self.pos = pos;
        let drop = (self.pos.floor() as usize).saturating_sub(HALF - 1);
        let drop = drop.min(self.inbuf.len() / ch);
        if drop > 0 {
            self.inbuf.drain(..drop * ch);
            self.pos -= drop as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_tone(ratio: f64, freq: f64) -> (f64, f64) {
        let sr = 48_000.0;
        let mut a = Asrc::new(1, 480);
        a.set_ratio(ratio);
        let mut n_in = 0usize;
        let mut out_all = vec![];
        for _ in 0..100 {
            let mut out = vec![0.0f32; 480];
            a.process(&mut out, 480, |dst, frames| {
                for _ in 0..frames {
                    dst.push(((n_in as f64) * 2.0 * std::f64::consts::PI * freq / sr).sin() as f32);
                    n_in += 1;
                }
            });
            out_all.extend(out);
        }
        let tail = &out_all[out_all.len() - 9600..];
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs())) as f64;
        (peak, n_in as f64 / out_all.len() as f64)
    }

    #[test]
    fn unity_ratio_preserves_tone() {
        let (peak, consumed) = run_tone(1.0, 1000.0);
        assert!((peak - 1.0).abs() < 0.01, "peak {peak}");
        assert!((consumed - 1.0).abs() < 0.01);
    }

    #[test]
    fn drift_ratio_consumes_proportionally_and_stays_flat() {
        let (peak, consumed) = run_tone(1.002, 12_000.0);
        assert!((peak - 1.0).abs() < 0.03, "12k peak {peak}");
        assert!((consumed - 1.002).abs() < 0.002, "consumed {consumed}");
    }
}
