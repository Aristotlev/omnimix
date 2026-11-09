//! Real-time graph: per-source [`Strip`]s (drift-corrected input, EQ, gain)
//! summed into a per-device [`BusProcessor`] (device EQ, safety limiter).
//!
//! Platform backends own the threads. A capture thread pushes frames into a
//! [`ring::Producer`]; the device render callback calls [`BusProcessor::render`].
//! Everything here is OS-agnostic and allocation-free after warm-up.

pub mod ring;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::dsp::asrc::Asrc;
use crate::dsp::limiter::Limiter;
use crate::dsp::{EqChain, EqParams};
use ring::{Consumer, Producer};

/// Largest render block processed in one pass (longer callbacks are split).
pub const MAX_BLOCK: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LatencyMode {
    Low,
    #[default]
    Balanced,
    Safe,
}

impl LatencyMode {
    /// Target ring fill in milliseconds.
    pub fn buffer_ms(self) -> f64 {
        match self {
            LatencyMode::Low => 20.0,
            LatencyMode::Balanced => 35.0,
            LatencyMode::Safe => 60.0,
        }
    }

    /// Render buffer the backend should request, in milliseconds.
    pub fn output_ms(self) -> f64 {
        match self {
            LatencyMode::Low => 10.0,
            LatencyMode::Balanced => 20.0,
            LatencyMode::Safe => 30.0,
        }
    }
}

#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        AtomicF32(AtomicU32::new(v.to_bits()))
    }
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
    /// Store max(current, v).
    pub fn fetch_max(&self, v: f32) {
        let mut cur = self.0.load(Ordering::Relaxed);
        while f32::from_bits(cur) < v {
            match self.0.compare_exchange_weak(cur, v.to_bits(), Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(x) => cur = x,
            }
        }
    }
    /// Read and reset to zero (meter consumption).
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Ordering::Relaxed))
    }
}

/// Latest-value mailbox. The audio thread only ever `try_lock`s it.
pub struct Mailbox<T: Copy> {
    slot: Mutex<Option<T>>,
}

impl<T: Copy> Default for Mailbox<T> {
    fn default() -> Self {
        Mailbox { slot: Mutex::new(None) }
    }
}

impl<T: Copy> Mailbox<T> {
    pub fn post(&self, v: T) {
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
    }
    pub fn take(&self) -> Option<T> {
        match self.slot.try_lock() {
            Ok(mut g) => g.take(),
            Err(_) => None,
        }
    }
}

pub const MAX_METER_CH: usize = 8;

/// Shared between the control thread and the render thread for one source.
pub struct StripControls {
    pub key: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub eq: Mailbox<EqParams>,
    pub gain: AtomicF32,
    pub muted: AtomicBool,
    /// Bit n set = channel n silenced.
    pub ch_mute: AtomicU32,
    pub peaks: [AtomicF32; MAX_METER_CH],
    pub fill_ms: AtomicF32,
    pub ratio_ppm: AtomicF32,
    pub underruns: AtomicU32,
    pub resyncs: AtomicU32,
    /// Set by the platform capture thread while it is delivering audio.
    pub capture_alive: AtomicBool,
    pub capture_error: Mutex<Option<String>>,
}

impl StripControls {
    pub fn take_peaks(&self) -> Vec<f32> {
        (0..self.channels.min(MAX_METER_CH)).map(|c| self.peaks[c].take()).collect()
    }
}

pub struct Strip {
    ctl: Arc<StripControls>,
    rx: Consumer,
    asrc: Asrc,
    eq: EqChain,
    buf: Vec<f32>,
    ch: usize,
    sr: f64,
    gain: f32,
    target: f64,
    fill_avg: f64,
    integ: f64,
    priming: bool,
}

/// Create a strip. Hand the [`Producer`] to the capture thread and the
/// boxed [`Strip`] to [`BusControls::add_strip`].
pub fn new_strip(key: &str, sample_rate: u32, channels: usize, latency: LatencyMode) -> (Arc<StripControls>, Producer, Box<Strip>) {
    let ch = channels.max(1);
    let (tx, rx) = ring::ring(sample_rate as usize, ch);
    let ctl = Arc::new(StripControls {
        key: key.to_string(),
        sample_rate,
        channels: ch,
        eq: Mailbox::default(),
        gain: AtomicF32::new(0.0),
        muted: AtomicBool::new(false),
        ch_mute: AtomicU32::new(0),
        peaks: Default::default(),
        fill_ms: AtomicF32::new(0.0),
        ratio_ppm: AtomicF32::new(0.0),
        underruns: AtomicU32::new(0),
        resyncs: AtomicU32::new(0),
        capture_alive: AtomicBool::new(false),
        capture_error: Mutex::new(None),
    });
    let sr = sample_rate as f64;
    let target = sr * latency.buffer_ms() / 1000.0;
    let strip = Box::new(Strip {
        ctl: ctl.clone(),
        rx,
        asrc: Asrc::new(ch, MAX_BLOCK),
        eq: EqChain::new(ch, MAX_BLOCK),
        buf: vec![0.0; MAX_BLOCK * ch],
        ch,
        sr,
        gain: 0.0,
        target,
        fill_avg: target,
        integ: 0.0,
        priming: true,
    });
    (ctl, tx, strip)
}

const KP: f64 = 0.1;
const KI: f64 = 0.02;
const MAX_DEV: f64 = 0.004;

impl Strip {
    pub fn key(&self) -> &str {
        &self.ctl.key
    }

    fn process_add(&mut self, mix: &mut [f32], frames: usize) {
        if let Some(p) = self.ctl.eq.take() {
            self.eq.set_params(p);
        }
        let ch = self.ch;
        let avail = self.rx.available() as f64;
        let need = self.asrc.frames_needed(frames) as f64;

        if self.priming {
            if avail >= self.target + need {
                self.priming = false;
                self.fill_avg = avail;
                self.integ = 0.0;
            } else {
                self.ctl.fill_ms.store((avail / self.sr * 1000.0) as f32);
                return;
            }
        }
        if avail < need {
            self.ctl.underruns.fetch_add(1, Ordering::Relaxed);
            self.priming = true;
            // Resume from silence with a ramp instead of a click.
            self.gain = 0.0;
            return;
        }
        if avail > self.target * 3.0 + need {
            let excess = (avail - self.target) as usize;
            self.rx.skip(excess);
            self.ctl.resyncs.fetch_add(1, Ordering::Relaxed);
            self.fill_avg = self.target;
            self.integ = 0.0;
            // The read position jumped: fade back in over this block.
            self.gain = 0.0;
        }

        // Drift control: hold the ring near the target fill by nudging the ratio.
        let fill = self.rx.available() as f64;
        self.fill_avg += 0.05 * (fill - self.fill_avg);
        let err = (self.fill_avg - self.target) / self.sr;
        let dt = frames as f64 / self.sr;
        self.integ = (self.integ + err * dt).clamp(-MAX_DEV / KI, MAX_DEV / KI);
        let dev = (KP * err + KI * self.integ).clamp(-MAX_DEV, MAX_DEV);
        self.asrc.set_ratio(1.0 + dev);
        self.ctl.ratio_ppm.store((dev * 1e6) as f32);
        self.ctl.fill_ms.store((fill / self.sr * 1000.0) as f32);

        let n = frames * ch;
        let rx = &mut self.rx;
        self.asrc.process(&mut self.buf[..n], frames, |dst, want| {
            rx.pop_into(dst, want);
        });
        self.eq.process(&mut self.buf[..n], frames);

        let target_gain = if self.ctl.muted.load(Ordering::Relaxed) { 0.0 } else { self.ctl.gain.load().clamp(0.0, 4.0) };
        let g0 = self.gain;
        let step = (target_gain - g0) / frames as f32;
        let mut peaks = [0.0f32; MAX_METER_CH];
        let cm = self.ctl.ch_mute.load(Ordering::Relaxed);
        for f in 0..frames {
            let g = g0 + step * (f + 1) as f32;
            for c in 0..ch {
                let i = f * ch + c;
                let v = if c < 32 && cm & (1 << c) != 0 { 0.0 } else { self.buf[i] * g };
                mix[i] += v;
                if c < MAX_METER_CH {
                    peaks[c] = peaks[c].max(v.abs());
                }
            }
        }
        self.gain = target_gain;
        for c in 0..ch.min(MAX_METER_CH) {
            self.ctl.peaks[c].fetch_max(peaks[c]);
        }
    }
}

enum BusCmd {
    Add(Box<Strip>),
    Remove(String),
}

/// Shared between the control thread and a device render thread.
pub struct BusControls {
    pub device_id: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub eq: Mailbox<EqParams>,
    pub peaks: [AtomicF32; MAX_METER_CH],
    pub strips: AtomicU32,
    pub alive: AtomicBool,
    pub output_latency_ms: AtomicF32,
    pub limiting: AtomicF32,
    pub error: Mutex<Option<String>>,
    pending: Mutex<Vec<BusCmd>>,
}

impl BusControls {
    pub fn new(device_id: &str, sample_rate: u32, channels: usize) -> Arc<BusControls> {
        Arc::new(BusControls {
            device_id: device_id.to_string(),
            sample_rate,
            channels: channels.max(1),
            eq: Mailbox::default(),
            peaks: Default::default(),
            strips: AtomicU32::new(0),
            alive: AtomicBool::new(false),
            output_latency_ms: AtomicF32::new(0.0),
            limiting: AtomicF32::new(1.0),
            error: Mutex::new(None),
            pending: Mutex::new(Vec::new()),
        })
    }

    pub fn add_strip(&self, s: Box<Strip>) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).push(BusCmd::Add(s));
    }

    pub fn remove_strip(&self, key: &str) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).push(BusCmd::Remove(key.to_string()));
    }

    pub fn take_peaks(&self) -> Vec<f32> {
        (0..self.channels.min(MAX_METER_CH)).map(|c| self.peaks[c].take()).collect()
    }
}

pub struct BusProcessor {
    ctl: Arc<BusControls>,
    strips: Vec<Box<Strip>>,
    eq: EqChain,
    limiter: Limiter,
    ch: usize,
}

impl BusProcessor {
    pub fn new(ctl: Arc<BusControls>) -> BusProcessor {
        let ch = ctl.channels;
        let sr = ctl.sample_rate as f64;
        BusProcessor { strips: Vec::with_capacity(32), eq: EqChain::new(ch, MAX_BLOCK), limiter: Limiter::new(sr, -0.3, 120.0), ch, ctl }
    }

    pub fn controls(&self) -> &Arc<BusControls> {
        &self.ctl
    }

    /// Fill `out` (interleaved, bus channel count) with `frames` frames.
    pub fn render(&mut self, out: &mut [f32], frames: usize) {
        if let Ok(mut q) = self.ctl.pending.try_lock() {
            for cmd in q.drain(..) {
                match cmd {
                    BusCmd::Add(s) => {
                        let k = s.key().to_string();
                        self.strips.retain(|x| x.key() != k);
                        self.strips.push(s);
                    }
                    BusCmd::Remove(k) => self.strips.retain(|x| x.key() != k),
                }
            }
            self.ctl.strips.store(self.strips.len() as u32, Ordering::Relaxed);
        }
        if let Some(p) = self.ctl.eq.take() {
            self.eq.set_params(p);
        }
        let ch = self.ch;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK);
            let slice = &mut out[done * ch..(done + n) * ch];
            slice.fill(0.0);
            for s in self.strips.iter_mut() {
                s.process_add(slice, n);
            }
            self.eq.process(slice, n);
            self.limiter.process(slice, n, ch);
            let mut peaks = [0.0f32; MAX_METER_CH];
            for f in 0..n {
                for c in 0..ch.min(MAX_METER_CH) {
                    peaks[c] = peaks[c].max(slice[f * ch + c].abs());
                }
            }
            for c in 0..ch.min(MAX_METER_CH) {
                self.ctl.peaks[c].fetch_max(peaks[c]);
            }
            done += n;
        }
        self.ctl.limiting.store(self.limiter.gain());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Capture runs 0.03% fast relative to render for 20 simulated seconds.
    /// The strip must keep the buffer near target with no underruns or resyncs.
    #[test]
    fn drift_is_absorbed_without_glitches() {
        let sr = 48_000u32;
        let bus = BusControls::new("dev", sr, 2);
        let mut proc_ = BusProcessor::new(bus.clone());
        let (ctl, mut tx, strip) = new_strip("app", sr, 2, LatencyMode::Balanced);
        ctl.gain.store(1.0);
        bus.add_strip(strip);
        let capture_rate = sr as f64 * 1.0003;
        let mut cap_acc = 0.0f64;
        let mut phase = 0.0f64;
        let mut out = vec![0.0f32; 480 * 2];
        let mut max_err_ms = 0.0f32;
        for blk in 0..2000 {
            cap_acc += capture_rate * 0.01;
            let frames = cap_acc.floor() as usize;
            cap_acc -= frames as f64;
            tx.push_with(frames, |i| {
                if i % 2 == 0 {
                    phase += 2.0 * std::f64::consts::PI * 440.0 / sr as f64;
                }
                (phase.sin() * 0.5) as f32
            });
            proc_.render(&mut out, 480);
            if blk > 600 {
                let fill = ctl.fill_ms.load();
                max_err_ms = max_err_ms.max((fill - 35.0).abs());
            }
        }
        assert_eq!(ctl.underruns.load(Ordering::Relaxed), 0, "underruns");
        assert_eq!(ctl.resyncs.load(Ordering::Relaxed), 0, "resyncs");
        assert!(max_err_ms < 12.0, "fill wandered {max_err_ms} ms");
        let ppm = ctl.ratio_ppm.load();
        assert!((ppm - 300.0).abs() < 120.0, "ratio settled at {ppm} ppm");
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.4 && peak < 0.55, "audio level {peak}");
    }

    #[test]
    fn mute_ramps_to_silence() {
        let sr = 48_000u32;
        let bus = BusControls::new("dev", sr, 2);
        let mut p = BusProcessor::new(bus.clone());
        let (ctl, mut tx, strip) = new_strip("a", sr, 2, LatencyMode::Low);
        ctl.gain.store(1.0);
        bus.add_strip(strip);
        let mut out = vec![0.0f32; 960];
        for i in 0..200 {
            tx.push_with(480, |_| 0.25);
            if i == 100 {
                ctl.muted.store(true, Ordering::Relaxed);
            }
            p.render(&mut out, 480);
        }
        assert!(out.iter().all(|v| v.abs() < 1e-6));
    }
}

#[cfg(test)]
mod jitter_tests {
    use super::*;

    /// Irregular render callbacks (like real WASAPI/Wine) must not distort the signal.
    #[test]
    fn irregular_blocks_keep_amplitude() {
        let sr = 48_000u32;
        let bus = BusControls::new("dev", sr, 2);
        let mut p = BusProcessor::new(bus.clone());
        let (ctl, mut tx, strip) = new_strip("a", sr, 2, LatencyMode::Balanced);
        ctl.gain.store(1.0);
        bus.add_strip(strip);
        let mut ph = 0.0f64;
        let mut cap_acc = 0.0f64;
        let sizes = [480usize, 960, 441, 7, 1024, 480, 300, 660];
        let mut max_peak = 0.0f32;
        let mut t_render = 0.0f64;
        let mut out = vec![0.0f32; 2048 * 2];
        for i in 0..6000 {
            let n = sizes[i % sizes.len()];
            t_render += n as f64 / sr as f64;
            // capture delivers in 10 ms packets at a 400 ppm slower clock
            while cap_acc < t_render * sr as f64 * 0.9996 + 960.0 {
                tx.push_with(480, |k| {
                    if k % 2 == 0 {
                        ph += 2.0 * std::f64::consts::PI * 440.0 / sr as f64;
                    }
                    (ph.sin() * 0.2) as f32
                });
                cap_acc += 480.0;
            }
            p.render(&mut out[..n * 2], n);
            if i > 200 {
                max_peak = max_peak.max(out[..n * 2].iter().fold(0.0f32, |m, v| m.max(v.abs())));
            }
        }
        assert!(max_peak < 0.205, "overshoot {max_peak}");
        assert!(max_peak > 0.19, "level {max_peak}");
        assert_eq!(ctl.underruns.load(Ordering::Relaxed), 0);
    }
}
