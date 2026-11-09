//! The orchestrator. Owns a backend, user state and the DSP graph, and keeps
//! reality in line with what the user asked for (the reconcile loop).
//! Hosts talk to it through [`spawn`] + [`MixerHandle`] with JSON-friendly
//! [`Command`]s, and receive [`MixerEvent`]s.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::backend::{AudioBackend, DspSupport, PlatformInfo, TapRequest};
use crate::dsp::presets::builtin_presets;
use crate::dsp::{EqParams, EqSettings, Preset};
use crate::engine::{new_strip, BusControls, StripControls};
use crate::model::{Channel, ConnectionKind, Device, DeviceState, Snapshot, Source};
use crate::state::{AppState, Prefs, StateStore};

const CONFIRM_TIMEOUT: Duration = Duration::from_secs(4);
const GONE_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_AFTER: Duration = Duration::from_secs(30);
const PINNED_RETRY_AFTER: Duration = Duration::from_secs(600);
const SAVE_EVERY: Duration = Duration::from_millis(1200);

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Command {
    SetDeviceVolume { id: String, volume: f32 },
    SetDeviceMute { id: String, muted: bool },
    SetDeviceChannelVolume { id: String, channel: u16, volume: f32 },
    SetDeviceChannelMute { id: String, channel: u16, muted: bool },
    SetDefaultDevice { id: String },
    SetSourceVolume { group: String, volume: f32 },
    SetSourceMute { group: String, muted: bool },
    SetSourceChannelMute { group: String, channel: u16, muted: bool },
    SetDeviceEq { id: String, eq: EqSettings },
    SetSourceEq { key: String, eq: EqSettings },
    SavePreset { name: String, eq: EqSettings },
    DeletePreset { name: String },
    SetPrefs { prefs: Prefs },
    Refresh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Direct,
    Starting,
    Processing,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiDevice {
    #[serde(flatten)]
    pub device: Device,
    pub eq: EqSettings,
    pub is_sink: bool,
    pub processing: bool,
    pub nickname: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiSource {
    /// Group id: one card per app per (logical) device.
    pub id: String,
    pub key: String,
    pub name: String,
    pub exe: Option<String>,
    pub device_id: String,
    pub physical_device_id: String,
    pub pids: Vec<u32>,
    pub session_ids: Vec<String>,
    pub active: bool,
    pub system: bool,
    pub volume: f32,
    pub muted: bool,
    pub channels: Vec<Channel>,
    pub eq: EqSettings,
    pub status: SourceStatus,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiDsp {
    #[serde(flatten)]
    pub support: DspSupport,
    pub sink: Option<String>,
    pub sink_candidates: Vec<String>,
    pub latency_ms: f32,
    pub streams: usize,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    pub version: String,
    pub platform: PlatformInfo,
    pub dsp: UiDsp,
    pub devices: Vec<UiDevice>,
    pub sources: Vec<UiSource>,
    pub presets: Vec<Preset>,
    pub prefs: Prefs,
    /// key -> PNG data URL. Full map on request, deltas on events.
    pub icons: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UiLevels {
    pub d: Vec<(String, Vec<f32>)>,
    pub s: Vec<(String, Vec<f32>)>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub level: &'static str,
    pub text: String,
}

#[derive(Clone, Debug)]
pub enum MixerEvent {
    State(Box<UiState>),
    Levels(UiLevels),
    Notice(Notice),
}

struct Diversion {
    key: String,
    target: String,
    strip: Arc<StripControls>,
    since: Instant,
    confirmed: bool,
    follow_default: bool,
    missing_since: Option<Instant>,
    /// App volume/mute when it was diverted, re-applied to its parked session.
    carry: (f32, bool),
}

struct Group {
    id: String,
    key: String,
    device: String,
    physical: String,
    sessions: Vec<usize>,
    pids: Vec<u32>,
}

pub struct Mixer {
    backend: Box<dyn AudioBackend>,
    store: StateStore,
    state: AppState,
    dirty_since: Option<Instant>,
    platform: PlatformInfo,
    dsp: DspSupport,
    dsp_note: Option<String>,
    snap: Snapshot,
    groups: Vec<Group>,
    buses: HashMap<String, Arc<BusControls>>,
    diverted: HashMap<u32, Diversion>,
    blocked: HashMap<u32, (String, Instant)>,
    sink: Option<String>,
    default_id: Option<String>,
    self_pid: u32,
    notices: Vec<Notice>,
    icons: BTreeMap<String, String>,
    icons_sent: HashSet<String>,
    /// pid -> (volume, mute, device it returns to, when) applied once the app is back.
    returning: HashMap<u32, (f32, bool, String, Instant)>,
    hint_shown: bool,
}

impl Mixer {
    pub fn new(mut backend: Box<dyn AudioBackend>, store: StateStore) -> Mixer {
        let state = store.load();
        let platform = backend.platform();
        let dsp = backend.dsp_support();
        let self_pid = backend.self_pid();
        let mut m = Mixer {
            backend,
            store,
            state,
            dirty_since: None,
            platform,
            dsp,
            dsp_note: None,
            snap: Snapshot::default(),
            groups: vec![],
            buses: HashMap::new(),
            diverted: HashMap::new(),
            blocked: HashMap::new(),
            sink: None,
            default_id: None,
            self_pid,
            notices: vec![],
            icons: BTreeMap::new(),
            icons_sent: HashSet::new(),
            returning: HashMap::new(),
            hint_shown: false,
        };
        m.refresh();
        m.reconcile();
        m
    }

    // ------------------------------------------------------------------ refresh

    fn refresh(&mut self) {
        match self.backend.refresh() {
            Ok(mut s) => {
                s.sources.retain(|x| x.pid != self.self_pid || x.system);
                for src in s.sources.iter_mut() {
                    if let Some(icon) = src.icon.take() {
                        self.icons.entry(src.key.clone()).or_insert(icon);
                    }
                }
                let new_default = s.devices.iter().find(|d| d.is_default).map(|d| d.id.clone());
                if new_default != self.default_id {
                    let old = self.default_id.take();
                    self.default_id = new_default;
                    if old.is_some() {
                        self.on_default_changed();
                    }
                }
                self.snap = s;
            }
            Err(e) => self.notice("error", format!("Refresh failed: {e}")),
        }
        self.rebuild_groups();
    }

    fn on_default_changed(&mut self) {
        let Some(new_default) = self.default_id.clone() else { return };
        let moving: Vec<u32> =
            self.diverted.iter().filter(|(_, d)| d.follow_default && d.target != new_default).map(|(p, _)| *p).collect();
        for pid in moving {
            self.release(pid);
        }
    }

    fn logical_device(&self, src: &Source) -> String {
        match self.diverted.get(&src.pid) {
            Some(d) => d.target.clone(),
            None => src.device_id.clone(),
        }
    }

    fn rebuild_groups(&mut self) {
        let mut groups: Vec<Group> = vec![];
        let mut index: HashMap<String, usize> = HashMap::new();
        for (i, src) in self.snap.sources.iter().enumerate() {
            let dev = self.logical_device(src);
            let id = format!("{}|{}", src.key, dev);
            let gi = *index.entry(id.clone()).or_insert_with(|| {
                groups.push(Group { id: id.clone(), key: src.key.clone(), device: dev.clone(), physical: src.device_id.clone(), sessions: vec![], pids: vec![] });
                groups.len() - 1
            });
            let g = &mut groups[gi];
            g.sessions.push(i);
            if !g.pids.contains(&src.pid) {
                g.pids.push(src.pid);
            }
        }
        self.groups = groups;
    }

    fn device(&self, id: &str) -> Option<&Device> {
        self.snap.devices.iter().find(|d| d.id == id)
    }

    fn group(&self, id: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    fn session_ids(&self, group: &str) -> Vec<String> {
        self.group(group).map(|g| g.sessions.iter().map(|&i| self.snap.sources[i].id.clone()).collect()).unwrap_or_default()
    }

    // ------------------------------------------------------------------ commands

    pub fn handle(&mut self, cmd: Command) {
        let r: Result<(), String> = match cmd {
            Command::SetDeviceVolume { id, volume } => {
                let v = volume.clamp(0.0, 1.0);
                let r = self.backend.set_device_volume(&id, v).map_err(|e| e.0);
                if let Some(d) = self.snap.devices.iter_mut().find(|d| d.id == id) {
                    d.volume = v;
                }
                r
            }
            Command::SetDeviceMute { id, muted } => {
                if self.sink.as_deref() == Some(id.as_str()) {
                    Err("This output is parking processed apps and stays muted while EQ is active".into())
                } else {
                    let r = self.backend.set_device_mute(&id, muted).map_err(|e| e.0);
                    if let Some(d) = self.snap.devices.iter_mut().find(|d| d.id == id) {
                        d.muted = muted;
                    }
                    r
                }
            }
            Command::SetDeviceChannelVolume { id, channel, volume } => {
                let v = volume.clamp(0.0, 1.0);
                self.set_device_channel(&id, channel, v)
            }
            Command::SetDeviceChannelMute { id, channel, muted } => {
                let current = self.device(&id).and_then(|d| d.channels.iter().find(|c| c.index == channel)).map(|c| c.volume);
                if muted {
                    if let Some(v) = current.filter(|v| *v > 0.0001) {
                        self.state.device_mut(&id).channel_memory.insert(channel, v);
                        self.mark_dirty();
                    }
                    self.set_device_channel(&id, channel, 0.0)
                } else {
                    let v = self.state.device_mut(&id).channel_memory.remove(&channel).unwrap_or(1.0);
                    self.mark_dirty();
                    self.set_device_channel(&id, channel, v)
                }
            }
            Command::SetDefaultDevice { id } => self.backend.set_default_device(&id).map_err(|e| e.0),
            Command::SetSourceVolume { group, volume } if self.dsp.soft_app_mixer => {
                let v = volume.clamp(0.0, 1.0);
                match self.group(&group).map(|g| g.key.clone()) {
                    Some(key) => {
                        self.state.source_mut(&key).volume = v;
                        self.mark_dirty();
                        self.push_soft(&key);
                        self.reconcile();
                        Ok(())
                    }
                    None => Err("That app stopped playing".into()),
                }
            }
            Command::SetSourceMute { group, muted } if self.dsp.soft_app_mixer => match self.group(&group).map(|g| g.key.clone()) {
                Some(key) => {
                    self.state.source_mut(&key).muted = muted;
                    self.mark_dirty();
                    self.push_soft(&key);
                    self.reconcile();
                    Ok(())
                }
                None => Err("That app stopped playing".into()),
            },
            Command::SetSourceChannelMute { group, channel, muted } if self.dsp.soft_app_mixer => match self.group(&group).map(|g| g.key.clone()) {
                Some(key) if channel < 32 => {
                    let st = self.state.source_mut(&key);
                    if muted {
                        st.channel_mute |= 1 << channel;
                    } else {
                        st.channel_mute &= !(1 << channel);
                    }
                    self.mark_dirty();
                    self.push_soft(&key);
                    self.reconcile();
                    Ok(())
                }
                _ => Err("That app stopped playing".into()),
            },
            Command::SetSourceVolume { group, volume } => {
                let v = volume.clamp(0.0, 1.0);
                let mut r = Ok(());
                for sid in self.session_ids(&group) {
                    if let Err(e) = self.backend.set_source_volume(&sid, v) {
                        r = Err(e.0);
                    }
                    if let Some(s) = self.snap.sources.iter_mut().find(|s| s.id == sid) {
                        s.volume = v;
                    }
                }
                r
            }
            Command::SetSourceMute { group, muted } => {
                let mut r = Ok(());
                for sid in self.session_ids(&group) {
                    if let Err(e) = self.backend.set_source_mute(&sid, muted) {
                        r = Err(e.0);
                    }
                    if let Some(s) = self.snap.sources.iter_mut().find(|s| s.id == sid) {
                        s.muted = muted;
                    }
                }
                r
            }
            Command::SetSourceChannelMute { group, channel, muted } => self.set_source_channel_mute(&group, channel, muted),
            Command::SetDeviceEq { id, eq } => {
                let eq = eq.sanitized();
                if let Some(bus) = self.buses.get(&id) {
                    bus.eq.post(EqParams::from_settings(&eq, bus.sample_rate as f64));
                }
                self.state.device_mut(&id).eq = eq;
                self.mark_dirty();
                self.reconcile();
                Ok(())
            }
            Command::SetSourceEq { key, eq } => {
                let eq = eq.sanitized();
                for d in self.diverted.values().filter(|d| d.key == key) {
                    d.strip.eq.post(EqParams::from_settings(&eq, d.strip.sample_rate as f64));
                }
                self.state.source_mut(&key).eq = eq;
                self.mark_dirty();
                self.reconcile();
                Ok(())
            }
            Command::SavePreset { name, eq } => {
                let name: String = name.trim().chars().take(40).collect();
                if name.is_empty() {
                    Err("Preset name is empty".into())
                } else if builtin_presets().iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
                    Err("That name belongs to a built-in preset".into())
                } else {
                    let mut eq = eq.sanitized();
                    eq.preset = Some(name.clone());
                    eq.enabled = true;
                    self.state.presets.retain(|p| !p.name.eq_ignore_ascii_case(&name));
                    self.state.presets.push(Preset { name: name.clone(), builtin: false, eq });
                    self.mark_dirty();
                    self.notice("info", format!("Saved preset \u{201c}{name}\u{201d}"));
                    Ok(())
                }
            }
            Command::DeletePreset { name } => {
                self.state.presets.retain(|p| p.name != name);
                self.mark_dirty();
                Ok(())
            }
            Command::SetPrefs { prefs } => {
                let sink_changed = prefs.sink != self.state.prefs.sink;
                let latency_changed = prefs.latency != self.state.prefs.latency;
                self.state.prefs = prefs;
                self.mark_dirty();
                if sink_changed || latency_changed {
                    self.release_all();
                    self.sink = None;
                    for id in self.buses.keys().cloned().collect::<Vec<_>>() {
                        self.close_bus(&id);
                    }
                }
                self.reconcile();
                Ok(())
            }
            Command::Refresh => {
                self.refresh();
                self.reconcile();
                Ok(())
            }
        };
        if let Err(e) = r {
            self.notice("error", e);
        }
    }

    /// Apply engine-side app volume/mute/channel mute to every running strip of `key`.
    fn push_soft(&self, key: &str) {
        if !self.dsp.soft_app_mixer {
            return;
        }
        let st = self.state.source(key).cloned().unwrap_or_default();
        for d in self.diverted.values().filter(|d| d.key == key) {
            d.strip.gain.store(st.volume);
            d.strip.muted.store(st.muted, Ordering::Relaxed);
            d.strip.ch_mute.store(st.channel_mute, Ordering::Relaxed);
        }
    }

    fn set_device_channel(&mut self, id: &str, ch: u16, v: f32) -> Result<(), String> {
        let r = self.backend.set_device_channel_volume(id, ch, v).map_err(|e| e.0);
        if let Some(c) = self.snap.devices.iter_mut().find(|d| d.id == id).and_then(|d| d.channels.iter_mut().find(|c| c.index == ch)) {
            c.volume = v;
            c.muted = v <= 0.0001;
        }
        r
    }

    fn set_source_channel_mute(&mut self, group: &str, ch: u16, muted: bool) -> Result<(), String> {
        let Some(g) = self.group(group) else { return Err("Source is gone".into()) };
        let key = g.key.clone();
        let sids = self.session_ids(group);
        let current = sids
            .first()
            .and_then(|sid| self.snap.sources.iter().find(|s| &s.id == sid))
            .and_then(|s| s.channels.iter().find(|c| c.index == ch))
            .map(|c| c.volume);
        let v = if muted {
            if let Some(v) = current.filter(|v| *v > 0.0001) {
                self.state.source_mut(&key).channel_memory.insert(ch, v);
            }
            0.0
        } else {
            self.state.source_mut(&key).channel_memory.remove(&ch).unwrap_or(1.0)
        };
        self.mark_dirty();
        let mut r = Ok(());
        for sid in sids {
            if let Err(e) = self.backend.set_source_channel_volume(&sid, ch, v) {
                r = Err(e.0);
            }
            if let Some(c) = self.snap.sources.iter_mut().find(|s| s.id == sid).and_then(|s| s.channels.iter_mut().find(|c| c.index == ch)) {
                c.volume = v;
                c.muted = v <= 0.0001;
            }
        }
        r
    }

    // ------------------------------------------------------------------ reconcile

    fn sink_valid(&self, id: &str) -> bool {
        let Some(d) = self.device(id) else { return false };
        d.state == DeviceState::Active && !d.is_default && !self.state.device_eq_on(id) && !self.diverted.values().any(|x| x.target == id)
    }

    fn sink_candidates(&self) -> Vec<String> {
        self.snap
            .devices
            .iter()
            .filter(|d| d.state == DeviceState::Active && !d.is_default && !self.state.device_eq_on(&d.id))
            .map(|d| d.id.clone())
            .collect()
    }

    fn pick_sink(&self) -> Option<String> {
        if let Some(p) = &self.state.prefs.sink {
            return if self.sink_valid(p) { Some(p.clone()) } else { None };
        }
        let busy: HashSet<&str> = self
            .snap
            .sources
            .iter()
            .filter(|s| s.active && !self.diverted.contains_key(&s.pid) && s.pid != self.self_pid)
            .map(|s| s.device_id.as_str())
            .collect();
        self.snap
            .devices
            .iter()
            .filter(|d| self.sink_valid(&d.id))
            .map(|d| {
                let name = d.name.to_lowercase();
                let mut score = match d.connection.kind {
                    ConnectionKind::Virtual => 100,
                    ConnectionKind::Display => 60,
                    ConnectionKind::Spdif => 55,
                    ConnectionKind::BuiltIn => 45,
                    ConnectionKind::Aux => 40,
                    _ => 10,
                };
                if name.contains("cable") || name.contains("virtual") {
                    score += 50;
                }
                if busy.contains(d.id.as_str()) {
                    score -= 80;
                }
                (score, d.id.clone())
            })
            .max_by_key(|(s, _)| *s)
            .map(|(_, id)| id)
    }

    fn ensure_bus(&mut self, device_id: &str) -> Result<Arc<BusControls>, String> {
        if let Some(b) = self.buses.get(device_id) {
            if b.alive.load(Ordering::Relaxed) || b.error.lock().map(|e| e.is_none()).unwrap_or(true) {
                return Ok(b.clone());
            }
            self.close_bus(device_id);
        }
        let dev = self.device(device_id).cloned().ok_or("Device disappeared")?;
        let bus = self.backend.open_output(&dev, self.state.prefs.latency).map_err(|e| e.0)?;
        let eq = self.state.device(device_id).map(|d| d.eq.clone()).unwrap_or_default();
        bus.eq.post(EqParams::from_settings(&eq, bus.sample_rate as f64));
        self.buses.insert(device_id.to_string(), bus.clone());
        Ok(bus)
    }

    fn close_bus(&mut self, device_id: &str) {
        if self.buses.remove(device_id).is_some() {
            self.backend.close_output(device_id);
        }
    }

    fn divert(&mut self, pid: u32, src_index: usize, target: &str, sink: Option<String>) -> Result<(), String> {
        let src = self.snap.sources[src_index].clone();
        let target_dev = self.device(target).cloned().ok_or("Target device disappeared")?;
        let sink_dev = match &sink {
            Some(s) => Some(self.device(s).cloned().ok_or("Sink device disappeared")?),
            None => None,
        };
        let bus = self.ensure_bus(target)?;
        let key = format!("pid:{pid}");
        let (ctl, producer, strip) = new_strip(&key, bus.sample_rate, bus.channels, self.state.prefs.latency);
        let eq = self.state.source(&src.key).map(|s| s.eq.clone()).unwrap_or_default();
        ctl.eq.post(EqParams::from_settings(&eq, bus.sample_rate as f64));
        if self.dsp.soft_app_mixer {
            let st = self.state.source(&src.key).cloned().unwrap_or_default();
            ctl.gain.store(st.volume);
            ctl.muted.store(st.muted, Ordering::Relaxed);
            ctl.ch_mute.store(st.channel_mute, Ordering::Relaxed);
        } else {
            ctl.gain.store(if self.dsp.needs_sink { 0.0 } else { 1.0 });
        }
        bus.add_strip(strip);
        let tap = TapRequest { producer, strip: ctl.clone(), sample_rate: bus.sample_rate, channels: bus.channels as u16 };
        if let Err(e) = self.backend.divert(&src, &target_dev, sink_dev.as_ref(), tap) {
            bus.remove_strip(&key);
            return Err(e.0);
        }
        let follow_default = self.default_id.as_deref() == Some(target);
        if !self.hint_shown {
            if let Some(h) = self.dsp.hint.clone() {
                self.notice("info", h);
            }
            self.hint_shown = true;
        }
        self.diverted.insert(
            pid,
            Diversion {
                key: src.key.clone(),
                target: target.to_string(),
                strip: ctl,
                since: Instant::now(),
                confirmed: !self.dsp.needs_sink,
                follow_default,
                missing_since: None,
                carry: (src.volume, src.muted),
            },
        );
        Ok(())
    }

    fn release(&mut self, pid: u32) {
        if let Some(d) = self.diverted.remove(&pid) {
            d.strip.gain.store(0.0);
            if self.dsp.needs_sink {
                let parked = self.snap.sources.iter().find(|s| s.pid == pid && Some(&s.device_id) == self.sink.as_ref());
                let (v, m) = parked.map(|s| (s.volume, s.muted)).unwrap_or(d.carry);
                self.returning.insert(pid, (v, m, d.target.clone(), Instant::now()));
            }
            if let Err(e) = self.backend.release(pid) {
                self.notice("warn", format!("Restoring {} failed: {e}", d.key));
            }
            if let Some(bus) = self.buses.get(&d.target) {
                bus.remove_strip(&d.strip.key);
            }
        }
    }

    fn release_all(&mut self) {
        let pids: Vec<u32> = self.diverted.keys().copied().collect();
        for p in pids {
            self.release(p);
        }
    }

    fn block(&mut self, pid: u32, reason: String, for_: Duration) {
        self.blocked.insert(pid, (reason, Instant::now() + for_));
    }

    pub fn reconcile(&mut self) {
        let now = Instant::now();
        self.blocked.retain(|_, (_, until)| *until > now);
        self.dsp_note = None;

        // Apps coming back from the parking output keep the level the user set.
        if !self.returning.is_empty() {
            let mut done = vec![];
            for (pid, (v, m, target, at)) in self.returning.iter() {
                if self.diverted.contains_key(pid) || now.duration_since(*at) > Duration::from_secs(10) {
                    done.push(*pid);
                    continue;
                }
                let ids: Vec<String> = self.snap.sources.iter().filter(|s| s.pid == *pid && &s.device_id == target).map(|s| s.id.clone()).collect();
                if !ids.is_empty() {
                    for id in ids {
                        let _ = self.backend.set_source_volume(&id, *v);
                        let _ = self.backend.set_source_mute(&id, *m);
                    }
                    done.push(*pid);
                }
            }
            for p in done {
                self.returning.remove(&p);
            }
        }

        if !self.dsp.available {
            if !self.diverted.is_empty() {
                self.release_all();
            }
            for id in self.buses.keys().cloned().collect::<Vec<_>>() {
                self.close_bus(&id);
            }
            return;
        }

        // The sink must stay valid, or every diversion parked on it is moved off.
        if let Some(s) = self.sink.clone() {
            let still_ok = self.device(&s).map(|d| d.state == DeviceState::Active && !d.is_default && !self.state.device_eq_on(&s)).unwrap_or(false)
                && self.state.prefs.sink.as_ref().map(|p| p == &s).unwrap_or(true);
            if !still_ok {
                self.release_all();
                self.sink = None;
            }
        }

        // What should be diverted right now: pid -> (source index, target device).
        let mut wanted: BTreeMap<u32, (usize, String)> = BTreeMap::new();
        for (i, src) in self.snap.sources.iter().enumerate() {
            if src.system || src.pid == 0 || src.pid == self.self_pid {
                continue;
            }
            let target = self.logical_device(src);
            if self.sink.as_deref() == Some(target.as_str()) && !self.diverted.contains_key(&src.pid) {
                continue;
            }
            let soft = self.dsp.soft_app_mixer && self.state.source(&src.key).map(|s| s.soft_active()).unwrap_or(false);
            let want = self.state.device_eq_on(&target) || self.state.source_eq_on(&src.key) || soft;
            let dev_ok = self.device(&target).map(|d| d.state == DeviceState::Active).unwrap_or(false);
            if want && dev_ok {
                wanted.entry(src.pid).or_insert((i, target));
            }
        }

        // Stop what is no longer wanted. Processes with no sessions at all are
        // handled by the gone-timeout below (streams can blink between tracks).
        let live: HashSet<u32> = self.snap.sources.iter().map(|s| s.pid).collect();
        let stale: Vec<u32> = self
            .diverted
            .iter()
            .filter(|(pid, d)| live.contains(pid) && wanted.get(pid).map(|(_, t)| t != &d.target).unwrap_or(true))
            .map(|(p, _)| *p)
            .collect();
        for pid in stale {
            self.release(pid);
        }

        // Start what is newly wanted.
        let todo: Vec<(u32, usize, String)> =
            wanted.iter().filter(|(pid, _)| !self.diverted.contains_key(pid) && !self.blocked.contains_key(pid)).map(|(p, (i, t))| (*p, *i, t.clone())).collect();
        if !todo.is_empty() {
            let sink = if self.dsp.needs_sink {
                if self.sink.is_none() {
                    self.sink = self.pick_sink();
                }
                if self.sink.is_none() {
                    self.dsp_note = Some(
                        "EQ needs a second output to park audio on. Plug in or enable another output, or install a virtual cable (e.g. VB-CABLE), then pick it under Settings.".into(),
                    );
                }
                self.sink.clone()
            } else {
                None
            };
            if !self.dsp.needs_sink || sink.is_some() {
                for (pid, idx, target) in todo {
                    if Some(&target) == sink.as_ref() {
                        continue;
                    }
                    if let Err(e) = self.divert(pid, idx, &target, sink.clone()) {
                        let name = self.snap.sources[idx].name.clone();
                        self.notice("warn", format!("Couldn't process {name}: {e}"));
                        self.block(pid, e, RETRY_AFTER);
                    }
                }
            }
        }

        // Confirm diversions took effect; drop the ones whose process is gone.
        let sink = self.sink.clone();
        let mut drop_list: Vec<(u32, Option<(String, Duration)>)> = vec![];
        let mut carry_apply: Vec<(String, (f32, bool))> = vec![];
        let soft_mixer = self.dsp.soft_app_mixer;
        for (pid, d) in self.diverted.iter_mut() {
            let sessions: Vec<&Source> = self.snap.sources.iter().filter(|s| s.pid == *pid).collect();
            if let Some(err) = d.strip.capture_error.lock().ok().and_then(|g| g.clone()) {
                drop_list.push((*pid, Some((err, RETRY_AFTER))));
                continue;
            }
            if sessions.is_empty() {
                let since = *d.missing_since.get_or_insert(now);
                if now.duration_since(since) > GONE_TIMEOUT {
                    drop_list.push((*pid, None));
                }
                continue;
            }
            d.missing_since = None;
            if !d.confirmed {
                let parked = sessions.iter().any(|s| Some(&s.device_id) == sink.as_ref());
                let still_direct = sessions.iter().any(|s| s.device_id == d.target);
                if parked || !still_direct {
                    d.confirmed = true;
                    if !soft_mixer {
                        d.strip.gain.store(1.0);
                    }
                    for s in sessions.iter().filter(|s| Some(&s.device_id) == sink.as_ref()) {
                        carry_apply.push((s.id.clone(), d.carry));
                    }
                } else if now.duration_since(d.since) > CONFIRM_TIMEOUT {
                    drop_list.push((
                        *pid,
                        Some(("This app picks its output device itself. Set its output to Default inside the app to process it.".into(), PINNED_RETRY_AFTER)),
                    ));
                }
            }
        }
        for (id, (v, m)) in carry_apply {
            let _ = self.backend.set_source_volume(&id, v);
            let _ = self.backend.set_source_mute(&id, m);
        }
        for (pid, why) in drop_list {
            self.release(pid);
            if let Some((reason, dur)) = why {
                self.block(pid, reason, dur);
            }
        }

        // Close buses nobody needs; keep EQ'd devices warm for new apps.
        let needed: HashSet<String> = self.diverted.values().map(|d| d.target.clone()).collect();
        let idle: Vec<String> = self
            .buses
            .keys()
            .filter(|id| {
                let alive_dev = self.device(id).map(|d| d.state == DeviceState::Active).unwrap_or(false);
                !alive_dev || (!needed.contains(*id) && !self.state.device_eq_on(id))
            })
            .cloned()
            .collect();
        for id in idle {
            self.close_bus(&id);
        }
        if self.diverted.is_empty() && self.state.prefs.sink.is_none() {
            self.sink = None;
        }
        self.rebuild_groups();
    }

    // ------------------------------------------------------------------ outputs

    pub fn tick(&mut self) {
        let events = self.backend.poll_events();
        let _ = events;
        self.refresh();
        self.reconcile();
        if let Some(t) = self.dirty_since {
            if t.elapsed() > SAVE_EVERY {
                self.save();
            }
        }
    }

    fn mark_dirty(&mut self) {
        if self.dirty_since.is_none() {
            self.dirty_since = Some(Instant::now());
        }
    }

    fn save(&mut self) {
        if let Err(e) = self.store.save(&self.state) {
            self.notice("error", format!("Saving settings failed: {e}"));
        }
        self.dirty_since = None;
    }

    fn notice(&mut self, level: &'static str, text: String) {
        self.notices.push(Notice { level, text });
    }

    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    pub fn ui_state(&mut self, all_icons: bool) -> UiState {
        let devices = self
            .snap
            .devices
            .iter()
            .filter(|d| self.state.prefs.show_inactive || d.state == DeviceState::Active)
            .map(|d| {
                let ds = self.state.device(&d.id);
                UiDevice {
                    device: d.clone(),
                    eq: ds.map(|s| s.eq.clone()).unwrap_or_default(),
                    is_sink: self.sink.as_deref() == Some(d.id.as_str()),
                    processing: self.buses.contains_key(&d.id),
                    nickname: ds.and_then(|s| s.nickname.clone()),
                }
            })
            .collect();
        let sources = self
            .groups
            .iter()
            .map(|g| {
                let sessions: Vec<&Source> = g.sessions.iter().map(|&i| &self.snap.sources[i]).collect();
                let parked = sessions.iter().find(|s| self.sink.as_ref() == Some(&s.device_id)).copied();
                let first = parked.or_else(|| sessions.iter().find(|s| s.active).copied()).unwrap_or(sessions[0]);
                let processing = g.pids.iter().any(|p| self.diverted.get(p).map(|d| d.confirmed).unwrap_or(false));
                let starting = !processing && g.pids.iter().any(|p| self.diverted.contains_key(p));
                let blocked = g.pids.iter().find_map(|p| self.blocked.get(p).map(|(r, _)| r.clone()));
                let (status, note) = if first.system {
                    (SourceStatus::Direct, Some("System sounds play directly and can't be EQ'd per app".to_string()))
                } else if processing {
                    (SourceStatus::Processing, None)
                } else if starting {
                    (SourceStatus::Starting, None)
                } else if let Some(r) = blocked {
                    (SourceStatus::Blocked, Some(r))
                } else {
                    (SourceStatus::Direct, None)
                };
                let mut channels = first.channels.clone();
                for c in channels.iter_mut() {
                    c.muted = c.volume <= 0.0001;
                }
                let soft = self.dsp.soft_app_mixer.then(|| self.state.source(&g.key).cloned().unwrap_or_default());
                if let Some(st) = &soft {
                    for c in channels.iter_mut() {
                        c.muted = c.index < 32 && st.channel_mute & (1 << c.index) != 0;
                        c.volume = if c.muted { 0.0 } else { 1.0 };
                    }
                }
                UiSource {
                    id: g.id.clone(),
                    key: g.key.clone(),
                    name: first.name.clone(),
                    exe: first.exe.clone(),
                    device_id: g.device.clone(),
                    physical_device_id: g.physical.clone(),
                    pids: g.pids.clone(),
                    session_ids: sessions.iter().map(|s| s.id.clone()).collect(),
                    active: sessions.iter().any(|s| s.active),
                    system: first.system,
                    volume: soft.as_ref().map(|s| s.volume).unwrap_or(first.volume),
                    muted: soft.as_ref().map(|s| s.muted).unwrap_or_else(|| sessions.iter().all(|s| s.muted)),
                    channels,
                    eq: self.state.source(&g.key).map(|s| s.eq.clone()).unwrap_or_default(),
                    status,
                    note,
                }
            })
            .collect();
        let mut presets = builtin_presets();
        presets.extend(self.state.presets.iter().cloned());
        let icons: BTreeMap<String, String> = if all_icons {
            self.icons_sent.extend(self.icons.keys().cloned());
            self.icons.clone()
        } else {
            let fresh: BTreeMap<String, String> =
                self.icons.iter().filter(|(k, _)| !self.icons_sent.contains(*k)).map(|(k, v)| (k.clone(), v.clone())).collect();
            self.icons_sent.extend(fresh.keys().cloned());
            fresh
        };
        let latency = self.state.prefs.latency;
        let out_ms = self.buses.values().map(|b| b.output_latency_ms.load()).fold(0.0f32, f32::max);
        UiState {
            version: crate::VERSION.to_string(),
            platform: self.platform.clone(),
            dsp: UiDsp {
                support: self.dsp.clone(),
                sink: self.sink.clone(),
                sink_candidates: self.sink_candidates(),
                latency_ms: latency.buffer_ms() as f32 + if out_ms > 0.0 { out_ms } else { latency.output_ms() as f32 },
                streams: self.diverted.len(),
                note: self.dsp_note.clone(),
            },
            devices,
            sources,
            presets,
            prefs: self.state.prefs.clone(),
            icons,
        }
    }

    pub fn levels(&mut self) -> UiLevels {
        let dev_ids: Vec<String> = self.snap.devices.iter().filter(|d| d.state == DeviceState::Active).map(|d| d.id.clone()).collect();
        let src_ids: Vec<String> = self.snap.sources.iter().map(|s| s.id.clone()).collect();
        let lv = self.backend.levels(&dev_ids, &src_ids);
        let by_session: HashMap<&str, &Vec<f32>> = lv.sources.iter().map(|(k, v)| (k.as_str(), v)).collect();
        let mut s = Vec::with_capacity(self.groups.len());
        for g in &self.groups {
            let processed: Vec<&Diversion> = g.pids.iter().filter_map(|p| self.diverted.get(p)).filter(|d| d.confirmed).collect();
            let mut peaks: Vec<f32> = vec![];
            let mut merge = |v: &[f32]| {
                if peaks.len() < v.len() {
                    peaks.resize(v.len(), 0.0);
                }
                for (a, b) in peaks.iter_mut().zip(v) {
                    *a = a.max(*b);
                }
            };
            if processed.is_empty() {
                for &i in &g.sessions {
                    if let Some(v) = by_session.get(self.snap.sources[i].id.as_str()) {
                        merge(v);
                    }
                }
            } else {
                for d in processed {
                    merge(&d.strip.take_peaks());
                }
            }
            s.push((g.id.clone(), peaks));
        }
        let mut d = lv.devices;
        for (id, bus) in &self.buses {
            if !d.iter().any(|(x, _)| x == id) {
                d.push((id.clone(), bus.take_peaks()));
            }
        }
        UiLevels { d, s }
    }

    pub fn shutdown(&mut self) {
        self.release_all();
        for id in self.buses.keys().cloned().collect::<Vec<_>>() {
            self.close_bus(&id);
        }
        self.backend.shutdown();
        self.save();
    }
}

// ---------------------------------------------------------------------- threading

enum Msg {
    Cmd(Command),
    GetState(Sender<UiState>),
    Shutdown(Sender<()>),
}

#[derive(Clone)]
pub struct MixerHandle {
    tx: Sender<Msg>,
}

impl MixerHandle {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(Msg::Cmd(cmd));
    }

    pub fn state(&self) -> Option<UiState> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Msg::GetState(tx)).ok()?;
        rx.recv_timeout(Duration::from_secs(5)).ok()
    }

    /// Restore the system and stop the mixer thread (blocks up to 5 s).
    pub fn shutdown(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Msg::Shutdown(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(5));
        }
    }
}

pub struct MixerOptions {
    pub tick: Duration,
    pub levels: Duration,
    pub min_state_gap: Duration,
}

impl Default for MixerOptions {
    fn default() -> Self {
        MixerOptions { tick: Duration::from_millis(300), levels: Duration::from_millis(33), min_state_gap: Duration::from_millis(120) }
    }
}

/// Run a mixer on a dedicated thread. `make_backend` runs on that thread
/// (platform APIs like COM are initialised per thread).
pub fn spawn<F, S>(make_backend: F, store: StateStore, opts: MixerOptions, sink: S) -> (MixerHandle, JoinHandle<()>)
where
    F: FnOnce() -> Box<dyn AudioBackend> + Send + 'static,
    S: Fn(MixerEvent) + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Msg>();
    let jh = std::thread::Builder::new()
        .name("omnimix-mixer".into())
        .spawn(move || run(make_backend(), store, opts, rx, sink))
        .expect("spawn mixer thread");
    (MixerHandle { tx }, jh)
}

fn run<S: Fn(MixerEvent)>(backend: Box<dyn AudioBackend>, store: StateStore, opts: MixerOptions, rx: Receiver<Msg>, sink: S) {
    let mut m = Mixer::new(backend, store);
    let mut next_tick = Instant::now() + opts.tick;
    let mut next_levels = Instant::now();
    let mut last_state_json = String::new();
    let mut last_state_at = Instant::now() - opts.min_state_gap;
    let mut state_pending = true;
    loop {
        let now = Instant::now();
        let deadline = next_tick.min(next_levels);
        let wait = deadline.saturating_duration_since(now).min(Duration::from_millis(50));
        match rx.recv_timeout(wait) {
            Ok(Msg::Cmd(c)) => {
                m.handle(c);
                state_pending = true;
            }
            Ok(Msg::GetState(reply)) => {
                let st = m.ui_state(true);
                last_state_json = state_fingerprint(&st);
                let _ = reply.send(st);
            }
            Ok(Msg::Shutdown(done)) => {
                m.shutdown();
                let _ = done.send(());
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                m.shutdown();
                return;
            }
        }
        let now = Instant::now();
        if now >= next_levels {
            sink(MixerEvent::Levels(m.levels()));
            next_levels = now + opts.levels;
        }
        if now >= next_tick {
            m.tick();
            state_pending = true;
            next_tick = now + opts.tick;
        }
        for n in m.take_notices() {
            sink(MixerEvent::Notice(n));
        }
        if state_pending && now.duration_since(last_state_at) >= opts.min_state_gap {
            let st = m.ui_state(false);
            let fp = state_fingerprint(&st);
            if fp != last_state_json || !st.icons.is_empty() {
                last_state_json = fp;
                sink(MixerEvent::State(Box::new(st)));
            }
            last_state_at = now;
            state_pending = false;
        }
    }
}

fn state_fingerprint(st: &UiState) -> String {
    let mut s = st.clone();
    s.icons.clear();
    serde_json::to_string(&s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockBackend;

    fn mixer() -> Mixer {
        Mixer::new(Box::new(MockBackend::new()), StateStore::memory())
    }

    fn find<'a>(st: &'a UiState, key: &str) -> &'a UiSource {
        st.sources.iter().find(|s| s.key == key).unwrap()
    }

    #[test]
    fn source_eq_diverts_runs_engine_and_releases() {
        let mut m = mixer();
        let st = m.ui_state(true);
        let sp = find(&st, "spotify.exe").clone();
        assert_eq!(sp.status, SourceStatus::Direct);
        let mut eq = sp.eq.clone();
        eq.enabled = true;
        eq.bands[1].gain_db = 6.0;
        m.handle(Command::SetSourceEq { key: "spotify.exe".into(), eq: eq.clone() });
        let st = m.ui_state(false);
        assert_eq!(find(&st, "spotify.exe").status, SourceStatus::Processing);
        assert_eq!(st.dsp.streams, 1);
        assert!(st.devices.iter().find(|d| d.device.id == "mock:usbc").unwrap().processing);
        std::thread::sleep(Duration::from_millis(500));
        let lv = m.levels();
        let peaks = &lv.s.iter().find(|(id, _)| id == &sp.id).unwrap().1;
        assert!(peaks.iter().any(|p| *p > 0.01), "engine produced no audio: {peaks:?}");
        eq.enabled = false;
        m.handle(Command::SetSourceEq { key: "spotify.exe".into(), eq });
        let st = m.ui_state(false);
        assert_eq!(st.dsp.streams, 0);
        assert!(!st.devices.iter().find(|d| d.device.id == "mock:usbc").unwrap().processing);
        m.shutdown();
    }

    #[test]
    fn device_eq_diverts_every_app_on_that_device() {
        let mut m = mixer();
        let mut eq = EqSettings { enabled: true, ..Default::default() };
        eq.bands[0].gain_db = 4.0;
        m.handle(Command::SetDeviceEq { id: "mock:usbc".into(), eq });
        let st = m.ui_state(false);
        assert_eq!(st.dsp.streams, 2, "spotify + chrome, never system sounds");
        assert_eq!(find(&st, "system").status, SourceStatus::Direct);
        assert_eq!(find(&st, "discord.exe").status, SourceStatus::Direct);
        m.shutdown();
    }

    #[test]
    fn channel_mute_restores_previous_level() {
        let mut m = mixer();
        m.handle(Command::SetDeviceChannelVolume { id: "mock:aux".into(), channel: 3, volume: 0.7 });
        m.handle(Command::SetDeviceChannelMute { id: "mock:aux".into(), channel: 3, muted: true });
        m.tick();
        let st = m.ui_state(false);
        let lfe = &st.devices.iter().find(|d| d.device.id == "mock:aux").unwrap().device.channels[3];
        assert_eq!(lfe.label, "LFE");
        assert!(lfe.volume == 0.0);
        m.handle(Command::SetDeviceChannelMute { id: "mock:aux".into(), channel: 3, muted: false });
        m.tick();
        let st = m.ui_state(false);
        let lfe = &st.devices.iter().find(|d| d.device.id == "mock:aux").unwrap().device.channels[3];
        assert!((lfe.volume - 0.7).abs() < 1e-6);
    }

    #[test]
    fn source_mute_and_channel_mute() {
        let mut m = mixer();
        let id = find(&m.ui_state(true), "discord.exe").id.clone();
        m.handle(Command::SetSourceMute { group: id.clone(), muted: true });
        m.handle(Command::SetSourceChannelMute { group: id.clone(), channel: 1, muted: true });
        m.tick();
        let st = m.ui_state(false);
        let d = find(&st, "discord.exe");
        assert!(d.muted);
        assert!(d.channels[1].muted && !d.channels[0].muted);
    }

    #[test]
    fn presets_and_commands_parse() {
        let c: Command = serde_json::from_str(r#"{"type":"setSourceMute","group":"g","muted":true}"#).unwrap();
        assert!(matches!(c, Command::SetSourceMute { muted: true, .. }));
        let c: Command = serde_json::from_str(r#"{"type":"setDeviceChannelMute","id":"d","channel":2,"muted":false}"#).unwrap();
        assert!(matches!(c, Command::SetDeviceChannelMute { channel: 2, .. }));
        let mut m = mixer();
        let n = m.ui_state(false).presets.len();
        m.handle(Command::SavePreset { name: "My Kick".into(), eq: EqSettings::default() });
        assert_eq!(m.ui_state(false).presets.len(), n + 1);
        m.handle(Command::SavePreset { name: "flat".into(), eq: EqSettings::default() });
        assert!(m.take_notices().iter().any(|x| x.level == "error"));
    }

    #[test]
    fn ui_state_serializes_camel_case() {
        let mut m = mixer();
        let v = serde_json::to_value(m.ui_state(true)).unwrap();
        let d = &v["devices"][0];
        assert!(d.get("formFactor").is_some() && d.get("isSink").is_some() && d.get("connection").is_some());
        assert_eq!(d["connection"]["kind"], "usb_c");
        assert!(v["sources"][0].get("sessionIds").is_some());
        assert!(v["dsp"].get("needsSink").is_some());
    }

    #[test]
    fn soft_mixer_routes_app_volume_through_engine() {
        let mut m = Mixer::new(Box::new(MockBackend::soft_mixer()), StateStore::memory());
        let sp = find(&m.ui_state(true), "spotify.exe").clone();
        assert_eq!(sp.volume, 1.0, "soft mode starts at unity");
        m.handle(Command::SetSourceVolume { group: sp.id.clone(), volume: 0.4 });
        let st = m.ui_state(false);
        assert_eq!(st.dsp.streams, 1, "lowering volume diverts the app");
        assert!((find(&st, "spotify.exe").volume - 0.4).abs() < 1e-6);
        let d = m.diverted.values().next().unwrap();
        assert!((d.strip.gain.load() - 0.4).abs() < 1e-6);
        m.handle(Command::SetSourceChannelMute { group: sp.id.clone(), channel: 1, muted: true });
        assert_eq!(m.diverted.values().next().unwrap().strip.ch_mute.load(Ordering::Relaxed), 2);
        assert!(find(&m.ui_state(false), "spotify.exe").channels[1].muted);
        m.handle(Command::SetSourceChannelMute { group: sp.id.clone(), channel: 1, muted: false });
        m.handle(Command::SetSourceVolume { group: sp.id.clone(), volume: 1.0 });
        assert_eq!(m.ui_state(false).dsp.streams, 0, "back to unity releases the app");
        m.shutdown();
    }
}
