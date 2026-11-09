//! `AudioBackend` implementation for Windows 10/11.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use omnimix_core::backend::{AudioBackend, BResult, BackendError, BackendEvent, DspSupport, PlatformInfo, TapRequest};
use omnimix_core::engine::{BusControls, LatencyMode};
use omnimix_core::model::{Device, DeviceState, Levels, Snapshot, Source};
use windows::core::{implement, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use crate::devices::{default_id, DeviceTable};
use crate::journal::{Journal, RouteEntry, SinkEntry};
use crate::policy::{self, AppRouting};
use crate::sessions::{self, ProcCache, Session};
use crate::streams::{self, Worker};
use crate::system;
use crate::util::{com_init, Ctx, WStr};

/// Minimum build with working process loopback (Windows 10 2004).
pub const MIN_DSP_BUILD: u32 = 19041;

#[derive(Default)]
struct NotifyState {
    devices: bool,
    default: bool,
    invalid: HashSet<String>,
}

#[implement(IMMNotificationClient)]
struct Notifier(Arc<Mutex<NotifyState>>);

impl Notifier_Impl {
    fn touch(&self, id: Option<&PCWSTR>, default: bool) {
        if let Ok(mut s) = self.0.lock() {
            s.devices = true;
            s.default |= default;
            if let Some(id) = id {
                if let Ok(v) = unsafe { id.to_string() } {
                    s.invalid.insert(v);
                }
            }
        }
    }
}

impl IMMNotificationClient_Impl for Notifier_Impl {
    fn OnDeviceStateChanged(&self, id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        self.touch(Some(id), false);
        Ok(())
    }
    fn OnDeviceAdded(&self, id: &PCWSTR) -> windows::core::Result<()> {
        self.touch(Some(id), false);
        Ok(())
    }
    fn OnDeviceRemoved(&self, id: &PCWSTR) -> windows::core::Result<()> {
        self.touch(Some(id), false);
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
        self.touch(None, true);
        Ok(())
    }
    fn OnPropertyValueChanged(&self, id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        self.touch(Some(id), false);
        Ok(())
    }
}

struct Tap {
    _worker: Worker,
    previous: Option<String>,
}

pub struct WindowsBackend {
    en: IMMDeviceEnumerator,
    table: DeviceTable,
    sessions: HashMap<String, Session>,
    procs: ProcCache,
    notifier: IMMNotificationClient,
    notify: Arc<Mutex<NotifyState>>,
    routing: Option<AppRouting>,
    routing_error: Option<String>,
    outputs: HashMap<String, Worker>,
    taps: HashMap<u32, Tap>,
    journal: Journal,
    build: u32,
    platform: PlatformInfo,
}

impl WindowsBackend {
    /// Create on the thread that will use it. `config_dir` holds the crash journal.
    pub fn new(config_dir: Option<PathBuf>) -> BResult<WindowsBackend> {
        com_init();
        let en: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.ctx("MMDeviceEnumerator")?;
        let notify = Arc::new(Mutex::new(NotifyState::default()));
        let notifier: IMMNotificationClient = Notifier(notify.clone()).into();
        unsafe {
            let _ = en.RegisterEndpointNotificationCallback(&notifier);
        }
        let (routing, routing_error) = match AppRouting::new() {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(format!("per-app routing unavailable: {}", e.message()))),
        };
        let (accent, accent_dark_ui) = system::accent_colors();
        let platform = PlatformInfo { os: system::os_string(), backend: "WASAPI / Core Audio".into(), accent, accent_dark_ui, dark_mode: system::dark_mode() };
        let mut b = WindowsBackend {
            en,
            table: DeviceTable::default(),
            sessions: HashMap::new(),
            procs: ProcCache::default(),
            notifier,
            notify,
            routing,
            routing_error,
            outputs: HashMap::new(),
            taps: HashMap::new(),
            journal: Journal::open(config_dir.map(|d| d.join("journal.json"))),
            build: system::build_number(),
            platform,
        };
        let _ = b.table.refresh(&b.en, &|_| false);
        b.restore_sink();
        Ok(b)
    }

    fn endpoint_volume(&self, id: &str) -> BResult<IAudioEndpointVolume> {
        if let Some(v) = self.table.endpoints.get(id).and_then(|e| e.volume.clone()) {
            return Ok(v);
        }
        unsafe {
            let dev = self.en.GetDevice(WStr::new(id).pcwstr()).ctx("open device")?;
            dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ctx("endpoint volume")
        }
    }

    fn session(&self, id: &str) -> BResult<&Session> {
        self.sessions.get(id).ok_or_else(|| BackendError("That app's audio session has ended".into()))
    }

    fn ensure_sink_muted(&mut self, sink_id: &str) -> BResult<()> {
        if let Some(s) = &self.journal.sink {
            if s.device_id == sink_id {
                let v = self.endpoint_volume(sink_id)?;
                unsafe { v.SetMute(true, std::ptr::null()) }.ctx("mute sink")?;
                return Ok(());
            }
            self.restore_sink();
        }
        let v = self.endpoint_volume(sink_id)?;
        let was_muted = unsafe { v.GetMute() }.map(|b| b.as_bool()).unwrap_or(false);
        self.journal.sink = Some(SinkEntry { device_id: sink_id.to_string(), was_muted });
        self.journal.save();
        unsafe { v.SetMute(true, std::ptr::null()) }.ctx("mute sink")?;
        Ok(())
    }

    fn restore_sink(&mut self) {
        if let Some(s) = self.journal.sink.take() {
            if let Ok(v) = self.endpoint_volume(&s.device_id) {
                unsafe {
                    let _ = v.SetMute(s.was_muted, std::ptr::null());
                }
            }
            self.journal.save();
        }
    }

    /// Put apps left on the sink by a previous crash back where they were.
    fn restore_orphans(&mut self) {
        if self.journal.routes.is_empty() {
            return;
        }
        let Some(routing) = &self.routing else { return };
        let mut fixed = vec![];
        for s in self.sessions.values() {
            let pid = s.source.pid;
            if self.taps.contains_key(&pid) {
                continue;
            }
            let Some(exe) = &s.source.exe else { continue };
            if let Some(e) = self.journal.routes.iter().find(|r| r.exe.eq_ignore_ascii_case(exe)) {
                if routing.set(pid, e.previous.as_deref()).is_ok() {
                    fixed.push(e.exe.clone());
                }
            }
        }
        if !fixed.is_empty() {
            self.journal.routes.retain(|r| !fixed.iter().any(|f| f.eq_ignore_ascii_case(&r.exe)) || self.taps.contains_key(&r.pid));
            self.journal.save();
        }
    }

    pub fn routing_error(&self) -> Option<&str> {
        self.routing_error.as_deref()
    }

    pub fn build(&self) -> u32 {
        self.build
    }

    /// Raw facts for diagnostics.
    pub fn table(&self) -> &DeviceTable {
        &self.table
    }
}

impl AudioBackend for WindowsBackend {
    fn platform(&self) -> PlatformInfo {
        self.platform.clone()
    }

    fn refresh(&mut self) -> BResult<Snapshot> {
        let invalid = {
            let mut n = self.notify.lock().unwrap_or_else(|e| e.into_inner());
            n.devices = false;
            n.default = false;
            std::mem::take(&mut n.invalid)
        };
        self.table.refresh(&self.en, &|id| invalid.contains(id)).ctx("enumerate outputs")?;
        let def = default_id(&self.en, eConsole);
        let comms = default_id(&self.en, eCommunications);
        let devices: Vec<Device> = self.table.order.iter().filter_map(|id| self.table.to_device(id, def.as_deref(), comms.as_deref())).collect();

        let mut map = HashMap::new();
        let mut sources: Vec<Source> = vec![];
        for id in &self.table.order {
            let Some(ep) = self.table.endpoints.get(id) else { continue };
            if ep.state != DeviceState::Active {
                continue;
            }
            for s in sessions::enumerate(&ep.device, id, &mut self.procs) {
                sources.push(s.source.clone());
                map.insert(s.source.id.clone(), s);
            }
        }
        self.sessions = map;
        self.restore_orphans();
        Ok(Snapshot { devices, sources })
    }

    fn levels(&mut self, devices: &[String], sources: &[String]) -> Levels {
        let mut out = Levels::default();
        let mut buf = [0f32; 16];
        unsafe {
            for id in devices {
                if let Some(m) = self.table.endpoints.get(id).and_then(|e| e.meter.as_ref()) {
                    let n = m.GetMeteringChannelCount().unwrap_or(0).min(16) as usize;
                    if n > 0 && m.GetChannelsPeakValues(&mut buf[..n]).is_ok() {
                        out.devices.push((id.clone(), buf[..n].to_vec()));
                    } else if let Ok(p) = m.GetPeakValue() {
                        out.devices.push((id.clone(), vec![p, p]));
                    }
                }
            }
            for id in sources {
                if let Some(m) = self.sessions.get(id).and_then(|s| s.meter.as_ref()) {
                    let n = m.GetMeteringChannelCount().unwrap_or(0).min(16) as usize;
                    if n > 0 && m.GetChannelsPeakValues(&mut buf[..n]).is_ok() {
                        out.sources.push((id.clone(), buf[..n].to_vec()));
                    } else if let Ok(p) = m.GetPeakValue() {
                        out.sources.push((id.clone(), vec![p, p]));
                    }
                }
            }
        }
        out
    }

    fn poll_events(&mut self) -> Vec<BackendEvent> {
        let n = self.notify.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = vec![];
        if n.devices {
            v.push(BackendEvent::DevicesChanged);
        }
        if n.default {
            v.push(BackendEvent::DefaultChanged);
        }
        v
    }

    fn set_device_volume(&mut self, id: &str, volume: f32) -> BResult<()> {
        let v = self.endpoint_volume(id)?;
        unsafe { v.SetMasterVolumeLevelScalar(volume.clamp(0.0, 1.0), std::ptr::null()) }.ctx("set volume")
    }

    fn set_device_mute(&mut self, id: &str, muted: bool) -> BResult<()> {
        let v = self.endpoint_volume(id)?;
        unsafe { v.SetMute(muted, std::ptr::null()) }.ctx("set mute")
    }

    fn set_device_channel_volume(&mut self, id: &str, channel: u16, volume: f32) -> BResult<()> {
        let v = self.endpoint_volume(id)?;
        unsafe { v.SetChannelVolumeLevelScalar(channel as u32, volume.clamp(0.0, 1.0), std::ptr::null()) }.ctx("set channel volume")
    }

    fn set_default_device(&mut self, id: &str) -> BResult<()> {
        policy::set_default_endpoint(id).ctx("set default output")
    }

    fn set_source_volume(&mut self, id: &str, volume: f32) -> BResult<()> {
        let s = self.session(id)?;
        unsafe { s.simple.SetMasterVolume(volume.clamp(0.0, 1.0), std::ptr::null()) }.ctx("set app volume")
    }

    fn set_source_mute(&mut self, id: &str, muted: bool) -> BResult<()> {
        let s = self.session(id)?;
        unsafe { s.simple.SetMute(muted, std::ptr::null()) }.ctx("set app mute")
    }

    fn set_source_channel_volume(&mut self, id: &str, channel: u16, volume: f32) -> BResult<()> {
        let s = self.session(id)?;
        let c = s.channels.as_ref().ok_or_else(|| BackendError("This app has no per-channel control".into()))?;
        unsafe { c.SetChannelVolume(channel as u32, volume.clamp(0.0, 1.0), std::ptr::null()) }.ctx("set app channel")
    }

    fn dsp_support(&mut self) -> DspSupport {
        let method = "Process loopback + per-app routing (no driver)".to_string();
        if self.build < MIN_DSP_BUILD {
            return DspSupport {
                available: false,
                reason: Some(format!("EQ needs Windows 10 version 2004 (build {MIN_DSP_BUILD}) or newer. This PC runs build {}. Volume, mute and channels still work.", self.build)),
                method,
                needs_sink: true,
                soft_app_mixer: false,
                hint: None,
            };
        }
        if self.routing.is_none() {
            return DspSupport { available: false, reason: self.routing_error.clone(), method, needs_sink: true, soft_app_mixer: false, hint: None };
        }
        DspSupport { available: true, reason: None, method, needs_sink: true, soft_app_mixer: false, hint: None }
    }

    fn open_output(&mut self, device: &Device, latency: LatencyMode) -> BResult<Arc<BusControls>> {
        self.outputs.remove(&device.id);
        let rate = device.format.as_ref().map(|f| f.sample_rate).filter(|r| *r >= 8000).unwrap_or(48_000);
        let ctl = BusControls::new(&device.id, rate, 2);
        let w = streams::start_output(&device.id, ctl.clone(), latency).map_err(BackendError)?;
        self.outputs.insert(device.id.clone(), w);
        Ok(ctl)
    }

    fn close_output(&mut self, device_id: &str) {
        self.outputs.remove(device_id);
    }

    fn divert(&mut self, source: &Source, _target: &Device, sink: Option<&Device>, tap: TapRequest) -> BResult<()> {
        let sink = sink.ok_or_else(|| BackendError("No parking output available".into()))?;
        let pid = source.pid;
        if self.taps.contains_key(&pid) {
            return Ok(());
        }
        if self.routing.is_none() {
            return Err(BackendError(self.routing_error.clone().unwrap_or_else(|| "per-app routing unavailable".into())));
        }
        let exe = source.exe.clone().unwrap_or_default();
        let pending = self.journal.routes.iter().position(|r| !exe.is_empty() && r.exe.eq_ignore_ascii_case(&exe) && !self.taps.contains_key(&r.pid));
        let mut previous = match pending {
            Some(i) => self.journal.routes.remove(i).previous,
            None => self.routing.as_ref().and_then(|r| r.get(pid).ok().flatten()),
        };
        if previous.as_deref() == Some(sink.id.as_str()) {
            previous = None;
        }

        let worker = streams::start_tap(pid, tap).map_err(BackendError)?;
        self.ensure_sink_muted(&sink.id)?;
        self.journal.routes.push(RouteEntry { pid, exe, previous: previous.clone() });
        self.journal.save();
        let routed = self.routing.as_ref().map(|r| r.set(pid, Some(&sink.id)));
        if let Some(Err(e)) = routed {
            self.journal.routes.retain(|r| r.pid != pid);
            self.journal.save();
            drop(worker);
            if self.taps.is_empty() {
                self.restore_sink();
            }
            return Err(crate::util::werr("route app", e));
        }
        self.taps.insert(pid, Tap { _worker: worker, previous });
        Ok(())
    }

    fn release(&mut self, pid: u32) -> BResult<()> {
        let mut result = Ok(());
        if let Some(t) = self.taps.remove(&pid) {
            if let Some(r) = &self.routing {
                if let Err(e) = r.set(pid, t.previous.as_deref()) {
                    result = Err(crate::util::werr("restore app output", e));
                }
            }
            drop(t);
            if result.is_ok() {
                self.journal.routes.retain(|r| r.pid != pid);
            }
            self.journal.save();
        }
        if self.taps.is_empty() {
            self.restore_sink();
        }
        result
    }

    fn shutdown(&mut self) {
        let pids: Vec<u32> = self.taps.keys().copied().collect();
        for p in pids {
            let _ = self.release(p);
        }
        self.outputs.clear();
        self.restore_sink();
        unsafe {
            let _ = self.en.UnregisterEndpointNotificationCallback(&self.notifier);
        }
    }
}

impl Drop for WindowsBackend {
    fn drop(&mut self) {
        if !self.taps.is_empty() || self.journal.sink.is_some() {
            self.shutdown();
        }
    }
}
