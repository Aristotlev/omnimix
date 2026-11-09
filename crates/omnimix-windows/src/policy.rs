//! Undocumented-but-stable Windows policy interfaces, called through raw vtables:
//! * IPolicyConfig::SetDefaultEndpoint (system default device; used by the
//!   Sound control panel since Windows 7).
//! * IAudioPolicyConfigFactory per-app default endpoint (what Settings >
//!   Sound > Volume mixer > App output uses, Windows 10 1803+).

use std::ffi::c_void;

use windows::core::{IUnknown, Interface, GUID, HRESULT, HSTRING, PCWSTR};
use windows::Win32::Media::Audio::{eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::WinRT::RoGetActivationFactory;

use crate::util::WStr;

const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);

#[repr(C)]
struct PolicyConfigVtbl {
    _iunknown: [usize; 3],
    _get_mix_format_through_set_property_value: [usize; 10],
    set_default_endpoint: unsafe extern "system" fn(*mut c_void, PCWSTR, ERole) -> HRESULT,
}

/// Make `endpoint_id` the default playback device for all roles.
pub fn set_default_endpoint(endpoint_id: &str) -> windows::core::Result<()> {
    unsafe {
        let unk: IUnknown = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)?;
        let mut raw = std::ptr::null_mut();
        unk.query(&IID_POLICY_CONFIG, &mut raw).ok()?;
        let obj = IUnknown::from_raw(raw);
        let vt = &**(raw as *const *const PolicyConfigVtbl);
        let id = WStr::new(endpoint_id);
        for role in [eConsole, eMultimedia, eCommunications] {
            (vt.set_default_endpoint)(raw, id.pcwstr(), role).ok()?;
        }
        drop(obj);
        Ok(())
    }
}

const IID_APC_21H2: GUID = GUID::from_u128(0xab3d4648_e242_459f_b02f_541c70306324);
const IID_APC_DOWNLEVEL: GUID = GUID::from_u128(0x2a59116d_6c4f_45e0_a74f_707e3fef9258);

#[repr(C)]
struct AppPolicyVtbl {
    _iinspectable: [usize; 6],
    _placeholders: [usize; 19],
    set_persisted: unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut c_void) -> HRESULT,
    get_persisted: unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut *mut c_void) -> HRESULT,
    _clear_all: unsafe extern "system" fn(*mut c_void) -> HRESULT,
}

const MMDEVAPI_TOKEN: &str = r"\\?\SWD#MMDEVAPI#";
const DEVINTERFACE_AUDIO_RENDER: &str = "#{e6327cad-dcec-4949-ae8a-991e976a79d2}";

/// Per-app output routing ("App volume and device preferences").
pub struct AppRouting {
    _obj: IUnknown,
    raw: *mut c_void,
}

impl AppRouting {
    pub fn new() -> windows::core::Result<AppRouting> {
        unsafe {
            let name = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
            let factory: IUnknown = RoGetActivationFactory(&name)?;
            let mut last = HRESULT(0);
            for iid in [IID_APC_21H2, IID_APC_DOWNLEVEL] {
                let mut raw = std::ptr::null_mut();
                let hr = factory.query(&iid, &mut raw);
                if hr.is_ok() && !raw.is_null() {
                    return Ok(AppRouting { _obj: IUnknown::from_raw(raw), raw });
                }
                last = hr;
            }
            Err(windows::core::Error::from(last))
        }
    }

    fn vt(&self) -> &AppPolicyVtbl {
        unsafe { &**(self.raw as *const *const AppPolicyVtbl) }
    }

    /// Route every render stream of `pid`'s app to `endpoint_id` (None = follow the system default).
    pub fn set(&self, pid: u32, endpoint_id: Option<&str>) -> windows::core::Result<()> {
        let h = endpoint_id.map(|id| HSTRING::from(format!("{MMDEVAPI_TOKEN}{id}{DEVINTERFACE_AUDIO_RENDER}")));
        let raw_h: *mut c_void = match &h {
            Some(h) => unsafe { std::mem::transmute_copy::<HSTRING, *mut c_void>(h) },
            None => std::ptr::null_mut(),
        };
        unsafe {
            (self.vt().set_persisted)(self.raw, pid, eRender, eMultimedia, raw_h).ok()?;
            (self.vt().set_persisted)(self.raw, pid, eRender, eConsole, raw_h).ok()?;
        }
        drop(h);
        Ok(())
    }

    /// The app's own persisted output, if it has one (None = follows default).
    pub fn get(&self, pid: u32) -> windows::core::Result<Option<String>> {
        let mut out: *mut c_void = std::ptr::null_mut();
        unsafe {
            (self.vt().get_persisted)(self.raw, pid, eRender, eMultimedia, &mut out).ok()?;
            if out.is_null() {
                return Ok(None);
            }
            let h: HSTRING = std::mem::transmute::<*mut c_void, HSTRING>(out);
            let mut s = h.to_string();
            if let Some(x) = s.strip_prefix(MMDEVAPI_TOKEN) {
                s = x.to_string();
            }
            if let Some(x) = s.strip_suffix(DEVINTERFACE_AUDIO_RENDER) {
                s = x.to_string();
            }
            Ok((!s.is_empty()).then_some(s))
        }
    }
}

// The raw pointer is only used on the thread that created it (the mixer thread).
unsafe impl Send for AppRouting {}
