//! Demo backend: a realistic simulated rig (USB-C, USB, Bluetooth, AUX, HDMI,
//! S/PDIF, virtual) with animated meters. Diverted sources run through the
//! real engine (synthetic drum-and-bass signal, drifting capture clock), so
//! EQ moves the meters exactly as it would on hardware. Used on platforms
//! without a native backend yet and for UI development / tests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::backend::{AudioBackend, BResult, BackendError, BackendEvent, DspSupport, PlatformInfo, TapRequest};
use crate::engine::{BusControls, BusProcessor, LatencyMode};
use crate::model::*;

struct Worker {
    stop: Arc<AtomicBool>,
    jh: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn<F: FnOnce(Arc<AtomicBool>) + Send + 'static>(name: &str, f: F) -> Worker {
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let jh = std::thread::Builder::new().name(name.into()).spawn(move || f(s2)).ok();
        Worker { stop, jh }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.jh.take() {
            let _ = j.join();
        }
    }
}

pub struct MockBackend {
    devices: Vec<Device>,
    sources: Vec<Source>,
    t0: Instant,
    outputs: HashMap<String, Worker>,
    taps: HashMap<u32, Worker>,
    events: Vec<BackendEvent>,
    soft: bool,
}

fn ch(labels: &[&str]) -> Vec<Channel> {
    labels.iter().enumerate().map(|(i, l)| Channel { index: i as u16, label: (*l).into(), volume: 1.0, muted: false }).collect()
}

#[allow(clippy::too_many_arguments)]
fn dev(id: &str, label: &str, adapter: &str, ff: FormFactor, kind: ConnectionKind, detail: &[&str], state: DeviceState, rate: u32, bits: u16, chans: &[&str]) -> Device {
    Device {
        id: id.into(),
        name: format!("{label} ({adapter})"),
        label: label.into(),
        adapter: adapter.into(),
        form_factor: ff,
        connection: ConnectionInfo { kind, detail: detail.iter().map(|s| s.to_string()).collect(), bus: None, usb: None, jack_color: None },
        state,
        is_default: false,
        is_default_comms: false,
        format: Some(StreamFormat { sample_rate: rate, channels: chans.len() as u16, bits, channel_mask: 0 }),
        volume: 0.62,
        muted: false,
        channels: ch(chans),
    }
}

fn src(pid: u32, key: &str, name: &str, device: &str, active: bool, vol: f32) -> Source {
    Source {
        id: format!("mock-session-{pid}"),
        key: key.into(),
        pid,
        name: name.into(),
        exe: Some(format!("C:\\Program Files\\{name}\\{key}")),
        icon: None,
        device_id: device.into(),
        active,
        system: key == "system",
        volume: vol,
        muted: false,
        channels: ch(&["L", "R"]),
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBackend {
    pub fn new() -> MockBackend {
        let mut devices = vec![
            dev("mock:usbc", "Headphones", "Studio Buds USB-C", FormFactor::Headphones, ConnectionKind::UsbC, &["Port 2", "USB 2.0 Hi-Speed"], DeviceState::Active, 48_000, 24, &["L", "R"]),
            dev("mock:bt", "Headphones", "WH Wireless", FormFactor::Headphones, ConnectionKind::Bluetooth, &["A2DP stereo"], DeviceState::Active, 48_000, 16, &["L", "R"]),
            dev("mock:aux", "Speakers", "Realtek(R) Audio", FormFactor::Speakers, ConnectionKind::Aux, &["3.5 mm", "Rear panel"], DeviceState::Active, 48_000, 24, &["L", "R", "C", "LFE", "SL", "SR"]),
            dev("mock:usb", "Speakers", "Scarlett USB", FormFactor::LineOut, ConnectionKind::Usb, &["Port 5", "USB 2.0 Hi-Speed"], DeviceState::Active, 96_000, 24, &["L", "R"]),
            dev("mock:hdmi", "LG ULTRAGEAR", "NVIDIA High Definition Audio", FormFactor::Display, ConnectionKind::Display, &["HDMI"], DeviceState::Active, 48_000, 24, &["L", "R"]),
            dev("mock:cable", "CABLE Input", "VB-Audio Virtual Cable", FormFactor::Speakers, ConnectionKind::Virtual, &["Virtual"], DeviceState::Active, 48_000, 24, &["L", "R"]),
            dev("mock:front", "Headphones", "Realtek(R) Audio", FormFactor::Headphones, ConnectionKind::Aux, &["3.5 mm", "Front panel"], DeviceState::Unplugged, 48_000, 24, &["L", "R"]),
            dev("mock:spdif", "Digital Output", "Realtek(R) Audio", FormFactor::Spdif, ConnectionKind::Spdif, &["Optical"], DeviceState::Disabled, 48_000, 24, &["L", "R"]),
        ];
        devices[0].is_default = true;
        devices[0].volume = 0.48;
        devices[1].is_default_comms = true;
        devices[2].connection.jack_color = Some(0x00A0_3C);
        let sources = vec![
            src(4120, "spotify.exe", "Spotify", "mock:usbc", true, 0.85),
            src(5208, "chrome.exe", "Google Chrome", "mock:usbc", true, 0.7),
            src(6312, "discord.exe", "Discord", "mock:bt", true, 0.9),
            src(7400, "steam.exe", "Steam", "mock:aux", false, 1.0),
            src(8216, "vlc.exe", "VLC media player", "mock:aux", true, 0.66),
            src(0, "system", "System sounds", "mock:usbc", false, 0.5),
        ];
        MockBackend { devices, sources, t0: Instant::now(), outputs: HashMap::new(), taps: HashMap::new(), events: vec![], soft: false }
    }

    /// Behave like an OS without a per-app mixer (macOS): app volume goes through the engine.
    pub fn soft_mixer() -> MockBackend {
        MockBackend { soft: true, ..MockBackend::new() }
    }

    fn device_mut(&mut self, id: &str) -> BResult<&mut Device> {
        self.devices.iter_mut().find(|d| d.id == id).ok_or_else(|| BackendError(format!("unknown device {id}")))
    }

    fn source_mut(&mut self, id: &str) -> BResult<&mut Source> {
        self.sources.iter_mut().find(|s| s.id == id).ok_or_else(|| BackendError(format!("unknown source {id}")))
    }

    fn envelope(&self, seed: u32, t: f64) -> f32 {
        let s = seed as f64 * 0.37;
        let beat = ((t * 2.1 + s).fract() * -6.0).exp();
        let wobble = 0.5 + 0.5 * (t * 1.3 + s).sin() * (t * 0.37 + s * 2.0).cos();
        let speechy = if (t * 0.6 + s).sin() > 0.1 { 1.0 } else { 0.15 };
        let v = match seed % 4 {
            0 => 0.55 + 0.35 * beat,
            1 => 0.35 * wobble * speechy + 0.08,
            2 => 0.5 * speechy * (0.6 + 0.4 * wobble),
            _ => 0.3 + 0.4 * wobble * beat,
        };
        v.clamp(0.0, 1.0) as f32
    }
}

impl AudioBackend for MockBackend {
    fn platform(&self) -> PlatformInfo {
        PlatformInfo {
            os: format!("Demo rig on {}", std::env::consts::OS),
            backend: "Simulated (demo)".into(),
            accent: Some("#0078D4".into()),
            accent_dark_ui: Some("#60CDFF".into()),
            dark_mode: true,
        }
    }

    fn self_pid(&self) -> u32 {
        1
    }

    fn refresh(&mut self) -> BResult<Snapshot> {
        Ok(Snapshot { devices: self.devices.clone(), sources: self.sources.clone() })
    }

    fn levels(&mut self, devices: &[String], sources: &[String]) -> Levels {
        let t = self.t0.elapsed().as_secs_f64();
        let mut out = Levels::default();
        let mut per_dev: HashMap<String, f32> = HashMap::new();
        for s in &self.sources {
            if !sources.contains(&s.id) {
                continue;
            }
            let base = if s.active && !s.muted { self.envelope(s.pid, t) * s.volume } else { 0.0 };
            let l = base * s.channels[0].volume;
            let r = base * s.channels[1].volume * (0.9 + 0.1 * (t * 3.0 + s.pid as f64).sin() as f32);
            out.sources.push((s.id.clone(), vec![l, r]));
            let e = per_dev.entry(s.device_id.clone()).or_default();
            *e = e.max(l.max(r));
        }
        for d in &self.devices {
            if !devices.contains(&d.id) {
                continue;
            }
            let lvl = if d.muted { 0.0 } else { per_dev.get(&d.id).copied().unwrap_or(0.0) * d.volume.sqrt() };
            let chans = d.channels.iter().enumerate().map(|(i, c)| lvl * c.volume * (0.85 + 0.15 * ((t * 2.0 + i as f64).sin() as f32))).collect();
            out.devices.push((d.id.clone(), chans));
        }
        out
    }

    fn poll_events(&mut self) -> Vec<BackendEvent> {
        std::mem::take(&mut self.events)
    }

    fn set_device_volume(&mut self, id: &str, volume: f32) -> BResult<()> {
        self.device_mut(id)?.volume = volume;
        Ok(())
    }

    fn set_device_mute(&mut self, id: &str, muted: bool) -> BResult<()> {
        self.device_mut(id)?.muted = muted;
        Ok(())
    }

    fn set_device_channel_volume(&mut self, id: &str, channel: u16, volume: f32) -> BResult<()> {
        let d = self.device_mut(id)?;
        let c = d.channels.iter_mut().find(|c| c.index == channel).ok_or("no such channel")?;
        c.volume = volume;
        Ok(())
    }

    fn set_default_device(&mut self, id: &str) -> BResult<()> {
        if !self.devices.iter().any(|d| d.id == id && d.state == DeviceState::Active) {
            return Err("device is not active".into());
        }
        let old = self.devices.iter().find(|d| d.is_default).map(|d| d.id.clone());
        for d in self.devices.iter_mut() {
            d.is_default = d.id == id;
        }
        for s in self.sources.iter_mut() {
            if Some(&s.device_id) == old.as_ref() {
                s.device_id = id.to_string();
            }
        }
        self.events.push(BackendEvent::DefaultChanged);
        Ok(())
    }

    fn set_source_volume(&mut self, id: &str, volume: f32) -> BResult<()> {
        self.source_mut(id)?.volume = volume;
        Ok(())
    }

    fn set_source_mute(&mut self, id: &str, muted: bool) -> BResult<()> {
        self.source_mut(id)?.muted = muted;
        Ok(())
    }

    fn set_source_channel_volume(&mut self, id: &str, channel: u16, volume: f32) -> BResult<()> {
        let s = self.source_mut(id)?;
        let c = s.channels.iter_mut().find(|c| c.index == channel).ok_or("no such channel")?;
        c.volume = volume;
        Ok(())
    }

    fn dsp_support(&mut self) -> DspSupport {
        DspSupport { available: true, reason: None, method: "Simulated engine (demo backend)".into(), needs_sink: false, soft_app_mixer: self.soft, hint: None }
    }

    fn open_output(&mut self, device: &Device, _latency: LatencyMode) -> BResult<Arc<BusControls>> {
        let rate = device.format.as_ref().map(|f| f.sample_rate).unwrap_or(48_000);
        let ctl = BusControls::new(&device.id, rate, 2);
        ctl.output_latency_ms.store(10.0);
        let c2 = ctl.clone();
        let w = Worker::spawn("mock-render", move |stop| {
            let mut p = BusProcessor::new(c2.clone());
            let frames = (rate / 100) as usize;
            let mut buf = vec![0.0f32; frames * 2];
            c2.alive.store(true, Ordering::Relaxed);
            let start = Instant::now();
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                p.render(&mut buf, frames);
                n += 1;
                let due = start + Duration::from_millis(10 * n);
                if let Some(d) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(d);
                }
            }
            c2.alive.store(false, Ordering::Relaxed);
        });
        self.outputs.insert(device.id.clone(), w);
        Ok(ctl)
    }

    fn close_output(&mut self, device_id: &str) {
        self.outputs.remove(device_id);
    }

    fn divert(&mut self, source: &Source, _target: &Device, _sink: Option<&Device>, tap: TapRequest) -> BResult<()> {
        let TapRequest { mut producer, strip, sample_rate, channels } = tap;
        let ch = channels as usize;
        let level = source.volume;
        let seed = source.pid;
        let w = Worker::spawn("mock-capture", move |stop| {
            strip.capture_alive.store(true, Ordering::Relaxed);
            let rate = sample_rate as f64 * 1.0002;
            let start = Instant::now();
            let mut produced = 0u64;
            let mut t = 0u64;
            let mut rng = 0x1234_5678u32 ^ seed;
            while !stop.load(Ordering::Relaxed) {
                let due = (start.elapsed().as_secs_f64() * rate) as u64;
                let n = due.saturating_sub(produced) as usize;
                producer.push_with(n, |i| {
                    if i % ch == 0 {
                        t += 1;
                    }
                    let ts = t as f64 / sample_rate as f64;
                    let beat = (ts * 2.0).fract();
                    let kick = (-beat * 18.0).exp() * (2.0 * std::f64::consts::PI * (55.0 + 90.0 * (-beat * 30.0).exp()) * ts).sin();
                    rng ^= rng << 13;
                    rng ^= rng >> 17;
                    rng ^= rng << 5;
                    let noise = (rng as f64 / u32::MAX as f64) * 2.0 - 1.0;
                    let hat_phase = (ts * 8.0).fract();
                    let hats = noise * (-hat_phase * 40.0).exp() * 0.25;
                    let clap_phase = ((ts * 2.0) + 0.5).fract();
                    let clap = noise * (-clap_phase * 25.0).exp() * 0.3;
                    let bass = (2.0 * std::f64::consts::PI * 41.2 * ts).sin() * 0.25;
                    ((kick * 0.45 + hats + clap + bass) * level as f64 * 0.8) as f32
                });
                produced += n as u64;
                std::thread::sleep(Duration::from_millis(10));
            }
            strip.capture_alive.store(false, Ordering::Relaxed);
        });
        self.taps.insert(source.pid, w);
        Ok(())
    }

    fn release(&mut self, pid: u32) -> BResult<()> {
        self.taps.remove(&pid);
        Ok(())
    }

    fn shutdown(&mut self) {
        self.taps.clear();
        self.outputs.clear();
    }
}
