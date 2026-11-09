//! Small helpers: wide strings, COM strings, PROPVARIANT access, errors.

use omnimix_core::backend::BackendError;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc, PROPVARIANT};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::{VARENUM, VT_BLOB, VT_EMPTY, VT_UI4};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Owns a NUL-terminated UTF-16 buffer and lends it out as PCWSTR.
pub struct WStr(Vec<u16>);

impl WStr {
    pub fn new(s: &str) -> WStr {
        WStr(wide(s))
    }
    pub fn pcwstr(&self) -> PCWSTR {
        PCWSTR(self.0.as_ptr())
    }
}

/// Convert a CoTaskMem-allocated PWSTR into a String and free it.
pub unsafe fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

pub fn from_wide_buf(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Initialise COM (MTA) on the calling thread. Harmless if already initialised.
pub fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

pub fn werr(ctx: &str, e: windows::core::Error) -> BackendError {
    BackendError(format!("{ctx}: {} (0x{:08X})", e.message().trim(), e.code().0 as u32))
}

pub trait Ctx<T> {
    fn ctx(self, what: &str) -> Result<T, BackendError>;
}

impl<T> Ctx<T> for windows::core::Result<T> {
    fn ctx(self, what: &str) -> Result<T, BackendError> {
        self.map_err(|e| werr(what, e))
    }
}

/// PROPVARIANT that clears itself.
pub struct PropVar(pub PROPVARIANT);

impl Drop for PropVar {
    fn drop(&mut self) {
        unsafe {
            let _ = PropVariantClear(&mut self.0);
        }
    }
}

impl PropVar {
    pub fn vt(&self) -> VARENUM {
        unsafe { self.0.Anonymous.Anonymous.vt }
    }

    pub fn string(&self) -> Option<String> {
        if self.vt() == VT_EMPTY {
            return None;
        }
        unsafe { PropVariantToStringAlloc(&self.0).ok().map(|p| take_pwstr(p)).filter(|s| !s.is_empty()) }
    }

    pub fn u32(&self) -> Option<u32> {
        if self.vt() == VT_UI4 {
            Some(unsafe { self.0.Anonymous.Anonymous.Anonymous.ulVal })
        } else {
            None
        }
    }

    pub fn blob(&self) -> Option<Vec<u8>> {
        if self.vt() != VT_BLOB {
            return None;
        }
        unsafe {
            let b = &self.0.Anonymous.Anonymous.Anonymous.blob;
            if b.pBlobData.is_null() || b.cbSize == 0 {
                return None;
            }
            Some(std::slice::from_raw_parts(b.pBlobData, b.cbSize as usize).to_vec())
        }
    }
}

pub fn prop(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<PropVar> {
    unsafe { store.GetValue(key).ok().map(PropVar) }
}

pub fn prop_string(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<String> {
    prop(store, key).and_then(|p| p.string())
}

pub fn prop_u32(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<u32> {
    prop(store, key).and_then(|p| p.u32())
}

/// Parsed WAVEFORMATEX / WAVEFORMATEXTENSIBLE header bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct WaveInfo {
    pub tag: u16,
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
    pub valid_bits: u16,
    pub mask: u32,
}

pub fn parse_wave(b: &[u8]) -> Option<WaveInfo> {
    if b.len() < 16 {
        return None;
    }
    let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let mut w = WaveInfo { tag: u16_at(0), channels: u16_at(2), rate: u32_at(4), bits: u16_at(14), valid_bits: 0, mask: 0 };
    if w.tag == 0xFFFE && b.len() >= 24 {
        w.valid_bits = u16_at(18);
        w.mask = u32_at(20);
    }
    Some(w)
}
