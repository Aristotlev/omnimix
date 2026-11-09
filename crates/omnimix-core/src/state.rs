//! Persistent user state: EQ per device and per app, channel mute memory,
//! user presets, preferences. Stored as JSON, written atomically.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dsp::{EqSettings, Preset};
use crate::engine::LatencyMode;

pub const STATE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DeviceSettings {
    pub eq: EqSettings,
    /// Channel volume to restore when a muted channel is unmuted.
    pub channel_memory: BTreeMap<u16, f32>,
    pub nickname: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SourceSettings {
    pub eq: EqSettings,
    pub channel_memory: BTreeMap<u16, f32>,
    /// Engine-side app volume/mute (OSes without a per-app mixer).
    pub volume: f32,
    pub muted: bool,
    pub channel_mute: u32,
}

impl Default for SourceSettings {
    fn default() -> Self {
        SourceSettings { eq: EqSettings::default(), channel_memory: BTreeMap::new(), volume: 1.0, muted: false, channel_mute: 0 }
    }
}

impl SourceSettings {
    /// Engine-side controls differ from pass-through.
    pub fn soft_active(&self) -> bool {
        self.volume < 0.999 || self.muted || self.channel_mute != 0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// Device used to park diverted apps (None = automatic).
    pub sink: Option<String>,
    pub latency: LatencyMode,
    pub show_inactive: bool,
    pub close_to_tray: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { sink: None, latency: LatencyMode::Balanced, show_inactive: true, close_to_tray: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppState {
    pub version: u32,
    pub devices: BTreeMap<String, DeviceSettings>,
    pub sources: BTreeMap<String, SourceSettings>,
    pub presets: Vec<Preset>,
    pub prefs: Prefs,
}

impl Default for AppState {
    fn default() -> Self {
        AppState { version: STATE_VERSION, devices: BTreeMap::new(), sources: BTreeMap::new(), presets: vec![], prefs: Prefs::default() }
    }
}

impl AppState {
    pub fn device(&self, id: &str) -> Option<&DeviceSettings> {
        self.devices.get(id)
    }
    pub fn device_mut(&mut self, id: &str) -> &mut DeviceSettings {
        self.devices.entry(id.to_string()).or_default()
    }
    pub fn source(&self, key: &str) -> Option<&SourceSettings> {
        self.sources.get(key)
    }
    pub fn source_mut(&mut self, key: &str) -> &mut SourceSettings {
        self.sources.entry(key.to_string()).or_default()
    }
    pub fn device_eq_on(&self, id: &str) -> bool {
        self.device(id).map(|d| d.eq.enabled).unwrap_or(false)
    }
    pub fn source_eq_on(&self, key: &str) -> bool {
        self.source(key).map(|s| s.eq.enabled).unwrap_or(false)
    }
}

/// Default per-user config directory without any OS API calls.
pub fn default_config_dir() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if cfg!(windows) {
        if let Some(p) = env("APPDATA") {
            return p.join("OmniMix");
        }
    } else if cfg!(target_os = "macos") {
        if let Some(h) = env("HOME") {
            return h.join("Library/Application Support/OmniMix");
        }
    } else if let Some(x) = env("XDG_CONFIG_HOME") {
        return x.join("omnimix");
    } else if let Some(h) = env("HOME") {
        return h.join(".config/omnimix");
    }
    PathBuf::from(".omnimix")
}

pub struct StateStore {
    path: Option<PathBuf>,
}

impl StateStore {
    pub fn new(path: Option<PathBuf>) -> StateStore {
        StateStore { path }
    }

    pub fn in_dir(dir: &Path) -> StateStore {
        StateStore { path: Some(dir.join("state.json")) }
    }

    pub fn memory() -> StateStore {
        StateStore { path: None }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn load(&self) -> AppState {
        let Some(p) = &self.path else { return AppState::default() };
        match fs::read_to_string(p) {
            Ok(s) => match serde_json::from_str::<AppState>(&s) {
                Ok(mut st) => {
                    st.version = STATE_VERSION;
                    st
                }
                Err(_) => {
                    let _ = fs::rename(p, p.with_extension("json.corrupt"));
                    AppState::default()
                }
            },
            Err(_) => AppState::default(),
        }
    }

    pub fn save(&self, st: &AppState) -> std::io::Result<()> {
        let Some(p) = &self.path else { return Ok(()) };
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = p.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(st).unwrap_or_default())?;
        fs::rename(&tmp, p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_corrupt_recovery() {
        let dir = std::env::temp_dir().join(format!("omx-state-{}", std::process::id()));
        let store = StateStore::in_dir(&dir);
        let mut st = AppState::default();
        st.device_mut("dev1").eq.enabled = true;
        st.source_mut("spotify.exe").eq.bands[1].gain_db = 4.0;
        store.save(&st).unwrap();
        let back = store.load();
        assert_eq!(back, st);
        fs::write(store.path().unwrap(), "{not json").unwrap();
        assert_eq!(store.load(), AppState::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let st: AppState = serde_json::from_str(r#"{"devices":{"x":{"eq":{"enabled":true}}}}"#).unwrap();
        assert!(st.device_eq_on("x"));
        assert_eq!(st.device("x").unwrap().eq.bands.len(), 10);
        assert_eq!(st.prefs.latency, LatencyMode::Balanced);
    }
}
