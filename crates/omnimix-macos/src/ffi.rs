//! Hand-written Core Audio HAL / CoreFoundation bindings (no SDK headers needed)
//! plus typed property helpers.
#![allow(non_snake_case, non_upper_case_globals, dead_code)]

use std::ffi::{c_char, c_void};
use std::mem::{size_of, MaybeUninit};

pub type OSStatus = i32;
pub type AudioObjectID = u32;

pub const fn fcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
}

pub const kAudioObjectSystemObject: AudioObjectID = 1;
pub const kAudioObjectUnknown: AudioObjectID = 0;
pub const SCOPE_GLOBAL: u32 = fcc(b"glob");
pub const SCOPE_OUTPUT: u32 = fcc(b"outp");
pub const SCOPE_INPUT: u32 = fcc(b"inpt");
pub const ELEMENT_MAIN: u32 = 0;

// System object
pub const kAudioHardwarePropertyDevices: u32 = fcc(b"dev#");
pub const kAudioHardwarePropertyDefaultOutputDevice: u32 = fcc(b"dOut");
pub const kAudioHardwarePropertyProcessObjectList: u32 = fcc(b"prs#");
pub const kAudioHardwarePropertyTranslatePIDToProcessObject: u32 = fcc(b"id2p");
// Process objects
pub const kAudioProcessPropertyPID: u32 = fcc(b"ppid");
pub const kAudioProcessPropertyBundleID: u32 = fcc(b"pbid");
pub const kAudioProcessPropertyIsRunningOutput: u32 = fcc(b"piro");
pub const kAudioProcessPropertyDevices: u32 = fcc(b"pdv#");
// Objects / devices
pub const kAudioObjectPropertyName: u32 = fcc(b"lnam");
pub const kAudioObjectPropertyManufacturer: u32 = fcc(b"lmak");
pub const kAudioDevicePropertyDeviceUID: u32 = fcc(b"uid ");
pub const kAudioDevicePropertyTransportType: u32 = fcc(b"tran");
pub const kAudioDevicePropertyStreams: u32 = fcc(b"stm#");
pub const kAudioDevicePropertyStreamConfiguration: u32 = fcc(b"slay");
pub const kAudioDevicePropertyNominalSampleRate: u32 = fcc(b"nsrt");
pub const kAudioDevicePropertyVolumeScalar: u32 = fcc(b"volm");
pub const kAudioDevicePropertyMute: u32 = fcc(b"mute");
pub const kAudioDevicePropertyDataSource: u32 = fcc(b"ssrc");
pub const kAudioDevicePropertyIsHidden: u32 = fcc(b"hidn");
pub const kAudioDevicePropertyDeviceIsAlive: u32 = fcc(b"livn");
pub const kAudioDevicePropertyBufferFrameSize: u32 = fcc(b"fsiz");
pub const kAudioDevicePropertyLatency: u32 = fcc(b"ltnc");
pub const kAudioStreamPropertyPhysicalFormat: u32 = fcc(b"pft ");
// Taps
pub const kAudioTapPropertyFormat: u32 = fcc(b"tfmt");

// Transport types
pub const TRANSPORT_BUILTIN: u32 = fcc(b"bltn");
pub const TRANSPORT_AGGREGATE: u32 = fcc(b"grup");
pub const TRANSPORT_VIRTUAL: u32 = fcc(b"virt");
pub const TRANSPORT_PCI: u32 = fcc(b"pci ");
pub const TRANSPORT_USB: u32 = fcc(b"usb ");
pub const TRANSPORT_FIREWIRE: u32 = fcc(b"1394");
pub const TRANSPORT_BLUETOOTH: u32 = fcc(b"blue");
pub const TRANSPORT_BLUETOOTH_LE: u32 = fcc(b"blea");
pub const TRANSPORT_HDMI: u32 = fcc(b"hdmi");
pub const TRANSPORT_DISPLAYPORT: u32 = fcc(b"dprt");
pub const TRANSPORT_AIRPLAY: u32 = fcc(b"airp");
pub const TRANSPORT_AVB: u32 = fcc(b"eavb");
pub const TRANSPORT_THUNDERBOLT: u32 = fcc(b"thun");

// Built-in data sources
pub const SOURCE_INTERNAL_SPEAKER: u32 = fcc(b"ispk");
pub const SOURCE_HEADPHONES: u32 = fcc(b"hdpn");

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AudioObjectPropertyAddress {
    pub mSelector: u32,
    pub mScope: u32,
    pub mElement: u32,
}

pub const fn addr(sel: u32, scope: u32, element: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: sel, mScope: scope, mElement: element }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AudioBuffer {
    pub mNumberChannels: u32,
    pub mDataByteSize: u32,
    pub mData: *mut c_void,
}

#[repr(C)]
pub struct AudioBufferList {
    pub mNumberBuffers: u32,
    pub mBuffers: [AudioBuffer; 1],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioStreamBasicDescription {
    pub mSampleRate: f64,
    pub mFormatID: u32,
    pub mFormatFlags: u32,
    pub mBytesPerPacket: u32,
    pub mFramesPerPacket: u32,
    pub mBytesPerFrame: u32,
    pub mChannelsPerFrame: u32,
    pub mBitsPerChannel: u32,
    pub mReserved: u32,
}

pub type AudioDeviceIOProc = unsafe extern "C" fn(
    in_device: AudioObjectID,
    in_now: *const c_void,
    in_input: *const AudioBufferList,
    in_input_time: *const c_void,
    out_output: *mut AudioBufferList,
    in_output_time: *const c_void,
    client: *mut c_void,
) -> OSStatus;

pub type AudioDeviceIOProcID = Option<AudioDeviceIOProc>;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    pub fn AudioObjectHasProperty(id: AudioObjectID, address: *const AudioObjectPropertyAddress) -> u8;
    pub fn AudioObjectIsPropertySettable(id: AudioObjectID, address: *const AudioObjectPropertyAddress, out: *mut u8) -> OSStatus;
    pub fn AudioObjectGetPropertyDataSize(id: AudioObjectID, address: *const AudioObjectPropertyAddress, qsize: u32, q: *const c_void, out: *mut u32) -> OSStatus;
    pub fn AudioObjectGetPropertyData(id: AudioObjectID, address: *const AudioObjectPropertyAddress, qsize: u32, q: *const c_void, io_size: *mut u32, out: *mut c_void) -> OSStatus;
    pub fn AudioObjectSetPropertyData(id: AudioObjectID, address: *const AudioObjectPropertyAddress, qsize: u32, q: *const c_void, size: u32, data: *const c_void) -> OSStatus;
    pub fn AudioDeviceCreateIOProcID(dev: AudioObjectID, proc_: AudioDeviceIOProc, client: *mut c_void, out: *mut AudioDeviceIOProcID) -> OSStatus;
    pub fn AudioDeviceDestroyIOProcID(dev: AudioObjectID, id: AudioDeviceIOProcID) -> OSStatus;
    pub fn AudioDeviceStart(dev: AudioObjectID, id: AudioDeviceIOProcID) -> OSStatus;
    pub fn AudioDeviceStop(dev: AudioObjectID, id: AudioDeviceIOProcID) -> OSStatus;
    pub fn AudioHardwareCreateAggregateDevice(description: *const c_void, out: *mut AudioObjectID) -> OSStatus;
    pub fn AudioHardwareDestroyAggregateDevice(id: AudioObjectID) -> OSStatus;
    pub fn AudioHardwareCreateProcessTap(description: *mut c_void, out: *mut AudioObjectID) -> OSStatus;
    pub fn AudioHardwareDestroyProcessTap(id: AudioObjectID) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFRelease(p: *const c_void);
    pub fn CFStringGetLength(s: *const c_void) -> isize;
    pub fn CFStringGetCString(s: *const c_void, buf: *mut c_char, size: isize, encoding: u32) -> u8;
}

extern "C" {
    pub fn proc_pidpath(pid: i32, buffer: *mut c_void, size: u32) -> i32;
}

const UTF8: u32 = 0x0800_0100;

/// Human-readable OSStatus (four-char codes print as text).
pub fn status_text(st: OSStatus) -> String {
    let b = (st as u32).to_be_bytes();
    if b.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
        format!("'{}'", String::from_utf8_lossy(&b))
    } else {
        format!("{st}")
    }
}

pub fn check(st: OSStatus, what: &str) -> Result<(), String> {
    if st == 0 {
        Ok(())
    } else {
        Err(format!("{what} failed ({})", status_text(st)))
    }
}

pub fn has(id: AudioObjectID, a: AudioObjectPropertyAddress) -> bool {
    unsafe { AudioObjectHasProperty(id, &a) != 0 }
}

pub fn settable(id: AudioObjectID, a: AudioObjectPropertyAddress) -> bool {
    let mut out = 0u8;
    has(id, a) && unsafe { AudioObjectIsPropertySettable(id, &a, &mut out) } == 0 && out != 0
}

pub fn get<T: Copy>(id: AudioObjectID, a: AudioObjectPropertyAddress) -> Option<T> {
    get_q::<T, u8>(id, a, None)
}

pub fn get_q<T: Copy, Q>(id: AudioObjectID, a: AudioObjectPropertyAddress, q: Option<&Q>) -> Option<T> {
    let mut v = MaybeUninit::<T>::zeroed();
    let mut size = size_of::<T>() as u32;
    let (qs, qp) = match q {
        Some(q) => (size_of::<Q>() as u32, q as *const Q as *const c_void),
        None => (0, std::ptr::null()),
    };
    let st = unsafe { AudioObjectGetPropertyData(id, &a, qs, qp, &mut size, v.as_mut_ptr() as *mut c_void) };
    (st == 0 && size as usize == size_of::<T>()).then(|| unsafe { v.assume_init() })
}

pub fn get_vec<T: Copy>(id: AudioObjectID, a: AudioObjectPropertyAddress) -> Vec<T> {
    let mut size = 0u32;
    unsafe {
        if AudioObjectGetPropertyDataSize(id, &a, 0, std::ptr::null(), &mut size) != 0 || size == 0 {
            return vec![];
        }
        let n = size as usize / size_of::<T>();
        let mut v: Vec<T> = Vec::with_capacity(n);
        if AudioObjectGetPropertyData(id, &a, 0, std::ptr::null(), &mut size, v.as_mut_ptr() as *mut c_void) != 0 {
            return vec![];
        }
        v.set_len((size as usize / size_of::<T>()).min(n));
        v
    }
}

pub fn get_bytes(id: AudioObjectID, a: AudioObjectPropertyAddress) -> Vec<u8> {
    get_vec::<u8>(id, a)
}

pub fn set<T>(id: AudioObjectID, a: AudioObjectPropertyAddress, v: &T) -> Result<(), String> {
    let st = unsafe { AudioObjectSetPropertyData(id, &a, 0, std::ptr::null(), size_of::<T>() as u32, v as *const T as *const c_void) };
    check(st, "set property")
}

/// Convert an owned CFStringRef to a String and release it.
pub unsafe fn take_cfstring(s: *const c_void) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let len = CFStringGetLength(s);
    let cap = (len * 4 + 1).max(1) as usize;
    let mut buf = vec![0 as c_char; cap];
    let ok = CFStringGetCString(s, buf.as_mut_ptr(), cap as isize, UTF8);
    CFRelease(s);
    if ok == 0 {
        return None;
    }
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn get_string(id: AudioObjectID, a: AudioObjectPropertyAddress) -> Option<String> {
    let p: *const c_void = get(id, a)?;
    unsafe { take_cfstring(p) }.filter(|s| !s.is_empty())
}

/// Total channels in an AudioBufferList-typed property (stream configuration).
pub fn channel_count(id: AudioObjectID, scope: u32) -> u32 {
    let b = get_bytes(id, addr(kAudioDevicePropertyStreamConfiguration, scope, ELEMENT_MAIN));
    if b.len() < 4 {
        return 0;
    }
    let n = u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as usize;
    let mut total = 0u32;
    for i in 0..n {
        let off = 8 + i * size_of::<AudioBuffer>();
        if off + 4 > b.len() {
            break;
        }
        total += u32::from_ne_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]);
    }
    total
}

pub fn exe_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}
