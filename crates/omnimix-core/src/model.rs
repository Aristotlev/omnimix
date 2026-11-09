//! Universal audio model: devices (endpoints), sources (per-app sessions),
//! channels, and how a device is physically connected.
//!
//! Every backend maps its native concepts onto these types:
//! Windows endpoint / session, CoreAudio device / process object,
//! PipeWire node / stream.

use serde::{Deserialize, Serialize};

pub type DeviceId = String;
pub type SourceId = String;

/// How a playback device reaches the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    UsbC,
    Usb,
    Bluetooth,
    Aux,
    Display,
    Thunderbolt,
    Spdif,
    BuiltIn,
    Virtual,
    Network,
    Unknown,
}

impl ConnectionKind {
    pub fn label(self) -> &'static str {
        match self {
            ConnectionKind::UsbC => "USB-C",
            ConnectionKind::Usb => "USB",
            ConnectionKind::Bluetooth => "Bluetooth",
            ConnectionKind::Aux => "AUX",
            ConnectionKind::Display => "HDMI / DP",
            ConnectionKind::Thunderbolt => "Thunderbolt",
            ConnectionKind::Spdif => "S/PDIF",
            ConnectionKind::BuiltIn => "Built-in",
            ConnectionKind::Virtual => "Virtual",
            ConnectionKind::Network => "Network",
            ConnectionKind::Unknown => "Other",
        }
    }
}

/// What the endpoint physically is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormFactor {
    Speakers,
    Headphones,
    Headset,
    LineOut,
    Spdif,
    Display,
    Network,
    Handset,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Active,
    Unplugged,
    Disabled,
    NotPresent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamFormat {
    pub sample_rate: u32,
    pub channels: u16,
    /// Hardware bit depth when the platform reports it (0 = unknown).
    pub bits: u16,
    pub channel_mask: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub index: u16,
    pub label: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsbIdent {
    pub vid: u16,
    pub pid: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub kind: ConnectionKind,
    /// Human chips, e.g. ["Port 3", "USB 2.0 HS"], ["3.5 mm", "Front panel"], ["A2DP stereo"].
    pub detail: Vec<String>,
    /// Raw bus/enumerator token from the platform ("USB", "BTHENUM", "HDAUDIO", ...).
    pub bus: Option<String>,
    pub usb: Option<UsbIdent>,
    /// Jack color as 0xRRGGBB when the hardware reports it.
    pub jack_color: Option<u32>,
}

impl Default for ConnectionInfo {
    fn default() -> Self {
        ConnectionInfo { kind: ConnectionKind::Unknown, detail: vec![], bus: None, usb: None, jack_color: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: DeviceId,
    /// Full platform name, e.g. "Speakers (Realtek(R) Audio)".
    pub name: String,
    /// Short endpoint label, e.g. "Speakers".
    pub label: String,
    /// Adapter / driver name, e.g. "Realtek(R) Audio".
    pub adapter: String,
    pub form_factor: FormFactor,
    pub connection: ConnectionInfo,
    pub state: DeviceState,
    pub is_default: bool,
    pub is_default_comms: bool,
    pub format: Option<StreamFormat>,
    pub volume: f32,
    pub muted: bool,
    pub channels: Vec<Channel>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// Platform session id (unique per running session).
    pub id: SourceId,
    /// Stable settings key: lower-case executable file name, or "system".
    pub key: String,
    pub pid: u32,
    pub name: String,
    pub exe: Option<String>,
    /// PNG data URL.
    pub icon: Option<String>,
    /// Device the session is physically attached to right now.
    pub device_id: DeviceId,
    pub active: bool,
    pub system: bool,
    pub volume: f32,
    pub muted: bool,
    pub channels: Vec<Channel>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub devices: Vec<Device>,
    pub sources: Vec<Source>,
}

/// Peak meters, linear 0..1 per channel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    #[serde(rename = "d")]
    pub devices: Vec<(DeviceId, Vec<f32>)>,
    #[serde(rename = "s")]
    pub sources: Vec<(SourceId, Vec<f32>)>,
}

/// Speaker positions in the order used by WAVEFORMATEXTENSIBLE / CoreAudio / PipeWire
/// channel masks (bit 0 = front left ...).
const SPEAKER_LABELS: [&str; 18] = [
    "L", "R", "C", "LFE", "BL", "BR", "FLC", "FRC", "BC", "SL", "SR", "TC", "TFL", "TFC", "TFR", "TBL", "TBC", "TBR",
];

/// Labels for `count` channels given a speaker bit mask (0 = unknown layout).
pub fn channel_labels(mask: u32, count: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(count);
    if mask != 0 {
        for (bit, name) in SPEAKER_LABELS.iter().enumerate() {
            if out.len() == count {
                break;
            }
            if mask & (1 << bit) != 0 {
                out.push((*name).to_string());
            }
        }
    }
    if out.len() != count {
        out.clear();
        match count {
            1 => out.push("M".into()),
            2 => {
                out.push("L".into());
                out.push("R".into());
            }
            _ => {
                for i in 0..count {
                    out.push(format!("{}", i + 1));
                }
            }
        }
    }
    out
}

/// Stable settings key for an app from its executable path.
pub fn source_key_from_exe(exe: &str) -> String {
    let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    file.to_lowercase()
}

/// Human-friendly name from an executable path when no description exists.
pub fn pretty_exe_name(exe: &str) -> String {
    let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file);
    let mut chars = stem.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => stem.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_stereo_and_surround() {
        assert_eq!(channel_labels(0x3, 2), vec!["L", "R"]);
        assert_eq!(channel_labels(0x3F, 6), vec!["L", "R", "C", "LFE", "BL", "BR"]);
        assert_eq!(channel_labels(0x60F, 6), vec!["L", "R", "C", "LFE", "SL", "SR"]);
        assert_eq!(channel_labels(0, 2), vec!["L", "R"]);
        assert_eq!(channel_labels(0, 3), vec!["1", "2", "3"]);
    }

    #[test]
    fn exe_keys() {
        assert_eq!(source_key_from_exe(r"C:\Program Files\Spotify\Spotify.exe"), "spotify.exe");
        assert_eq!(pretty_exe_name(r"C:\x\discord.exe"), "Discord");
    }
}
