//! `AudioBackend` for macOS.

use std::collections::HashMap;
use std::sync::Arc;

use omnimix_core::backend::{AudioBackend, BResult, BackendError, BackendEvent, DspSupport, PlatformInfo, TapRequest};
use omnimix_core::engine::{BusControls, LatencyMode};
use omnimix_core::model::*;

use crate::ffi::*;
use crate::objc;
use crate::procs::{process_object, ProcTable};
use crate::streams::{Output, Tap};

/// Process taps need macOS 14.4+ (Sonoma).
pub const MIN_MACOS: (u32, u32) = (14, 4);

pub struct MacBackend {
    uid_to_id: HashMap<String, AudioObjectID>,
    id_to_uid: HashMap<AudioObjectID, String>,
    procs: ProcTable,
    outputs: HashMap<String, Output>,
    taps: HashMap<u32, Tap>,
    os: String,
    version: (u32, u32),
    platform: PlatformInfo,
}

fn connection(dev: AudioObjectID, name: &str) -> (ConnectionInfo, FormFactor) {
    let t = get::<u32>(dev, addr(kAudioDevicePropertyTransportType, SCOPE_GLOBAL, ELEMENT_MAIN)).unwrap_or(0);
    let lname = name.to_lowercase();
    let mut info = ConnectionInfo { bus: Some(String::from_utf8_lossy(&t.to_be_bytes()).trim().to_string()), ..Default::default() };
    let headphone_name = ["headphone", "airpods", "buds", "beats", "earphone"].iter().any(|k| lname.contains(k));
    let mut form = if headphone_name {
        FormFactor::Headphones
    } else if lname.contains("headset") {
        FormFactor::Headset
    } else {
        FormFactor::Unknown
    };
    match t {
        TRANSPORT_USB => {
            info.kind = ConnectionKind::Usb;
            info.detail.push("USB Audio".into());
        }
        TRANSPORT_BLUETOOTH | TRANSPORT_BLUETOOTH_LE => {
            info.kind = ConnectionKind::Bluetooth;
            let inputs = channel_count(dev, SCOPE_INPUT);
            let rate = get::<f64>(dev, addr(kAudioDevicePropertyNominalSampleRate, SCOPE_GLOBAL, ELEMENT_MAIN)).unwrap_or(0.0);
            info.detail.push(if t == TRANSPORT_BLUETOOTH_LE { "LE Audio".into() } else if inputs > 0 && rate > 0.0 && rate <= 24_000.0 { "Hands-free".into() } else { "Stereo".into() });
            if form == FormFactor::Unknown {
                form = FormFactor::Headphones;
            }
        }
        TRANSPORT_HDMI => {
            info.kind = ConnectionKind::Display;
            info.detail.push("HDMI".into());
            form = FormFactor::Display;
        }
        TRANSPORT_DISPLAYPORT => {
            info.kind = ConnectionKind::Display;
            info.detail.push("DisplayPort".into());
            form = FormFactor::Display;
        }
        TRANSPORT_THUNDERBOLT => {
            info.kind = ConnectionKind::Thunderbolt;
            info.detail.push("Thunderbolt".into());
        }
        TRANSPORT_AIRPLAY | TRANSPORT_AVB => {
            info.kind = ConnectionKind::Network;
            info.detail.push(if t == TRANSPORT_AIRPLAY { "AirPlay".into() } else { "AVB".into() });
        }
        TRANSPORT_VIRTUAL | TRANSPORT_AGGREGATE => {
            info.kind = ConnectionKind::Virtual;
            info.detail.push(if t == TRANSPORT_AGGREGATE { "Aggregate".into() } else { "Virtual".into() });
        }
        TRANSPORT_BUILTIN => {
            let src = get::<u32>(dev, addr(kAudioDevicePropertyDataSource, SCOPE_OUTPUT, ELEMENT_MAIN));
            match src {
                Some(SOURCE_HEADPHONES) => {
                    info.kind = ConnectionKind::Aux;
                    info.detail = vec!["3.5 mm".into(), "Headphone jack".into()];
                    form = FormFactor::Headphones;
                }
                Some(s) if s != SOURCE_INTERNAL_SPEAKER => {
                    info.kind = ConnectionKind::Aux;
                    info.detail = vec!["3.5 mm".into(), "Line out".into()];
                    form = FormFactor::LineOut;
                }
                _ => {
                    info.kind = ConnectionKind::BuiltIn;
                    info.detail.push("Internal speakers".into());
                    form = FormFactor::Speakers;
                }
            }
        }
        TRANSPORT_PCI | TRANSPORT_FIREWIRE => {
            info.kind = ConnectionKind::Unknown;
            info.detail.push(if t == TRANSPORT_PCI { "PCIe".into() } else { "FireWire".into() });
        }
        _ => info.kind = ConnectionKind::Unknown,
    }
    (info, form)
}

fn volume_elements(dev: AudioObjectID, channels: u32) -> Vec<u32> {
    (1..=channels.min(16)).filter(|e| has(dev, addr(kAudioDevicePropertyVolumeScalar, SCOPE_OUTPUT, *e))).collect()
}

impl MacBackend {
    pub fn new() -> BResult<MacBackend> {
        let (os, version) = objc::os_version();
        let accent = objc::accent_hex();
        let platform = PlatformInfo { os: os.clone(), backend: "Core Audio".into(), accent: accent.clone(), accent_dark_ui: accent, dark_mode: objc::dark_mode() };
        Ok(MacBackend { uid_to_id: HashMap::new(), id_to_uid: HashMap::new(), procs: ProcTable::default(), outputs: HashMap::new(), taps: HashMap::new(), os, version, platform })
    }

    fn dev(&self, uid: &str) -> BResult<AudioObjectID> {
        self.uid_to_id.get(uid).copied().ok_or_else(|| BackendError("That output is gone".into()))
    }

    fn device(&self, id: AudioObjectID, uid: &str, default: AudioObjectID) -> Device {
        let name = get_string(id, addr(kAudioObjectPropertyName, SCOPE_GLOBAL, ELEMENT_MAIN)).unwrap_or_else(|| "Audio output".into());
        let maker = get_string(id, addr(kAudioObjectPropertyManufacturer, SCOPE_GLOBAL, ELEMENT_MAIN)).unwrap_or_default();
        let (connection, form_factor) = connection(id, &name);
        let ch = channel_count(id, SCOPE_OUTPUT);
        let rate = get::<f64>(id, addr(kAudioDevicePropertyNominalSampleRate, SCOPE_GLOBAL, ELEMENT_MAIN)).unwrap_or(48_000.0);
        let streams: Vec<AudioObjectID> = get_vec(id, addr(kAudioDevicePropertyStreams, SCOPE_OUTPUT, ELEMENT_MAIN));
        let bits = streams
            .first()
            .and_then(|s| get::<AudioStreamBasicDescription>(*s, addr(kAudioStreamPropertyPhysicalFormat, SCOPE_GLOBAL, ELEMENT_MAIN)))
            .map(|f| f.mBitsPerChannel as u16)
            .unwrap_or(0);
        let elems = volume_elements(id, ch);
        let main_vol = get::<f32>(id, addr(kAudioDevicePropertyVolumeScalar, SCOPE_OUTPUT, ELEMENT_MAIN));
        let ch_vols: Vec<f32> = elems.iter().filter_map(|e| get::<f32>(id, addr(kAudioDevicePropertyVolumeScalar, SCOPE_OUTPUT, *e))).collect();
        let volume = main_vol.or_else(|| (!ch_vols.is_empty()).then(|| ch_vols.iter().sum::<f32>() / ch_vols.len() as f32)).unwrap_or(1.0);
        let muted = get::<u32>(id, addr(kAudioDevicePropertyMute, SCOPE_OUTPUT, ELEMENT_MAIN)).map(|m| m != 0).unwrap_or(false);
        let mask = match ch {
            6 => 0x3F,
            8 => 0x63F,
            _ => 0,
        };
        let labels = channel_labels(mask, ch as usize);
        let channels = if elems.len() as u32 == ch && ch > 0 {
            elems
                .iter()
                .zip(labels)
                .map(|(e, label)| {
                    let v = get::<f32>(id, addr(kAudioDevicePropertyVolumeScalar, SCOPE_OUTPUT, *e)).unwrap_or(1.0);
                    Channel { index: (*e - 1) as u16, label, volume: v, muted: v <= 0.0001 }
                })
                .collect()
        } else {
            vec![]
        };
        let label = match connection.kind {
            ConnectionKind::BuiltIn => "Speakers".to_string(),
            ConnectionKind::Aux => "Headphones".to_string(),
            _ => name.clone(),
        };
        let adapter = if matches!(connection.kind, ConnectionKind::BuiltIn | ConnectionKind::Aux) { name.clone() } else { maker };
        Device {
            id: uid.to_string(),
            name,
            label,
            adapter,
            form_factor,
            connection,
            state: DeviceState::Active,
            is_default: id == default,
            is_default_comms: false,
            format: Some(StreamFormat { sample_rate: rate.round() as u32, channels: ch as u16, bits, channel_mask: mask }),
            volume,
            muted,
            channels,
        }
    }
}

impl AudioBackend for MacBackend {
    fn platform(&self) -> PlatformInfo {
        self.platform.clone()
    }

    fn refresh(&mut self) -> BResult<Snapshot> {
        let ids: Vec<AudioObjectID> = get_vec(kAudioObjectSystemObject, addr(kAudioHardwarePropertyDevices, SCOPE_GLOBAL, ELEMENT_MAIN));
        let default = get::<AudioObjectID>(kAudioObjectSystemObject, addr(kAudioHardwarePropertyDefaultOutputDevice, SCOPE_GLOBAL, ELEMENT_MAIN)).unwrap_or(0);
        self.uid_to_id.clear();
        self.id_to_uid.clear();
        let mut devices = vec![];
        for id in ids {
            if channel_count(id, SCOPE_OUTPUT) == 0 {
                continue;
            }
            if get::<u32>(id, addr(kAudioDevicePropertyIsHidden, SCOPE_GLOBAL, ELEMENT_MAIN)).unwrap_or(0) != 0 {
                continue;
            }
            let Some(uid) = get_string(id, addr(kAudioDevicePropertyDeviceUID, SCOPE_GLOBAL, ELEMENT_MAIN)) else { continue };
            if uid.starts_with("app.omnimix.") {
                continue;
            }
            self.uid_to_id.insert(uid.clone(), id);
            self.id_to_uid.insert(id, uid.clone());
            devices.push(self.device(id, &uid, default));
        }
        let default_uid = self.id_to_uid.get(&default).cloned();
        let id_to_uid = self.id_to_uid.clone();
        let tapped: Vec<u32> = self.taps.keys().copied().collect();
        let sources = self.procs.sources(&|d| id_to_uid.get(&d).cloned(), default_uid.as_deref(), &|pid| tapped.contains(&(pid as u32)));
        Ok(Snapshot { devices, sources })
    }

    fn levels(&mut self, _devices: &[String], _sources: &[String]) -> Levels {
        // macOS has no per-device or per-app meters. The mixer shows engine
        // meters for everything OmniMix processes.
        Levels::default()
    }

    fn poll_events(&mut self) -> Vec<BackendEvent> {
        vec![]
    }

    fn set_device_volume(&mut self, uid: &str, volume: f32) -> BResult<()> {
        let id = self.dev(uid)?;
        let v = volume.clamp(0.0, 1.0);
        let main = addr(kAudioDevicePropertyVolumeScalar, SCOPE_OUTPUT, ELEMENT_MAIN);
        if settable(id, main) {
            return set(id, main, &v).map_err(BackendError);
        }
        let elems = volume_elements(id, channel_count(id, SCOPE_OUTPUT));
        let mut any = false;
        for e in elems {
            let a = addr(kAudioDevicePropertyVolumeScalar, SCOPE_OUTPUT, e);
            if settable(id, a) {
                set(id, a, &v).map_err(BackendError)?;
                any = true;
            }
        }
        if any {
            Ok(())
        } else {
            Err(BackendError("This output has no software volume. Use the device's own controls.".into()))
        }
    }

    fn set_device_mute(&mut self, uid: &str, muted: bool) -> BResult<()> {
        let id = self.dev(uid)?;
        let a = addr(kAudioDevicePropertyMute, SCOPE_OUTPUT, ELEMENT_MAIN);
        if !settable(id, a) {
            return Err(BackendError("This output has no mute control.".into()));
        }
        set(id, a, &(muted as u32)).map_err(BackendError)
    }

    fn set_device_channel_volume(&mut self, uid: &str, channel: u16, volume: f32) -> BResult<()> {
        let id = self.dev(uid)?;
        let a = addr(kAudioDevicePropertyVolumeScalar, SCOPE_OUTPUT, channel as u32 + 1);
        if !settable(id, a) {
            return Err(BackendError("This output has no per-channel volume.".into()));
        }
        set(id, a, &volume.clamp(0.0, 1.0)).map_err(BackendError)
    }

    fn set_default_device(&mut self, uid: &str) -> BResult<()> {
        let id = self.dev(uid)?;
        set(kAudioObjectSystemObject, addr(kAudioHardwarePropertyDefaultOutputDevice, SCOPE_GLOBAL, ELEMENT_MAIN), &id).map_err(BackendError)
    }

    // App volume/mute live in the engine on macOS (soft_app_mixer); the mixer never calls these.
    fn set_source_volume(&mut self, _id: &str, _v: f32) -> BResult<()> {
        Ok(())
    }
    fn set_source_mute(&mut self, _id: &str, _m: bool) -> BResult<()> {
        Ok(())
    }
    fn set_source_channel_volume(&mut self, _id: &str, _c: u16, _v: f32) -> BResult<()> {
        Ok(())
    }

    fn dsp_support(&mut self) -> DspSupport {
        let method = "Core Audio process taps (muted while tapped)".to_string();
        if self.version < MIN_MACOS || objc::class(c"CATapDescription").is_none() {
            return DspSupport {
                available: false,
                reason: Some(format!("EQ and app volume need macOS {}.{} or newer. This Mac runs {}.", MIN_MACOS.0, MIN_MACOS.1, self.os)),
                method,
                needs_sink: false,
                soft_app_mixer: true,
                hint: None,
            };
        }
        DspSupport {
            available: true,
            reason: None,
            method,
            needs_sink: false,
            soft_app_mixer: true,
            hint: Some(
                "macOS asks once to let OmniMix record system audio. Allow it, or processed apps go silent. Change it any time in System Settings > Privacy & Security > Screen & System Audio Recording.".into(),
            ),
        }
    }

    fn open_output(&mut self, device: &Device, _latency: LatencyMode) -> BResult<Arc<BusControls>> {
        self.outputs.remove(&device.id);
        let id = self.dev(&device.id)?;
        let rate = get::<f64>(id, addr(kAudioDevicePropertyNominalSampleRate, SCOPE_GLOBAL, ELEMENT_MAIN)).unwrap_or(48_000.0).round() as u32;
        let ctl = BusControls::new(&device.id, rate, 2);
        let out = Output::start(id, ctl.clone()).map_err(BackendError)?;
        self.outputs.insert(device.id.clone(), out);
        Ok(ctl)
    }

    fn close_output(&mut self, device_id: &str) {
        self.outputs.remove(device_id);
    }

    fn divert(&mut self, source: &Source, target: &Device, _sink: Option<&Device>, tap: TapRequest) -> BResult<()> {
        if self.taps.contains_key(&source.pid) {
            return Ok(());
        }
        let obj = process_object(source.pid as i32).ok_or_else(|| BackendError("App is no longer playing".into()))?;
        let dev = self.dev(&target.id)?;
        let t = Tap::start(source.pid, obj, dev, &target.id, tap).map_err(BackendError)?;
        self.taps.insert(source.pid, t);
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

impl Drop for MacBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}
