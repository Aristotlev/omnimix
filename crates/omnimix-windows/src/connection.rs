#![allow(non_upper_case_globals)]
//! Physical connection detection for a render endpoint:
//! USB-C vs USB (hub port connector properties), Bluetooth profile,
//! AUX jack type/location/color, HDMI vs DisplayPort, S/PDIF, built-in, virtual.

use std::mem::size_of;

use omnimix_core::model::{ConnectionInfo, ConnectionKind, FormFactor, UsbIdent};
use windows::core::Interface;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Get_Device_IDW, CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW,
    CM_Get_Device_Interface_PropertyW, CM_Get_Parent, CM_Locate_DevNodeW, CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
    CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_EnumeratorName;
use windows::Win32::Devices::Properties::{DEVPKEY_Device_Address, DEVPKEY_Device_InstanceId, DEVPROPTYPE};
use windows::Win32::Devices::Usb::{
    GUID_DEVINTERFACE_USB_HUB, IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX, IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES,
    USB_NODE_CONNECTION_INFORMATION_EX, USB_PORT_CONNECTOR_PROPERTIES,
};
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE};
use windows::Win32::Media::Audio::{IDeviceTopology, IMMDevice, IPart, PKEY_AudioEndpoint_JackSubType};
use windows::Win32::Media::KernelStreaming::*;
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::Com::{CLSCTX_ALL, CLSCTX_INPROC_SERVER};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

use crate::util::{from_wide_buf, prop_string, take_pwstr, WStr};

#[derive(Clone, Debug, Default)]
pub struct Jack {
    pub connection_type: i32,
    pub geo: i32,
    pub port: i32,
    pub color: u32,
    pub connected: bool,
}

#[derive(Clone, Debug, Default)]
pub struct UsbPort {
    pub port: u32,
    pub type_c: Option<bool>,
    pub speed: Option<u8>,
    pub bcd_usb: u16,
    pub vid: u16,
    pub pid: u16,
}

/// Raw facts gathered for one endpoint (also printed by omnimix-probe).
#[derive(Clone, Debug, Default)]
pub struct EndpointFacts {
    pub enumerator: Option<String>,
    pub filter_path: Option<String>,
    pub instance_id: Option<String>,
    pub jack_subtype: Option<String>,
    pub jacks: Vec<Jack>,
    pub usb: Option<UsbPort>,
}

pub fn gather(dev: &IMMDevice, store: &IPropertyStore) -> EndpointFacts {
    let mut f = EndpointFacts {
        enumerator: prop_string(store, &PKEY_Device_EnumeratorName).map(|s| s.to_uppercase()),
        jack_subtype: prop_string(store, &PKEY_AudioEndpoint_JackSubType),
        ..Default::default()
    };
    unsafe {
        if let Ok(topo) = dev.Activate::<IDeviceTopology>(CLSCTX_ALL, None) {
            if let Ok(conn) = topo.GetConnector(0) {
                if let Ok(to) = conn.GetConnectedTo() {
                    if let Ok(part) = to.cast::<IPart>() {
                        f.jacks = jacks(&part);
                        if let Ok(ft) = part.GetTopologyObject() {
                            if let Ok(p) = ft.GetDeviceId() {
                                f.filter_path = Some(take_pwstr(p));
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(path) = &f.filter_path {
        f.instance_id = interface_instance_id(path);
    }
    if f.bus().as_deref() == Some("USB") {
        if let Some(id) = &f.instance_id {
            f.usb = usb_port(id);
        }
    }
    f
}

impl EndpointFacts {
    /// Bus token: prefix of the PnP instance id ("USB", "HDAUDIO", "BTHENUM", ...).
    pub fn bus(&self) -> Option<String> {
        self.instance_id
            .as_ref()
            .and_then(|id| id.split('\\').next().map(|s| s.to_uppercase()))
            .or_else(|| self.enumerator.clone())
    }
}

unsafe fn jacks(part: &IPart) -> Vec<Jack> {
    let mut raw = std::ptr::null_mut();
    if part.Activate(CLSCTX_INPROC_SERVER.0, &IKsJackDescription::IID, Some(&mut raw)).is_err() || raw.is_null() {
        return vec![];
    }
    let jd = IKsJackDescription::from_raw(raw);
    let n = jd.GetJackCount().unwrap_or(0).min(16);
    let mut out = vec![];
    for i in 0..n {
        let mut d = KSJACK_DESCRIPTION::default();
        if jd.GetJackDescription(i, &mut d).is_ok() {
            out.push(Jack {
                connection_type: d.ConnectionType.0,
                geo: d.GeoLocation.0,
                port: d.PortConnection.0,
                color: d.Color & 0x00FF_FFFF,
                connected: d.IsConnected.as_bool(),
            });
        }
    }
    out
}

fn interface_instance_id(path: &str) -> Option<String> {
    let p = WStr::new(path);
    let mut ty = DEVPROPTYPE::default();
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe { CM_Get_Device_Interface_PropertyW(p.pcwstr(), &DEVPKEY_Device_InstanceId, &mut ty, Some(buf.as_mut_ptr() as *mut u8), &mut size, 0) };
    (r == CR_SUCCESS).then(|| from_wide_buf(&buf))
}

fn devnode_id(dn: u32) -> Option<String> {
    let mut buf = [0u16; 512];
    let r = unsafe { CM_Get_Device_IDW(dn, &mut buf, 0) };
    (r == CR_SUCCESS).then(|| from_wide_buf(&buf))
}

fn devnode_u32(dn: u32, key: &windows::Win32::Foundation::DEVPROPKEY) -> Option<u32> {
    let mut ty = DEVPROPTYPE::default();
    let mut v = 0u32;
    let mut size = 4u32;
    let r = unsafe { CM_Get_DevNode_PropertyW(dn, key, &mut ty, Some(&mut v as *mut u32 as *mut u8), &mut size, 0) };
    (r == CR_SUCCESS && size == 4).then_some(v)
}

fn hub_interface(instance_id: &str) -> Option<String> {
    let id = WStr::new(instance_id);
    let mut len = 0u32;
    unsafe {
        if CM_Get_Device_Interface_List_SizeW(&mut len, &GUID_DEVINTERFACE_USB_HUB, id.pcwstr(), CM_GET_DEVICE_INTERFACE_LIST_PRESENT) != CR_SUCCESS || len <= 1 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        if CM_Get_Device_Interface_ListW(&GUID_DEVINTERFACE_USB_HUB, id.pcwstr(), &mut buf, CM_GET_DEVICE_INTERFACE_LIST_PRESENT) != CR_SUCCESS {
            return None;
        }
        let s = from_wide_buf(&buf);
        (!s.is_empty()).then_some(s)
    }
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Walk from the audio function's devnode up to the device attached to a hub
/// port, then ask the hub about that port.
pub fn usb_port(instance_id: &str) -> Option<UsbPort> {
    let id = WStr::new(instance_id);
    let mut dn = 0u32;
    unsafe {
        if CM_Locate_DevNodeW(&mut dn, id.pcwstr(), CM_LOCATE_DEVNODE_NORMAL) != CR_SUCCESS {
            return None;
        }
        for _ in 0..8 {
            let mut parent = 0u32;
            if CM_Get_Parent(&mut parent, dn, 0) != CR_SUCCESS {
                return None;
            }
            let parent_id = devnode_id(parent)?;
            if let Some(hub) = hub_interface(&parent_id) {
                let port = devnode_u32(dn, &DEVPKEY_Device_Address)?;
                return Some(query_hub(&hub, port));
            }
            dn = parent;
        }
    }
    None
}

fn query_hub(hub_path: &str, port: u32) -> UsbPort {
    let mut out = UsbPort { port, ..Default::default() };
    let p = WStr::new(hub_path);
    let h = unsafe { CreateFileW(p.pcwstr(), GENERIC_WRITE.0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) };
    let Ok(h) = h else { return out };
    let h = Handle(h);
    unsafe {
        // Port connector properties: bit 3 of UsbPortProperties = PortConnectorIsTypeC.
        let mut buf = [0u8; 1024];
        let props = USB_PORT_CONNECTOR_PROPERTIES { ConnectionIndex: port, ..Default::default() };
        std::ptr::copy_nonoverlapping(&props as *const _ as *const u8, buf.as_mut_ptr(), size_of::<USB_PORT_CONNECTOR_PROPERTIES>());
        let mut ret = 0u32;
        if DeviceIoControl(
            h.0,
            IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES,
            Some(buf.as_ptr() as *const _),
            size_of::<USB_PORT_CONNECTOR_PROPERTIES>() as u32,
            Some(buf.as_mut_ptr() as *mut _),
            buf.len() as u32,
            Some(&mut ret),
            None,
        )
        .is_ok()
            && ret as usize >= 12
        {
            let r: USB_PORT_CONNECTOR_PROPERTIES = std::ptr::read_unaligned(buf.as_ptr() as *const _);
            let bits = r.UsbPortProperties.ul;
            out.type_c = Some(bits & (1 << 3) != 0);
        }

        // Connection info: speed + device descriptor.
        let mut buf = [0u8; 2048];
        let info = USB_NODE_CONNECTION_INFORMATION_EX { ConnectionIndex: port, ..Default::default() };
        std::ptr::copy_nonoverlapping(&info as *const _ as *const u8, buf.as_mut_ptr(), size_of::<USB_NODE_CONNECTION_INFORMATION_EX>());
        let mut ret = 0u32;
        if DeviceIoControl(
            h.0,
            IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX,
            Some(buf.as_ptr() as *const _),
            buf.len() as u32,
            Some(buf.as_mut_ptr() as *mut _),
            buf.len() as u32,
            Some(&mut ret),
            None,
        )
        .is_ok()
        {
            let r: USB_NODE_CONNECTION_INFORMATION_EX = std::ptr::read_unaligned(buf.as_ptr() as *const _);
            let desc = r.DeviceDescriptor;
            out.speed = Some(r.Speed);
            out.bcd_usb = desc.bcdUSB;
            out.vid = desc.idVendor;
            out.pid = desc.idProduct;
        }
    }
    out
}

fn conn_type_label(t: i32) -> Option<&'static str> {
    match EPcxConnectionType(t) {
        eConnType3Point5mm => Some("3.5 mm"),
        eConnTypeQuarter => Some("6.3 mm"),
        eConnTypeRCA => Some("RCA"),
        eConnTypeOptical => Some("Optical"),
        eConnTypeOtherDigital => Some("Coaxial"),
        eConnTypeOtherAnalog => Some("Analog"),
        eConnTypeXlrProfessional => Some("XLR"),
        eConnTypeCombination => Some("Combo jack"),
        eConnTypeMultichannelAnalogDIN => Some("DIN"),
        _ => None,
    }
}

fn geo_label(g: i32) -> Option<&'static str> {
    match EPcxGeoLocation(g) {
        eGeoLocFront => Some("Front panel"),
        eGeoLocRear | eGeoLocRearPanel => Some("Rear panel"),
        eGeoLocLeft => Some("Left side"),
        eGeoLocRight => Some("Right side"),
        eGeoLocTop => Some("Top"),
        eGeoLocBottom => Some("Bottom"),
        eGeoLocInsideMobileLid | eGeoLocOutsideMobileLid => Some("Lid"),
        eGeoLocHDMI => Some("HDMI"),
        _ => None,
    }
}

fn usb_chips(u: &UsbPort) -> Vec<String> {
    let mut v = vec![format!("Port {}", u.port)];
    let ver = match u.bcd_usb {
        0 => None,
        b => Some(format!("USB {}.{}", b >> 8, (b >> 4) & 0xF)),
    };
    let speed = match u.speed {
        Some(0) => Some("Low-Speed"),
        Some(1) => Some("Full-Speed"),
        Some(2) => Some("Hi-Speed"),
        Some(3) => Some("SuperSpeed"),
        Some(4) | Some(5) => Some("SuperSpeed+"),
        _ => None,
    };
    match (ver, speed) {
        (Some(a), Some(b)) => v.push(format!("{a} {b}")),
        (Some(a), None) => v.push(a),
        (None, Some(b)) => v.push(b.to_string()),
        _ => {}
    }
    v
}

fn guid_eq(s: &str, g: &windows::core::GUID) -> bool {
    s.trim_matches(|c| c == '{' || c == '}').eq_ignore_ascii_case(&format!("{g:?}"))
}

pub fn classify(f: &EndpointFacts, form: FormFactor, name: &str) -> ConnectionInfo {
    let bus = f.bus();
    let lname = name.to_lowercase();
    let inst = f.instance_id.clone().unwrap_or_default().to_uppercase();
    let mut info = ConnectionInfo { bus: bus.clone(), ..Default::default() };
    let b = bus.as_deref().unwrap_or("");
    let jack = f.jacks.iter().find(|j| j.connected).or(f.jacks.first()).cloned();
    let subtype = f.jack_subtype.clone().unwrap_or_default();

    if b.starts_with("BTH") {
        info.kind = ConnectionKind::Bluetooth;
        let profile = if b == "BTHHFENUM" {
            "Hands-free"
        } else if b.starts_with("BTHLE") {
            "LE Audio"
        } else if inst.contains("0000110B") {
            "A2DP stereo"
        } else {
            "Stereo"
        };
        info.detail.push(profile.into());
    } else if b == "USB" {
        match &f.usb {
            Some(u) => {
                info.kind = if u.type_c == Some(true) { ConnectionKind::UsbC } else { ConnectionKind::Usb };
                info.detail = usb_chips(u);
                if u.vid != 0 {
                    info.usb = Some(UsbIdent { vid: u.vid, pid: u.pid });
                }
            }
            None => info.kind = ConnectionKind::Usb,
        }
    } else if form == FormFactor::Display || guid_eq(&subtype, &KSNODETYPE_HDMI_INTERFACE) || guid_eq(&subtype, &KSNODETYPE_DISPLAYPORT_INTERFACE) {
        info.kind = ConnectionKind::Display;
        let port = if guid_eq(&subtype, &KSNODETYPE_DISPLAYPORT_INTERFACE) || lname.contains("displayport") {
            "DisplayPort"
        } else if guid_eq(&subtype, &KSNODETYPE_HDMI_INTERFACE) || lname.contains("hdmi") {
            "HDMI"
        } else {
            "HDMI / DisplayPort"
        };
        info.detail.push(port.into());
    } else if form == FormFactor::Spdif
        || jack.as_ref().map(|j| matches!(EPcxConnectionType(j.connection_type), eConnTypeOptical | eConnTypeOtherDigital)).unwrap_or(false)
    {
        info.kind = ConnectionKind::Spdif;
        if let Some(l) = jack.as_ref().and_then(|j| conn_type_label(j.connection_type)) {
            info.detail.push(l.into());
        }
    } else if form == FormFactor::Network {
        info.kind = ConnectionKind::Network;
    } else if matches!(b, "ROOT" | "SWD" | "SW" | "SWC") || lname.contains("virtual") || lname.contains("cable input") {
        info.kind = ConnectionKind::Virtual;
        info.detail.push("Virtual".into());
    } else if let Some(j) = &jack {
        if EPxcPortConnection(j.port) == ePortConnIntegratedDevice {
            info.kind = ConnectionKind::BuiltIn;
        } else {
            info.kind = ConnectionKind::Aux;
            if let Some(l) = conn_type_label(j.connection_type) {
                info.detail.push(l.into());
            }
            if let Some(g) = geo_label(j.geo) {
                info.detail.push(g.into());
            }
            if j.color != 0 {
                info.jack_color = Some(j.color);
            }
        }
    } else if matches!(b, "HDAUDIO" | "INTELAUDIO" | "ACPI" | "ACP" | "PCI") {
        info.kind = if form == FormFactor::Speakers { ConnectionKind::BuiltIn } else { ConnectionKind::Aux };
    }
    info
}
