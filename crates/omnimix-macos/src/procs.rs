//! Apps playing audio: Core Audio process objects + app identity (name, bundle, icon).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use omnimix_core::model::{pretty_exe_name, source_key_from_exe, Channel, Source};

use crate::ffi::*;
use crate::objc::{app_info, outer_app};

#[derive(Clone)]
struct Identity {
    key: String,
    name: String,
    exe: Option<String>,
    icon: Option<String>,
}

#[derive(Default)]
pub struct ProcTable {
    ids: HashMap<i32, Identity>,
    last_output: HashMap<i32, Instant>,
    user_app: HashMap<i32, bool>,
}

/// Keep an app card for this long after it stops playing (pauses between tracks).
const LINGER: Duration = Duration::from_secs(600);

pub fn process_object(pid: i32) -> Option<AudioObjectID> {
    let a = addr(kAudioHardwarePropertyTranslatePIDToProcessObject, SCOPE_GLOBAL, ELEMENT_MAIN);
    get_q::<AudioObjectID, i32>(kAudioObjectSystemObject, a, Some(&pid)).filter(|o| *o != kAudioObjectUnknown)
}

impl ProcTable {
    fn identity(&mut self, pid: i32, bundle: Option<String>) -> Identity {
        if let Some(i) = self.ids.get(&pid) {
            return i.clone();
        }
        let exe = exe_path(pid);
        // Safari and every WKWebView app play audio from the shared WebKit GPU process.
        if exe.as_deref().map(|e| e.contains("com.apple.WebKit")).unwrap_or(false) || bundle.as_deref().map(|b| b.starts_with("com.apple.WebKit")).unwrap_or(false) {
            let icon = crate::objc::app_info("/Applications/Safari.app").icon_png;
            let id = Identity { key: "com.apple.webkit".into(), name: "Safari & web apps".into(), exe: exe.clone(), icon };
            self.ids.insert(pid, id.clone());
            return id;
        }
        let id = match exe.as_deref().and_then(outer_app) {
            Some(app) => {
                let info = app_info(&app);
                let key = info.bundle_id.clone().or(bundle.clone()).map(|b| b.to_lowercase()).unwrap_or_else(|| source_key_from_exe(&app));
                Identity { key, name: info.name, exe: exe.clone(), icon: info.icon_png }
            }
            None => {
                let name = exe.as_deref().map(pretty_exe_name).or_else(|| bundle.clone()).unwrap_or_else(|| format!("Process {pid}"));
                let key = bundle.map(|b| b.to_lowercase()).or_else(|| exe.as_deref().map(source_key_from_exe)).unwrap_or_else(|| format!("pid-{pid}"));
                Identity { key, name, exe: exe.clone(), icon: None }
            }
        };
        self.ids.insert(pid, id.clone());
        id
    }

    fn is_user_app(&mut self, pid: i32) -> bool {
        if let Some(v) = self.user_app.get(&pid) {
            return *v;
        }
        let v = exe_path(pid)
            .and_then(|e| outer_app(&e))
            .map(|app| !(app.starts_with("/System/Library/") || app.starts_with("/Library/Apple/") || app.starts_with("/usr/") || app.contains("/CoreServices/")))
            .unwrap_or(false);
        self.user_app.insert(pid, v);
        v
    }

    /// Sources for every app that is playing, played recently, or is tapped.
    pub fn sources(&mut self, uid_of: &dyn Fn(AudioObjectID) -> Option<String>, default_uid: Option<&str>, tapped: &dyn Fn(i32) -> bool) -> Vec<Source> {
        let now = Instant::now();
        let objs: Vec<AudioObjectID> = get_vec(kAudioObjectSystemObject, addr(kAudioHardwarePropertyProcessObjectList, SCOPE_GLOBAL, ELEMENT_MAIN));
        let mut out = vec![];
        let mut alive = vec![];
        for obj in objs {
            let Some(pid) = get::<i32>(obj, addr(kAudioProcessPropertyPID, SCOPE_GLOBAL, ELEMENT_MAIN)) else { continue };
            if pid <= 0 {
                continue;
            }
            alive.push(pid);
            let running = get::<u32>(obj, addr(kAudioProcessPropertyIsRunningOutput, SCOPE_GLOBAL, ELEMENT_MAIN)).unwrap_or(0) != 0;
            if running {
                self.last_output.insert(pid, now);
            }
            let recent = self.last_output.get(&pid).map(|t| now.duration_since(*t) < LINGER).unwrap_or(false);
            // Regular apps (Discord, Spotify, browsers, games) are listed even while silent,
            // like the Windows mixer does. Background system services only while they play.
            if !(running || recent || tapped(pid) || self.is_user_app(pid)) {
                continue;
            }
            let bundle = get_string(obj, addr(kAudioProcessPropertyBundleID, SCOPE_GLOBAL, ELEMENT_MAIN));
            let devs: Vec<AudioObjectID> = get_vec(obj, addr(kAudioProcessPropertyDevices, SCOPE_OUTPUT, ELEMENT_MAIN));
            let device_id = devs.iter().find_map(|d| uid_of(*d)).or_else(|| default_uid.map(str::to_string));
            let Some(device_id) = device_id else { continue };
            let id = self.identity(pid, bundle);
            out.push(Source {
                id: format!("proc:{pid}"),
                key: id.key,
                pid: pid as u32,
                name: id.name,
                exe: id.exe,
                icon: id.icon,
                device_id,
                active: running,
                system: false,
                volume: 1.0,
                muted: false,
                channels: vec![
                    Channel { index: 0, label: "L".into(), volume: 1.0, muted: false },
                    Channel { index: 1, label: "R".into(), volume: 1.0, muted: false },
                ],
            });
        }
        self.ids.retain(|p, _| alive.contains(p));
        self.last_output.retain(|p, _| alive.contains(p));
        self.user_app.retain(|p, _| alive.contains(p));
        out
    }
}
