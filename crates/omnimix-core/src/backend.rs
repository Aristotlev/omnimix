//! The contract every OS backend implements.
//!
//! Two planes:
//! * **Control plane**: enumerate devices and per-app sources, volume, mute,
//!   per-channel levels, meters, default device. Pure OS mixer calls.
//! * **DSP plane**: take over ("divert") a source so its audio flows through
//!   the OmniMix engine, and render engine buses to devices.
//!
//! How each OS diverts a source:
//! * Windows 10 2004+/11: process-loopback tap of the app + park the app on a
//!   muted sink endpoint (per-app routing). Original path silenced, tap is pre-endpoint.
//! * macOS 14.2+: Core Audio process tap with `muteBehavior = muted`.
//! * Linux (PipeWire): relink the app's stream node into an OmniMix filter node.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::engine::ring::Producer;
use crate::engine::{BusControls, LatencyMode, StripControls};
use crate::model::{Device, Levels, Snapshot, Source};

#[derive(Clone, Debug, PartialEq)]
pub struct BackendError(pub String);

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackendError {}

impl From<std::io::Error> for BackendError {
    fn from(e: std::io::Error) -> Self {
        BackendError(e.to_string())
    }
}

impl From<String> for BackendError {
    fn from(s: String) -> Self {
        BackendError(s)
    }
}

impl From<&str> for BackendError {
    fn from(s: &str) -> Self {
        BackendError(s.to_string())
    }
}

pub type BResult<T> = Result<T, BackendError>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    /// e.g. "Windows 11 Pro 24H2 (26100.2605)".
    pub os: String,
    /// e.g. "WASAPI / Core Audio".
    pub backend: String,
    /// Native accent color "#RRGGBB" (the "1 native" color of the palette).
    pub accent: Option<String>,
    /// Accent variant tuned for dark surfaces.
    pub accent_dark_ui: Option<String>,
    pub dark_mode: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DspSupport {
    pub available: bool,
    pub reason: Option<String>,
    /// Short description of the diversion mechanism in use.
    pub method: String,
    /// Backend needs a silent "sink" device to park diverted apps on.
    pub needs_sink: bool,
    /// The OS has no per-app mixer (macOS): app volume, mute and channel
    /// mute are applied by the OmniMix engine, so changing them diverts the app.
    pub soft_app_mixer: bool,
    /// Shown once, the first time an app is processed (e.g. an OS permission prompt).
    pub hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendEvent {
    DevicesChanged,
    SessionsChanged,
    DefaultChanged,
}

/// Everything a backend needs to start capturing one source into the engine.
pub struct TapRequest {
    pub producer: Producer,
    pub strip: Arc<StripControls>,
    pub sample_rate: u32,
    pub channels: u16,
}

pub trait AudioBackend {
    fn platform(&self) -> PlatformInfo;

    /// Process id of the host (its own sessions are hidden and never diverted).
    fn self_pid(&self) -> u32 {
        std::process::id()
    }

    /// Full re-enumeration of devices and sources.
    fn refresh(&mut self) -> BResult<Snapshot>;

    /// Peak meters for the given devices and sources (linear 0..1 per channel).
    fn levels(&mut self, devices: &[String], sources: &[String]) -> Levels;

    /// Change notifications since the last call.
    fn poll_events(&mut self) -> Vec<BackendEvent>;

    // ---- control plane -------------------------------------------------
    fn set_device_volume(&mut self, id: &str, volume: f32) -> BResult<()>;
    fn set_device_mute(&mut self, id: &str, muted: bool) -> BResult<()>;
    fn set_device_channel_volume(&mut self, id: &str, channel: u16, volume: f32) -> BResult<()>;
    fn set_default_device(&mut self, id: &str) -> BResult<()>;
    fn set_source_volume(&mut self, id: &str, volume: f32) -> BResult<()>;
    fn set_source_mute(&mut self, id: &str, muted: bool) -> BResult<()>;
    fn set_source_channel_volume(&mut self, id: &str, channel: u16, volume: f32) -> BResult<()>;

    // ---- DSP plane -----------------------------------------------------
    fn dsp_support(&mut self) -> DspSupport;

    /// Start an output stream on `device` driven by a new engine bus.
    fn open_output(&mut self, device: &Device, latency: LatencyMode) -> BResult<Arc<BusControls>>;

    fn close_output(&mut self, device_id: &str);

    /// Take over every stream of `source.pid`: deliver its audio into `tap`
    /// and silence its direct path (parking it on `sink` where the OS needs one).
    fn divert(&mut self, source: &Source, target: &Device, sink: Option<&Device>, tap: TapRequest) -> BResult<()>;

    /// Undo [`AudioBackend::divert`] for a process.
    fn release(&mut self, pid: u32) -> BResult<()>;

    /// Restore every change the backend made to the system (routing, mutes).
    fn shutdown(&mut self);
}
