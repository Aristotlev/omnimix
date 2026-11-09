//! Render endpoints: enumeration, static facts (cached), live volume/mute/channels.

use std::collections::HashMap;

use omnimix_core::model::{channel_labels, Channel, ConnectionInfo, Device, DeviceState, FormFactor, StreamFormat};
use windows::Win32::Devices::FunctionDiscovery::{PKEY_DeviceInterface_FriendlyName, PKEY_Device_DeviceDesc, PKEY_Device_FriendlyName};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL, STGM_READ};

use crate::connection::{classify, gather, EndpointFacts};
use crate::util::{parse_wave, prop, prop_string, prop_u32, take_pwstr, WaveInfo};

#[derive(Clone)]
pub struct StaticInfo {
    pub name: String,
    pub label: String,
    pub adapter: String,
    pub form: FormFactor,
    pub connection: ConnectionInfo,
    pub facts: EndpointFacts,
    pub mix: Option<WaveInfo>,
    pub device_format: Option<WaveInfo>,
}

pub struct Endpoint {
    pub device: IMMDevice,
    pub volume: Option<IAudioEndpointVolume>,
    pub meter: Option<IAudioMeterInformation>,
    pub info: StaticInfo,
    pub state: DeviceState,
}

pub fn form_factor(v: Option<u32>) -> FormFactor {
    match v {
        Some(0) => FormFactor::Network,
        Some(1) => FormFactor::Speakers,
        Some(2) => FormFactor::LineOut,
        Some(3) => FormFactor::Headphones,
        Some(5) => FormFactor::Headset,
        Some(6) => FormFactor::Handset,
        Some(8) => FormFactor::Spdif,
        Some(9) => FormFactor::Display,
        _ => FormFactor::Unknown,
    }
}

fn map_state(s: DEVICE_STATE) -> DeviceState {
    match s {
        DEVICE_STATE_ACTIVE => DeviceState::Active,
        DEVICE_STATE_UNPLUGGED => DeviceState::Unplugged,
        DEVICE_STATE_DISABLED => DeviceState::Disabled,
        _ => DeviceState::NotPresent,
    }
}

pub fn mix_format(dev: &IMMDevice) -> Option<WaveInfo> {
    unsafe {
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).ok()?;
        let p = client.GetMixFormat().ok()?;
        if p.is_null() {
            return None;
        }
        let len = 18 + (*p).cbSize as usize;
        let bytes = std::slice::from_raw_parts(p as *const u8, len.min(64)).to_vec();
        CoTaskMemFree(Some(p as *const _));
        parse_wave(&bytes)
    }
}

pub fn static_info(dev: &IMMDevice, active: bool) -> Option<StaticInfo> {
    unsafe {
        let store = dev.OpenPropertyStore(STGM_READ).ok()?;
        let name = prop_string(&store, &PKEY_Device_FriendlyName).unwrap_or_else(|| "Audio output".into());
        let label = prop_string(&store, &PKEY_Device_DeviceDesc).unwrap_or_else(|| name.clone());
        let adapter = prop_string(&store, &PKEY_DeviceInterface_FriendlyName).unwrap_or_default();
        let form = form_factor(prop_u32(&store, &PKEY_AudioEndpoint_FormFactor));
        let facts = gather(dev, &store);
        let connection = classify(&facts, form, &name);
        let device_format = prop(&store, &PKEY_AudioEngine_DeviceFormat).and_then(|p| p.blob()).and_then(|b| parse_wave(&b));
        let mix = if active { mix_format(dev) } else { None };
        Some(StaticInfo { name, label, adapter, form, connection, facts, mix, device_format })
    }
}

#[derive(Default)]
pub struct DeviceTable {
    pub endpoints: HashMap<String, Endpoint>,
    pub order: Vec<String>,
}

impl DeviceTable {
    /// Re-enumerate. Static facts are reused unless `invalidate` names the id.
    pub fn refresh(&mut self, en: &IMMDeviceEnumerator, invalidate: &dyn Fn(&str) -> bool) -> windows::core::Result<()> {
        let mut seen = vec![];
        unsafe {
            let coll = en.EnumAudioEndpoints(eRender, DEVICE_STATE(DEVICE_STATEMASK_ALL))?;
            let n = coll.GetCount()?;
            for i in 0..n {
                let Ok(dev) = coll.Item(i) else { continue };
                let Ok(idp) = dev.GetId() else { continue };
                let id = take_pwstr(idp);
                let state = map_state(dev.GetState().unwrap_or(DEVICE_STATE_NOTPRESENT));
                if state == DeviceState::NotPresent {
                    continue;
                }
                let active = state == DeviceState::Active;
                let reuse = self.endpoints.get(&id).map(|e| e.state == state && !invalidate(&id)).unwrap_or(false);
                if !reuse {
                    let Some(info) = static_info(&dev, active) else { continue };
                    let volume = if active { dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok() } else { None };
                    let meter = if active { dev.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None).ok() } else { None };
                    self.endpoints.insert(id.clone(), Endpoint { device: dev, volume, meter, info, state });
                }
                seen.push(id);
            }
        }
        self.endpoints.retain(|k, _| seen.contains(k));
        self.order = seen;
        Ok(())
    }

    pub fn to_device(&self, id: &str, default_id: Option<&str>, comms_id: Option<&str>) -> Option<Device> {
        let e = self.endpoints.get(id)?;
        let i = &e.info;
        let (mut volume, mut muted, mut channels) = (0.0, false, vec![]);
        let mask = i.mix.map(|m| m.mask).filter(|m| *m != 0).or(i.device_format.map(|d| d.mask)).unwrap_or(0);
        if let Some(v) = &e.volume {
            unsafe {
                volume = v.GetMasterVolumeLevelScalar().unwrap_or(0.0);
                muted = v.GetMute().map(|b| b.as_bool()).unwrap_or(false);
                let count = v.GetChannelCount().unwrap_or(0).min(16) as usize;
                for (ci, label) in channel_labels(mask, count).into_iter().enumerate() {
                    let cv = v.GetChannelVolumeLevelScalar(ci as u32).unwrap_or(1.0);
                    channels.push(Channel { index: ci as u16, label, volume: cv, muted: cv <= 0.0001 });
                }
            }
        }
        let format = match (i.mix, i.device_format) {
            (Some(m), d) => Some(StreamFormat {
                sample_rate: m.rate,
                channels: m.channels,
                bits: d.map(|d| if d.valid_bits != 0 { d.valid_bits } else { d.bits }).unwrap_or(0),
                channel_mask: mask,
            }),
            (None, Some(d)) => Some(StreamFormat { sample_rate: d.rate, channels: d.channels, bits: if d.valid_bits != 0 { d.valid_bits } else { d.bits }, channel_mask: mask }),
            _ => None,
        };
        Some(Device {
            id: id.to_string(),
            name: i.name.clone(),
            label: i.label.clone(),
            adapter: i.adapter.clone(),
            form_factor: i.form,
            connection: i.connection.clone(),
            state: e.state,
            is_default: default_id == Some(id),
            is_default_comms: comms_id == Some(id),
            format,
            volume,
            muted,
            channels,
        })
    }
}

pub fn default_id(en: &IMMDeviceEnumerator, role: ERole) -> Option<String> {
    unsafe { en.GetDefaultAudioEndpoint(eRender, role).ok().and_then(|d| d.GetId().ok()).map(|p| take_pwstr(p)) }
}
