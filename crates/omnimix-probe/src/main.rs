//! omnimix-probe
//!
//!   omnimix-probe list       every output, how it is connected, every app playing
//!   omnimix-probe selftest   proves the per-app EQ path on this PC, end to end
//!   omnimix-probe json       same as list, machine readable

use omnimix_core::backend::AudioBackend;

fn print_list(b: &mut dyn AudioBackend) {
    let p = b.platform();
    println!("OmniMix probe {}  |  {}  |  {}", omnimix_core::VERSION, p.os, p.backend);
    println!("Accent: {:?} (dark UI {:?})\n", p.accent, p.accent_dark_ui);
    let dsp = b.dsp_support();
    println!("Per-app/device EQ: {}  [{}]", if dsp.available { "available" } else { "unavailable" }, dsp.method);
    if let Some(r) = dsp.reason {
        println!("  reason: {r}");
    }
    let snap = match b.refresh() {
        Ok(s) => s,
        Err(e) => {
            println!("refresh failed: {e}");
            return;
        }
    };
    println!("\nOUTPUTS ({})", snap.devices.len());
    for d in &snap.devices {
        let fmt = d.format.as_ref().map(|f| format!("{} Hz {}-bit {}ch", f.sample_rate, f.bits, f.channels)).unwrap_or_default();
        println!(
            "  {} {:<9} {:<46} {:?} {}{}",
            if d.is_default { "*" } else { " " },
            d.connection.kind.label(),
            d.name,
            d.state,
            fmt,
            if d.muted { "  MUTED" } else { "" }
        );
        if !d.connection.detail.is_empty() || d.connection.bus.is_some() {
            println!("      {}   bus={}", d.connection.detail.join(" · "), d.connection.bus.clone().unwrap_or_default());
        }
        if !d.channels.is_empty() {
            let ch: Vec<String> = d.channels.iter().map(|c| format!("{}={:.0}%", c.label, c.volume * 100.0)).collect();
            println!("      vol {:.0}%  channels {}", d.volume * 100.0, ch.join(" "));
        }
        println!("      id {}", d.id);
    }
    println!("\nAPPS ({})", snap.sources.len());
    for s in &snap.sources {
        let dev = snap.devices.iter().find(|d| d.id == s.device_id).map(|d| d.name.clone()).unwrap_or_default();
        println!(
            "  {:<28} pid {:<6} vol {:>3.0}%{} {}  -> {}",
            s.name,
            s.pid,
            s.volume * 100.0,
            if s.muted { " MUTED" } else { "" },
            if s.active { "playing" } else { "idle" },
            dev
        );
    }
}

#[cfg(windows)]
mod win {
    use std::f64::consts::PI;
    use std::process::{Child, Command};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use omnimix_core::backend::{AudioBackend, TapRequest};
    use omnimix_core::engine::ring::Consumer;
    use omnimix_core::engine::{new_strip, LatencyMode};
    use omnimix_core::model::DeviceState;
    use omnimix_windows::policy::AppRouting;
    use omnimix_windows::streams::{float_format, start_tap};
    use omnimix_windows::util::com_init;
    use omnimix_windows::WindowsBackend;
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    /// Child mode: play a 440 Hz tone at -12 dBFS on the default output.
    pub fn tone(seconds: f64) {
        com_init();
        unsafe {
            let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).unwrap();
            let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).unwrap();
            let rate = 48_000u32;
            let fmt = float_format(rate, 2);
            client
                .Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, 200_000, 0, &fmt.Format, None)
                .unwrap();
            let ev = CreateEventW(None, false, false, None).unwrap();
            client.SetEventHandle(ev).unwrap();
            let total = client.GetBufferSize().unwrap();
            let render: IAudioRenderClient = client.GetService().unwrap();
            client.Start().unwrap();
            let mut ph = 0.0f64;
            let start = Instant::now();
            while start.elapsed().as_secs_f64() < seconds {
                if WaitForSingleObject(ev, 200) != WAIT_OBJECT_0 {
                    continue;
                }
                let pad = client.GetCurrentPadding().unwrap_or(total);
                let n = total - pad;
                if n == 0 {
                    continue;
                }
                let p = render.GetBuffer(n).unwrap() as *mut f32;
                let out = std::slice::from_raw_parts_mut(p, n as usize * 2);
                for f in 0..n as usize {
                    let v = (ph.sin() * 0.25) as f32;
                    ph += 2.0 * PI * 440.0 / rate as f64;
                    out[f * 2] = v;
                    out[f * 2 + 1] = v;
                }
                render.ReleaseBuffer(n, 0).unwrap();
            }
            let _ = client.Stop();
        }
    }

    fn rms_dbfs(rx: &mut Consumer, secs: f64) -> f64 {
        let t0 = Instant::now();
        let (mut sum, mut n) = (0.0f64, 0usize);
        let mut buf = Vec::with_capacity(48_000);
        while t0.elapsed().as_secs_f64() < secs {
            std::thread::sleep(Duration::from_millis(20));
            buf.clear();
            let avail = rx.available();
            rx.pop_into(&mut buf, avail);
            for v in &buf {
                sum += (*v as f64) * (*v as f64);
                n += 1;
            }
        }
        if n == 0 {
            return -200.0;
        }
        10.0 * (sum / n as f64).max(1e-20).log10()
    }

    struct Kill(Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
        }
    }

    fn line(ok: bool, what: &str, detail: String) {
        println!("  [{}] {what}: {detail}", if ok { "PASS" } else { "FAIL" });
    }

    pub fn selftest() -> i32 {
        com_init();
        println!("OmniMix self-test: plays a quiet 440 Hz tone for ~12 s from a helper process.\n");
        let mut b = match WindowsBackend::new(None) {
            Ok(b) => b,
            Err(e) => {
                println!("backend failed: {e}");
                return 2;
            }
        };
        let dsp = b.dsp_support();
        line(dsp.available, "Windows support", dsp.reason.clone().unwrap_or_else(|| format!("build {}", b.build())));
        let me = std::env::current_exe().unwrap();
        let child = match Command::new(me).args(["tone", "14"]).spawn() {
            Ok(c) => Kill(c),
            Err(e) => {
                println!("could not start tone helper: {e}");
                return 2;
            }
        };
        let pid = child.0.id();
        std::thread::sleep(Duration::from_millis(1500));
        let snap = b.refresh().unwrap_or_default();
        let session = snap.sources.iter().find(|s| s.pid == pid).cloned();
        line(session.is_some(), "Helper session visible", format!("pid {pid}"));
        let Some(session) = session else { return 1 };
        let target = snap.devices.iter().find(|d| d.id == session.device_id).cloned().unwrap();
        let rate = target.format.as_ref().map(|f| f.sample_rate).unwrap_or(48_000);

        let (ctl, producer, _strip) = new_strip("probe", rate, 2, LatencyMode::Balanced);
        let (tap_tx, mut rx) = omnimix_core::engine::ring::ring(rate as usize * 4, 2);
        drop(producer);
        let tap = start_tap(pid, TapRequest { producer: tap_tx, strip: ctl.clone(), sample_rate: rate, channels: 2 });
        line(tap.is_ok(), "Process loopback capture", tap.as_ref().err().cloned().unwrap_or_else(|| "running".into()));
        let Ok(_tap) = tap else { return 1 };
        std::thread::sleep(Duration::from_millis(300));
        let base = rms_dbfs(&mut rx, 1.0);
        line(base > -30.0, "Tap level (expect about -15 dBFS)", format!("{base:.1} dBFS"));

        // Does the per-app session mute reach the tap?
        let _ = b.set_source_mute(&session.id, true);
        std::thread::sleep(Duration::from_millis(300));
        let _ = rx.available();
        let muted = rms_dbfs(&mut rx, 0.8);
        let _ = b.set_source_mute(&session.id, false);
        println!("  [INFO] Session mute is {} the tap ({muted:.1} dBFS while muted)", if muted < base - 20.0 { "before" } else { "after" });

        // The real mechanism: park on a muted sink, tap must stay at full level.
        let sink = snap.devices.iter().find(|d| d.state == DeviceState::Active && d.id != target.id).cloned();
        line(sink.is_some(), "Spare output for parking", sink.as_ref().map(|d| d.name.clone()).unwrap_or_else(|| "none (install VB-CABLE or enable another output)".into()));
        let mut code = if base > -30.0 { 0 } else { 1 };
        if let Some(sink) = sink {
            let routing = AppRouting::new();
            line(routing.is_ok(), "Per-app routing interface", routing.as_ref().err().map(|e| e.message()).unwrap_or_else(|| "ok".into()));
            if let Ok(routing) = routing {
                let prev = routing.get(pid).ok().flatten();
                let was_muted = sink.muted;
                let _ = b.set_device_mute(&sink.id, true);
                let r = routing.set(pid, Some(&sink.id));
                std::thread::sleep(Duration::from_millis(1200));
                let snap2 = b.refresh().unwrap_or_default();
                let moved = snap2.sources.iter().any(|s| s.pid == pid && s.device_id == sink.id);
                line(r.is_ok() && moved, "App moved to parking output", if moved { "yes".into() } else { format!("no ({r:?})") });
                let parked = rms_dbfs(&mut rx, 1.0);
                let ok = parked > base - 3.0;
                line(ok, "Tap level while parked on muted output", format!("{parked:.1} dBFS (base {base:.1})"));
                let _ = routing.set(pid, prev.as_deref());
                let _ = b.set_device_mute(&sink.id, was_muted);
                if !(ok && moved) {
                    code = 1;
                }
            } else {
                code = 1;
            }
        }
        ctl.capture_alive.load(Ordering::Relaxed);
        drop(child);
        b.shutdown();
        println!("\nVerdict: {}", if code == 0 { "per-app EQ path WORKS on this PC" } else { "per-app EQ path NOT confirmed (see FAIL lines)" });
        code
    }

    /// Render the engine to the default output for 3 s with an in-process test signal.
    pub fn enginetest() -> i32 {
        com_init();
        let mut b = match WindowsBackend::new(None) {
            Ok(b) => b,
            Err(e) => {
                println!("backend failed: {e}");
                return 2;
            }
        };
        let snap = b.refresh().unwrap_or_default();
        let Some(dev) = snap.devices.iter().find(|d| d.is_default).cloned() else {
            println!("no default output");
            return 1;
        };
        let bus = match b.open_output(&dev, LatencyMode::Balanced) {
            Ok(x) => x,
            Err(e) => {
                println!("open_output failed: {e}");
                return 1;
            }
        };
        let (ctl, mut tx, strip) = new_strip("test", bus.sample_rate, 2, LatencyMode::Balanced);
        ctl.gain.store(1.0);
        bus.add_strip(strip);
        let rate = bus.sample_rate as f64;
        let mut ph = 0.0f64;
        let t0 = Instant::now();
        let mut sent = 0u64;
        while t0.elapsed().as_secs_f64() < 3.0 {
            let due = (t0.elapsed().as_secs_f64() * rate) as u64;
            let n = (due.saturating_sub(sent)) as usize;
            tx.push_with(n, |i| {
                if i % 2 == 0 {
                    ph += 2.0 * PI * 440.0 / rate;
                }
                (ph.sin() * 0.2) as f32
            });
            sent += n as u64;
            std::thread::sleep(Duration::from_millis(10));
        }
        let peaks = bus.take_peaks();
        let ok = bus.alive.load(Ordering::Relaxed) && peaks.iter().any(|p| *p > 0.1);
        println!(
            "output {} @ {} Hz: alive={} peaks={:?} underruns={} resyncs={} fill={:.1} ms ratio={:.0} ppm latency={:.1} ms",
            dev.name,
            bus.sample_rate,
            bus.alive.load(Ordering::Relaxed),
            peaks,
            ctl.underruns.load(Ordering::Relaxed),
            ctl.resyncs.load(Ordering::Relaxed),
            ctl.fill_ms.load(),
            ctl.ratio_ppm.load(),
            bus.output_latency_ms.load()
        );
        b.close_output(&dev.id);
        println!("engine test: {}", if ok { "PASS" } else { "FAIL" });
        if ok { 0 } else { 1 }
    }

    pub fn backend() -> Box<dyn AudioBackend> {
        Box::new(WindowsBackend::new(None).expect("Windows audio backend"))
    }

    pub fn facts() {
        com_init();
        let b = WindowsBackend::new(None).expect("backend");
        for id in &b.table().order {
            if let Some(e) = b.table().endpoints.get(id) {
                println!("{}\n  {:?}\n", e.info.name, e.info.facts);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("list");
    #[cfg(windows)]
    {
        match cmd {
            "tone" => win::tone(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10.0)),
            "selftest" => std::process::exit(win::selftest()),
            "facts" => win::facts(),
            "enginetest" => std::process::exit(win::enginetest()),
            "json" => {
                let mut b = win::backend();
                let s = b.refresh().unwrap_or_default();
                println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
            }
            _ => print_list(win::backend().as_mut()),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut b: Box<dyn AudioBackend> = Box::new(omnimix_macos::MacBackend::new().expect("Core Audio backend"));
        match cmd {
            "json" => println!("{}", serde_json::to_string_pretty(&b.refresh().unwrap_or_default()).unwrap_or_default()),
            "uistate" => {
                let mut m = omnimix_core::mixer::Mixer::new(b, omnimix_core::state::StateStore::memory());
                println!("{}", serde_json::to_string_pretty(&m.ui_state(true)).unwrap_or_default());
                m.shutdown();
            }
            _ => print_list(b.as_mut()),
        }
        return;
    }
    #[allow(unreachable_code)]
    if cmd == "uistate" {
        #[cfg(windows)]
        let b = win::backend();
        #[cfg(not(windows))]
        let b: Box<dyn AudioBackend> = Box::new(omnimix_core::mock::MockBackend::new());
        let mut m = omnimix_core::mixer::Mixer::new(b, omnimix_core::state::StateStore::memory());
        println!("{}", serde_json::to_string_pretty(&m.ui_state(true)).unwrap_or_default());
        m.shutdown();
        return;
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let mut b = omnimix_core::mock::MockBackend::new();
        match cmd {
            "json" => println!("{}", serde_json::to_string_pretty(&b.refresh().unwrap_or_default()).unwrap_or_default()),
            _ => {
                println!("(no native backend on this OS yet: showing the demo rig)\n");
                print_list(&mut b);
            }
        }
    }
}
