//! Minimal Objective-C glue through objc2 `msg_send!` (Foundation, AppKit,
//! CoreAudio's CATapDescription). Every function here must run inside
//! `objc2::rc::autoreleasepool`.

use std::ffi::{c_char, c_void, CStr, CString};

use base64::Engine;
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{msg_send, sel};

pub type Id = *mut AnyObject;

#[link(name = "Foundation", kind = "framework")]
extern "C" {}
#[link(name = "AppKit", kind = "framework")]
extern "C" {}

pub fn class(name: &CStr) -> Option<&'static AnyClass> {
    AnyClass::get(name)
}

pub unsafe fn nsstring(s: &str) -> Id {
    let c = CString::new(s.replace('\0', "")).unwrap_or_default();
    match class(c"NSString") {
        Some(cls) => msg_send![cls, stringWithUTF8String: c.as_ptr()],
        None => std::ptr::null_mut(),
    }
}

pub unsafe fn to_string(obj: Id) -> Option<String> {
    if obj.is_null() {
        return None;
    }
    let p: *const c_char = msg_send![obj, UTF8String];
    (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}

pub unsafe fn nsnumber_u32(v: u32) -> Id {
    msg_send![class(c"NSNumber").unwrap(), numberWithUnsignedInt: v]
}

pub unsafe fn nsnumber_bool(v: bool) -> Id {
    msg_send![class(c"NSNumber").unwrap(), numberWithBool: Bool::new(v)]
}

pub unsafe fn nsarray(items: &[Id]) -> Id {
    msg_send![class(c"NSArray").unwrap(), arrayWithObjects: items.as_ptr(), count: items.len()]
}

pub unsafe fn nsdict(pairs: &[(&str, Id)]) -> Id {
    let keys: Vec<Id> = pairs.iter().map(|(k, _)| nsstring(k)).collect();
    let vals: Vec<Id> = pairs.iter().map(|(_, v)| *v).collect();
    msg_send![class(c"NSDictionary").unwrap(), dictionaryWithObjects: vals.as_ptr(), forKeys: keys.as_ptr(), count: keys.len()]
}

fn responds(obj: Id, sel: objc2::runtime::Sel) -> bool {
    if obj.is_null() {
        return false;
    }
    unsafe { (*obj).class().responds_to(sel) }
}

/// A stereo-mixdown tap of one process object that silences the app while tapped.
/// Returns (+1 retained description, tap UUID string).
pub unsafe fn tap_description(process_object: u32, name: &str) -> Result<(Id, String), String> {
    let cls = class(c"CATapDescription").ok_or("Core Audio taps need macOS 14.4 or newer")?;
    let procs = nsarray(&[nsnumber_u32(process_object)]);
    let alloc: Id = msg_send![cls, alloc];
    let desc: Id = msg_send![alloc, initStereoMixdownOfProcesses: procs];
    if desc.is_null() {
        return Err("Could not describe the tap".into());
    }
    let uuid: Id = msg_send![class(c"NSUUID").unwrap(), UUID];
    if responds(desc, sel!(setUUID:)) {
        let _: () = msg_send![desc, setUUID: uuid];
    }
    if responds(desc, sel!(setName:)) {
        let _: () = msg_send![desc, setName: nsstring(name)];
    }
    if responds(desc, sel!(setPrivateTap:)) {
        let _: () = msg_send![desc, setPrivateTap: Bool::YES];
    } else if responds(desc, sel!(setPrivate:)) {
        let _: () = msg_send![desc, setPrivate: Bool::YES];
    }
    // CATapMutedWhenTapped = 2: the app is silent only while OmniMix reads the tap.
    if responds(desc, sel!(setMuteBehavior:)) {
        let _: () = msg_send![desc, setMuteBehavior: 2isize];
    }
    let u: Id = msg_send![desc, UUID];
    let s: Id = if u.is_null() { std::ptr::null_mut() } else { msg_send![u, UUIDString] };
    let uid = to_string(s).ok_or("Tap has no UUID")?;
    Ok((desc, uid))
}

pub unsafe fn release(obj: Id) {
    if !obj.is_null() {
        let _: () = msg_send![obj, release];
    }
}

/// "macOS 15.1 (24B83)" and (major, minor).
pub fn os_version() -> (String, (u32, u32)) {
    objc2::rc::autoreleasepool(|_| unsafe {
        let Some(cls) = class(c"NSProcessInfo") else { return ("macOS".into(), (0, 0)) };
        let pi: Id = msg_send![cls, processInfo];
        let s = to_string(msg_send![pi, operatingSystemVersionString]).unwrap_or_default();
        let ver = s.trim_start_matches("Version ").split_whitespace().next().unwrap_or("0.0").to_string();
        let mut it = ver.split('.').map(|x| x.parse::<u32>().unwrap_or(0));
        let v = (it.next().unwrap_or(0), it.next().unwrap_or(0));
        let build = s.split("Build ").nth(1).map(|b| b.trim_end_matches(')').to_string()).unwrap_or_default();
        (format!("macOS {ver} ({build})"), v)
    })
}

pub fn accent_hex() -> Option<String> {
    objc2::rc::autoreleasepool(|_| unsafe {
        let ns_color = class(c"NSColor")?;
        let accent: Id = msg_send![ns_color, controlAccentColor];
        let space: Id = msg_send![class(c"NSColorSpace")?, sRGBColorSpace];
        if accent.is_null() || space.is_null() {
            return None;
        }
        let c: Id = msg_send![accent, colorUsingColorSpace: space];
        if c.is_null() {
            return None;
        }
        let r: f64 = msg_send![c, redComponent];
        let g: f64 = msg_send![c, greenComponent];
        let b: f64 = msg_send![c, blueComponent];
        let to = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some(format!("#{:02X}{:02X}{:02X}", to(r), to(g), to(b)))
    })
}

pub fn dark_mode() -> bool {
    objc2::rc::autoreleasepool(|_| unsafe {
        let Some(cls) = class(c"NSUserDefaults") else { return true };
        let d: Id = msg_send![cls, standardUserDefaults];
        let v: Id = msg_send![d, stringForKey: nsstring("AppleInterfaceStyle")];
        to_string(v).map(|s| s.eq_ignore_ascii_case("dark")).unwrap_or(false)
    })
}

/// Outermost `.app` bundle in an executable path (helpers group under their app).
pub fn outer_app(exe: &str) -> Option<String> {
    let idx = exe.find(".app/")?;
    Some(exe[..idx + 4].to_string())
}

pub struct AppInfo {
    pub name: String,
    pub bundle_id: Option<String>,
    pub icon_png: Option<String>,
}

pub fn app_info(bundle_path: &str) -> AppInfo {
    objc2::rc::autoreleasepool(|_| unsafe {
        let fallback = bundle_path.rsplit('/').next().unwrap_or(bundle_path).trim_end_matches(".app").to_string();
        let mut info = AppInfo { name: fallback, bundle_id: None, icon_png: None };
        if let Some(nsbundle) = class(c"NSBundle") {
            let b: Id = msg_send![nsbundle, bundleWithPath: nsstring(bundle_path)];
            if !b.is_null() {
                info.bundle_id = to_string(msg_send![b, bundleIdentifier]);
                for key in ["CFBundleDisplayName", "CFBundleName"] {
                    let v: Id = msg_send![b, objectForInfoDictionaryKey: nsstring(key)];
                    if let Some(n) = to_string(v).filter(|n| !n.trim().is_empty()) {
                        info.name = n;
                        break;
                    }
                }
            }
        }
        info.icon_png = icon_png(bundle_path);
        info
    })
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    w: f64,
    h: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}
unsafe impl Encode for CGPoint {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl Encode for CGRect {
    const ENCODING: Encoding = Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
}
unsafe impl RefEncode for CGRect {
    const ENCODING_REF: Encoding = Encoding::Pointer(&<Self as Encode>::ENCODING);
}

/// Opaque CGImageRef target so debug-mode encoding checks match `^{CGImage=}`.
#[repr(C)]
struct CGImage {
    _private: [u8; 0],
}
unsafe impl RefEncode for CGImage {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGImage", &[]));
}

/// App icon at 64 px as a PNG data URL.
pub unsafe fn icon_png(path: &str) -> Option<String> {
    let ws: Id = msg_send![class(c"NSWorkspace")?, sharedWorkspace];
    let img: Id = msg_send![ws, iconForFile: nsstring(path)];
    if img.is_null() {
        return None;
    }
    let mut rect = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: 64.0, h: 64.0 } };
    let nil: Id = std::ptr::null_mut();
    let cg: *mut CGImage = msg_send![img, CGImageForProposedRect: &mut rect as *mut CGRect, context: nil, hints: nil];
    if cg.is_null() {
        return None;
    }
    let rep_cls = class(c"NSBitmapImageRep")?;
    let alloc: Id = msg_send![rep_cls, alloc];
    let rep: Id = msg_send![alloc, initWithCGImage: cg];
    if rep.is_null() {
        return None;
    }
    // NSBitmapImageFileTypePNG = 4
    let props = nsdict(&[]);
    let data: Id = msg_send![rep, representationUsingType: 4usize, properties: props];
    let out = if data.is_null() {
        None
    } else {
        let len: usize = msg_send![data, length];
        let bytes: *const c_void = msg_send![data, bytes];
        (!bytes.is_null() && len > 0 && len < 2_000_000).then(|| {
            let raw = std::slice::from_raw_parts(bytes as *const u8, len);
            format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(raw))
        })
    };
    release(rep);
    out
}
