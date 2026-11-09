//! C ABI over the universal core. Header: `include/omnimix.h`.
//! Any host on any OS can run the same EQ engine OmniMix uses.

use std::ffi::{c_char, CStr, CString};

use omnimix_core::dsp::presets::builtin_presets;
use omnimix_core::dsp::{EqChain, EqParams, EqSettings};

pub struct OmxEq {
    chain: EqChain,
    settings: EqSettings,
    sr: f64,
    ch: usize,
}

impl OmxEq {
    fn push(&mut self) {
        self.chain.set_params(EqParams::from_settings(&self.settings, self.sr));
    }
}

/// Library version, static NUL-terminated string.
#[no_mangle]
pub extern "C" fn omx_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// New EQ for `channels` interleaved channels at `sample_rate`. Starts enabled and flat.
#[no_mangle]
pub extern "C" fn omx_eq_new(sample_rate: f64, channels: u32) -> *mut OmxEq {
    if !(sample_rate > 1000.0) || channels == 0 || channels > 32 {
        return std::ptr::null_mut();
    }
    let ch = channels as usize;
    let mut e = Box::new(OmxEq { chain: EqChain::new(ch, 4096), settings: EqSettings { enabled: true, ..Default::default() }, sr: sample_rate, ch });
    e.push();
    Box::into_raw(e)
}

/// # Safety
/// `h` must come from `omx_eq_new` and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn omx_eq_free(h: *mut OmxEq) {
    if !h.is_null() {
        drop(Box::from_raw(h));
    }
}

/// Replace all settings with EqSettings JSON (same schema the app stores). 0 = ok, -1 = bad input.
///
/// # Safety
/// `h` valid, `json` NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn omx_eq_set_json(h: *mut OmxEq, json: *const c_char) -> i32 {
    let (Some(e), false) = (h.as_mut(), json.is_null()) else { return -1 };
    let Ok(s) = CStr::from_ptr(json).to_str() else { return -1 };
    match serde_json::from_str::<EqSettings>(s) {
        Ok(v) => {
            e.settings = v.sanitized();
            e.push();
            0
        }
        Err(_) => -1,
    }
}

/// Set the gain of band `index` (0..9 = Sub, Bass, Lows, Body, Mud, Mids, Claps, Presence, Hats, Air).
///
/// # Safety
/// `h` valid.
#[no_mangle]
pub unsafe extern "C" fn omx_eq_set_gain(h: *mut OmxEq, index: u32, gain_db: f32) -> i32 {
    let Some(e) = h.as_mut() else { return -1 };
    let Some(b) = e.settings.bands.get_mut(index as usize) else { return -1 };
    b.gain_db = gain_db;
    e.settings = e.settings.sanitized();
    e.push();
    0
}

/// Load a built-in preset by name ("Flat", "Bass Boost", "Punch", ...). 0 = ok.
///
/// # Safety
/// `h` valid, `name` NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn omx_eq_load_preset(h: *mut OmxEq, name: *const c_char) -> i32 {
    let (Some(e), false) = (h.as_mut(), name.is_null()) else { return -1 };
    let n = CStr::from_ptr(name).to_string_lossy();
    match builtin_presets().into_iter().find(|p| p.name.eq_ignore_ascii_case(&n)) {
        Some(p) => {
            e.settings = p.eq;
            e.push();
            0
        }
        None => -1,
    }
}

/// # Safety
/// `h` valid.
#[no_mangle]
pub unsafe extern "C" fn omx_eq_set_enabled(h: *mut OmxEq, enabled: bool) {
    if let Some(e) = h.as_mut() {
        e.settings.enabled = enabled;
        e.push();
    }
}

/// Process `frames` interleaved frames in place.
///
/// # Safety
/// `h` valid, `buf` holds `frames * channels` floats.
#[no_mangle]
pub unsafe extern "C" fn omx_eq_process(h: *mut OmxEq, buf: *mut f32, frames: u32) {
    let (Some(e), false) = (h.as_mut(), buf.is_null()) else { return };
    let ch = e.ch;
    let data = std::slice::from_raw_parts_mut(buf, frames as usize * ch);
    for chunk in data.chunks_mut(4096 * ch) {
        let n = chunk.len() / ch;
        e.chain.process(chunk, n);
    }
}

/// Magnitude response (dB) of the current curve at `n` frequencies.
///
/// # Safety
/// `h` valid, `freqs`/`out` hold `n` doubles.
#[no_mangle]
pub unsafe extern "C" fn omx_eq_response_db(h: *const OmxEq, freqs: *const f64, out: *mut f64, n: u32) {
    let (Some(e), false, false) = (h.as_ref(), freqs.is_null(), out.is_null()) else { return };
    let f = std::slice::from_raw_parts(freqs, n as usize);
    let r = e.settings.curve_db(f, e.sr);
    std::ptr::copy_nonoverlapping(r.as_ptr(), out, n as usize);
}

/// Current settings as JSON. Free with `omx_string_free`.
///
/// # Safety
/// `h` valid.
#[no_mangle]
pub unsafe extern "C" fn omx_eq_get_json(h: *const OmxEq) -> *mut c_char {
    let Some(e) = h.as_ref() else { return std::ptr::null_mut() };
    CString::new(serde_json::to_string(&e.settings).unwrap_or_default()).map(|c| c.into_raw()).unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `s` must come from this library.
#[no_mangle]
pub unsafe extern "C" fn omx_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_api_roundtrip() {
        unsafe {
            let h = omx_eq_new(48_000.0, 2);
            assert!(!h.is_null());
            assert_eq!(omx_eq_load_preset(h, c"Bass Boost".as_ptr()), 0);
            assert_eq!(omx_eq_set_gain(h, 6, 3.0), 0);
            let f = [64.0f64, 2000.0];
            let mut out = [0.0f64; 2];
            omx_eq_response_db(h, f.as_ptr(), out.as_mut_ptr(), 2);
            assert!(out[0] > 5.0 && out[1] > 2.5);
            let mut buf = vec![0.25f32; 2 * 1000];
            omx_eq_process(h, buf.as_mut_ptr(), 1000);
            assert!(buf.iter().all(|v| v.is_finite()));
            let j = omx_eq_get_json(h);
            assert_eq!(omx_eq_set_json(h, j), 0);
            omx_string_free(j);
            assert_eq!(omx_eq_set_json(h, c"{oops".as_ptr()), -1);
            omx_eq_free(h);
        }
    }
}
