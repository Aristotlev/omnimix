//! Crash-safe record of every system change the DSP plane makes
//! (per-app routes, sink endpoint mute). Replayed on next start.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteEntry {
    pub pid: u32,
    /// Full executable path, used to find the app again after a restart.
    pub exe: String,
    /// The app's own persisted output before OmniMix touched it (None = default).
    pub previous: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SinkEntry {
    pub device_id: String,
    pub was_muted: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Journal {
    pub routes: Vec<RouteEntry>,
    pub sink: Option<SinkEntry>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Journal {
    pub fn open(path: Option<PathBuf>) -> Journal {
        let mut j: Journal = path.as_ref().and_then(|p| fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        j.path = path;
        j
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty() && self.sink.is_none()
    }

    pub fn save(&self) {
        let Some(p) = &self.path else { return };
        if self.is_empty() {
            let _ = fs::remove_file(p);
            return;
        }
        if let Some(d) = p.parent() {
            let _ = fs::create_dir_all(d);
        }
        let tmp = p.with_extension("tmp");
        if fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default()).is_ok() {
            let _ = fs::rename(&tmp, p);
        }
    }
}
