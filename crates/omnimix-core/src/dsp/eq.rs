//! Multiband EQ: serializable settings, real-time parameter block, and the
//! processing chain (click-free parameter changes via block crossfade).

use serde::{Deserialize, Serialize};

use super::biquad::{Coeffs, State};
use super::{db_to_lin, log_freqs, presets};

pub const MAX_BANDS: usize = 16;
pub const MAX_GAIN_DB: f32 = 18.0;
pub const MAX_PREAMP_DB: f32 = 18.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterKind {
    Peak,
    LowShelf,
    HighShelf,
    LowCut,
    HighCut,
    Notch,
}

impl FilterKind {
    pub fn uses_gain(self) -> bool {
        matches!(self, FilterKind::Peak | FilterKind::LowShelf | FilterKind::HighShelf)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Band {
    pub label: String,
    pub kind: FilterKind,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
    pub enabled: bool,
}

impl Band {
    pub fn new(label: &str, kind: FilterKind, freq: f32, q: f32) -> Band {
        Band { label: label.to_string(), kind, freq, gain_db: 0.0, q, enabled: true }
    }

    /// Whether this band changes the signal at all.
    pub fn is_effective(&self) -> bool {
        self.enabled && (!self.kind.uses_gain() || self.gain_db.abs() > 0.01)
    }

    pub fn coeffs(&self, sr: f64) -> Coeffs {
        Coeffs::design(self.kind, self.freq as f64, self.gain_db as f64, self.q as f64, sr)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EqSettings {
    pub enabled: bool,
    /// Name of the preset the bands came from (UI hint only).
    pub preset: Option<String>,
    /// Manual output trim in dB.
    pub preamp_db: f32,
    /// Automatically pull the level down by the curve's largest boost (clip guard).
    pub auto_preamp: bool,
    /// Stereo width: 0 = mono, 1 = unchanged, 2 = extra wide.
    pub width: f32,
    pub bands: Vec<Band>,
}

impl Default for EqSettings {
    fn default() -> Self {
        EqSettings {
            enabled: false,
            preset: Some("Flat".into()),
            preamp_db: 0.0,
            auto_preamp: true,
            width: 1.0,
            bands: presets::musical_bands(),
        }
    }
}

impl EqSettings {
    /// Clamp every field into its legal range so untrusted input (UI, files, FFI) is safe.
    pub fn sanitized(&self) -> EqSettings {
        let mut s = self.clone();
        s.bands.truncate(MAX_BANDS);
        for b in &mut s.bands {
            if !b.freq.is_finite() {
                b.freq = 1000.0;
            }
            b.freq = b.freq.clamp(10.0, 22_000.0);
            if !b.gain_db.is_finite() {
                b.gain_db = 0.0;
            }
            b.gain_db = b.gain_db.clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
            if !b.q.is_finite() {
                b.q = 1.0;
            }
            b.q = b.q.clamp(0.1, 24.0);
            if b.label.len() > 24 {
                b.label.truncate(24);
            }
        }
        if !s.preamp_db.is_finite() {
            s.preamp_db = 0.0;
        }
        s.preamp_db = s.preamp_db.clamp(-MAX_PREAMP_DB, MAX_PREAMP_DB);
        if !s.width.is_finite() {
            s.width = 1.0;
        }
        s.width = s.width.clamp(0.0, 2.0);
        s
    }

    /// True when processing would not change the audio.
    pub fn is_neutral(&self) -> bool {
        !self.bands.iter().any(|b| b.is_effective()) && self.preamp_db.abs() < 0.01 && (self.width - 1.0).abs() < 0.001
    }

    /// Combined filter response in dB at each frequency (preamp excluded).
    pub fn curve_db(&self, freqs: &[f64], sr: f64) -> Vec<f64> {
        let cs: Vec<Coeffs> = self.bands.iter().filter(|b| b.is_effective()).map(|b| b.coeffs(sr)).collect();
        freqs.iter().map(|&f| cs.iter().map(|c| c.magnitude_db(f, sr)).sum()).collect()
    }

    /// Largest boost of the curve within the audible band.
    pub fn peak_boost_db(&self, sr: f64) -> f64 {
        let fs = log_freqs(192, 20.0, (sr * 0.45).min(20_000.0));
        self.curve_db(&fs, sr).into_iter().fold(0.0f64, f64::max)
    }

    /// Output gain actually applied (manual trim plus auto clip guard).
    pub fn effective_preamp_db(&self, sr: f64) -> f64 {
        let auto = if self.auto_preamp { -self.peak_boost_db(sr) } else { 0.0 };
        self.preamp_db as f64 + auto
    }
}

/// Real-time parameter block. Fixed size and `Copy` so it moves through a
/// mailbox without allocating on the audio thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqParams {
    pub active: bool,
    pub n: usize,
    pub coeffs: [Coeffs; MAX_BANDS],
    pub preamp: f64,
    pub width: f32,
}

impl EqParams {
    pub const BYPASS: EqParams =
        EqParams { active: false, n: 0, coeffs: [Coeffs::IDENTITY; MAX_BANDS], preamp: 1.0, width: 1.0 };

    pub fn from_settings(s: &EqSettings, sr: f64) -> EqParams {
        if !s.enabled {
            return EqParams::BYPASS;
        }
        let s = s.sanitized();
        let mut p = EqParams::BYPASS;
        for b in s.bands.iter().filter(|b| b.is_effective()).take(MAX_BANDS) {
            p.coeffs[p.n] = b.coeffs(sr);
            p.n += 1;
        }
        p.preamp = db_to_lin(s.effective_preamp_db(sr));
        p.width = s.width;
        p.active = p.n > 0 || (p.preamp - 1.0).abs() > 1e-6 || (p.width - 1.0).abs() > 1e-6;
        p
    }
}

/// Multichannel EQ processor (interleaved f32).
pub struct EqChain {
    ch: usize,
    cur: EqParams,
    next: Option<EqParams>,
    st: Vec<[State; MAX_BANDS]>,
    st_next: Vec<[State; MAX_BANDS]>,
    scratch: Vec<f32>,
}

impl EqChain {
    pub fn new(channels: usize, max_frames: usize) -> EqChain {
        let ch = channels.max(1);
        EqChain {
            ch,
            cur: EqParams::BYPASS,
            next: None,
            st: vec![[State::default(); MAX_BANDS]; ch],
            st_next: vec![[State::default(); MAX_BANDS]; ch],
            scratch: vec![0.0; max_frames * ch],
        }
    }

    pub fn params(&self) -> &EqParams {
        self.next.as_ref().unwrap_or(&self.cur)
    }

    /// Queue new parameters; they are crossfaded in over the next block.
    pub fn set_params(&mut self, p: EqParams) {
        if p != self.cur || self.next.is_some() {
            self.next = Some(p);
        }
    }

    pub fn process(&mut self, buf: &mut [f32], frames: usize) {
        let n = frames * self.ch;
        debug_assert!(buf.len() >= n);
        if let Some(next) = self.next.take() {
            if self.scratch.len() < n {
                self.scratch.resize(n, 0.0);
            }
            self.scratch[..n].copy_from_slice(&buf[..n]);
            for c in 0..self.ch {
                self.st_next[c] = if self.cur.active { self.st[c] } else { [State::default(); MAX_BANDS] };
            }
            run(&self.cur, &mut self.st, &mut buf[..n], self.ch);
            run(&next, &mut self.st_next, &mut self.scratch[..n], self.ch);
            let inv = 1.0 / frames.max(1) as f32;
            for f in 0..frames {
                let t = (f + 1) as f32 * inv;
                for c in 0..self.ch {
                    let i = f * self.ch + c;
                    buf[i] = buf[i] * (1.0 - t) + self.scratch[i] * t;
                }
            }
            std::mem::swap(&mut self.st, &mut self.st_next);
            self.cur = next;
        } else {
            run(&self.cur, &mut self.st, &mut buf[..n], self.ch);
        }
        for c in 0..self.ch {
            for s in self.st[c].iter_mut() {
                s.flush();
            }
        }
    }

    pub fn reset(&mut self) {
        for c in 0..self.ch {
            self.st[c] = [State::default(); MAX_BANDS];
        }
    }
}

fn run(p: &EqParams, st: &mut [[State; MAX_BANDS]], buf: &mut [f32], ch: usize) {
    if !p.active {
        return;
    }
    let frames = buf.len() / ch;
    for f in 0..frames {
        for c in 0..ch {
            let i = f * ch + c;
            let mut x = buf[i] as f64 * p.preamp;
            let s = &mut st[c];
            for b in 0..p.n {
                x = s[b].tick(&p.coeffs[b], x);
            }
            buf[i] = x as f32;
        }
        if ch == 2 && (p.width - 1.0).abs() > 1e-6 {
            let i = f * 2;
            let (l, r) = (buf[i], buf[i + 1]);
            let m = 0.5 * (l + r);
            let sd = 0.5 * (l - r) * p.width;
            buf[i] = m + sd;
            buf[i + 1] = m - sd;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::lin_to_db;
    use std::f64::consts::PI;

    fn sine_gain_db(eq: &mut EqChain, freq: f64, sr: f64) -> f64 {
        let frames = 480;
        let mut phase = 0.0f64;
        let mut sum_sq = 0.0f64;
        let mut count = 0usize;
        for blk in 0..200 {
            let mut buf = vec![0.0f32; frames * 2];
            for f in 0..frames {
                let v = (phase).sin() as f32 * 0.25;
                phase += 2.0 * PI * freq / sr;
                buf[f * 2] = v;
                buf[f * 2 + 1] = v;
            }
            eq.process(&mut buf, frames);
            if blk > 150 {
                for v in buf.iter() {
                    sum_sq += (*v as f64) * (*v as f64);
                    count += 1;
                }
            }
        }
        let rms = (sum_sq / count as f64).sqrt();
        lin_to_db(rms / (0.25 / 2f64.sqrt()))
    }

    #[test]
    fn flat_is_identity() {
        let s = EqSettings { enabled: true, auto_preamp: false, ..Default::default() };
        let p = EqParams::from_settings(&s, 48_000.0);
        assert!(!p.active);
        let mut eq = EqChain::new(2, 480);
        eq.set_params(p);
        let mut buf: Vec<f32> = (0..960).map(|i| ((i as f32) * 0.01).sin()).collect();
        let orig = buf.clone();
        eq.process(&mut buf, 480);
        assert_eq!(buf, orig);
    }

    #[test]
    fn peak_band_hits_target_gain() {
        let sr = 48_000.0;
        let mut s = EqSettings { enabled: true, auto_preamp: false, ..Default::default() };
        let bass = s.bands.iter_mut().find(|b| b.label == "Bass").unwrap();
        bass.gain_db = 6.0;
        let f0 = bass.freq as f64;
        let mut eq = EqChain::new(2, 480);
        eq.set_params(EqParams::from_settings(&s, sr));
        let g = sine_gain_db(&mut eq, f0, sr);
        let predicted = s.curve_db(&[f0], sr)[0];
        assert!((g - predicted).abs() < 0.2, "measured {g} predicted {predicted}");
        assert!(g > 5.0, "bass boost too small: {g}");
        let g_far = sine_gain_db(&mut EqChain::new(2, 480), 8000.0, sr);
        assert!(g_far.abs() < 0.1);
    }

    #[test]
    fn auto_preamp_prevents_boost_overshoot() {
        let sr = 48_000.0;
        let mut s = EqSettings { enabled: true, ..Default::default() };
        for b in s.bands.iter_mut() {
            b.gain_db = 9.0;
        }
        let pre = s.effective_preamp_db(sr);
        assert!(pre < -8.0, "auto preamp should cut by roughly the peak boost, got {pre}");
        let mut eq = EqChain::new(2, 480);
        eq.set_params(EqParams::from_settings(&s, sr));
        for f in [60.0, 1000.0, 9000.0] {
            let g = sine_gain_db(&mut eq, f, sr);
            assert!(g < 0.5, "{f} Hz overshoots: {g} dB");
        }
    }

    #[test]
    fn hostile_input_is_sanitized() {
        let mut s = EqSettings { enabled: true, ..Default::default() };
        s.bands[0].freq = f32::NAN;
        s.bands[1].gain_db = 900.0;
        s.bands[2].q = -1.0;
        s.preamp_db = f32::INFINITY;
        for _ in 0..40 {
            s.bands.push(Band::new("x", FilterKind::Peak, 1000.0, 1.0));
        }
        let c = s.sanitized();
        assert_eq!(c.bands.len(), MAX_BANDS);
        assert!(c.bands[0].freq.is_finite());
        assert_eq!(c.bands[1].gain_db, MAX_GAIN_DB);
        assert!(c.bands[2].q > 0.0);
        assert_eq!(c.preamp_db, 0.0);
        let p = EqParams::from_settings(&s, 48_000.0);
        let mut eq = EqChain::new(2, 64);
        eq.set_params(p);
        let mut buf = vec![0.5f32; 128];
        eq.process(&mut buf, 64);
        assert!(buf.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn crossfade_has_no_jump() {
        let sr = 48_000.0;
        let mut eq = EqChain::new(1, 256);
        let mut s = EqSettings { enabled: true, auto_preamp: false, ..Default::default() };
        let mut last = 0.0f32;
        let mut max_step = 0.0f32;
        let mut phase = 0.0f64;
        for blk in 0..60 {
            if blk % 10 == 0 {
                s.bands[5].gain_db = if s.bands[5].gain_db > 0.0 { -12.0 } else { 12.0 };
                eq.set_params(EqParams::from_settings(&s, sr));
            }
            let mut buf = vec![0.0f32; 256];
            for v in buf.iter_mut() {
                *v = (phase.sin() * 0.3) as f32;
                phase += 2.0 * PI * 200.0 / sr;
            }
            eq.process(&mut buf, 256);
            for v in buf {
                max_step = max_step.max((v - last).abs());
                last = v;
            }
        }
        assert!(max_step < 0.05, "discontinuity {max_step}");
    }
}
