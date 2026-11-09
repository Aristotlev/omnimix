//! Per-app audio sessions: enumeration, process identity, app icons.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine;
use omnimix_core::model::{channel_labels, pretty_exe_name, source_key_from_exe, Channel, Source};
use windows::core::{Interface, PWSTR};
use windows::Win32::Foundation::{CloseHandle, S_OK};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2, IChannelAudioVolume, IMMDevice, ISimpleAudioVolume,
};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Shell::SHDefExtractIconW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use crate::util::{take_pwstr, WStr};

pub struct Session {
    pub source: Source,
    pub simple: ISimpleAudioVolume,
    pub channels: Option<IChannelAudioVolume>,
    pub meter: Option<IAudioMeterInformation>,
}

#[derive(Clone)]
struct ProcInfo {
    exe: Option<String>,
    name: Option<String>,
    seen: Instant,
}

#[derive(Default)]
pub struct ProcCache {
    procs: HashMap<u32, ProcInfo>,
    icons: HashMap<String, Option<String>>,
}

impl ProcCache {
    fn info(&mut self, pid: u32) -> ProcInfo {
        let now = Instant::now();
        self.procs.retain(|_, p| now.duration_since(p.seen) < Duration::from_secs(60));
        if let Some(p) = self.procs.get_mut(&pid) {
            p.seen = now;
            return p.clone();
        }
        let exe = exe_path(pid);
        let name = exe.as_deref().and_then(file_description);
        let p = ProcInfo { exe, name, seen: now };
        self.procs.insert(pid, p.clone());
        p
    }

    fn icon(&mut self, exe: &str) -> Option<String> {
        if let Some(v) = self.icons.get(exe) {
            return v.clone();
        }
        let v = extract_icon_png(exe);
        self.icons.insert(exe.to_string(), v.clone());
        v
    }
}

pub fn exe_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn file_description(exe: &str) -> Option<String> {
    let path = WStr::new(exe);
    unsafe {
        let size = GetFileVersionInfoSizeW(path.pcwstr(), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(path.pcwstr(), None, size, data.as_mut_ptr() as *mut _).ok()?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        let mut langs: Vec<(u16, u16)> = vec![];
        if VerQueryValueW(data.as_ptr() as *const _, WStr::new(r"\VarFileInfo\Translation").pcwstr(), &mut ptr, &mut len).as_bool() && len >= 4 {
            let pairs = std::slice::from_raw_parts(ptr as *const u16, (len / 2) as usize);
            for c in pairs.chunks(2) {
                if c.len() == 2 {
                    langs.push((c[0], c[1]));
                }
            }
        }
        langs.push((0x0409, 0x04B0));
        langs.push((0x0409, 0x04E4));
        for (lang, cp) in langs {
            let q = WStr::new(&format!(r"\StringFileInfo\{lang:04x}{cp:04x}\FileDescription"));
            let mut p = std::ptr::null_mut();
            let mut l = 0u32;
            if VerQueryValueW(data.as_ptr() as *const _, q.pcwstr(), &mut p, &mut l).as_bool() && l > 1 {
                let s = std::slice::from_raw_parts(p as *const u16, l as usize);
                let text = crate::util::from_wide_buf(s).trim().to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }
}

/// Extract the app icon at 48 px and return it as a PNG data URL.
pub fn extract_icon_png(exe: &str) -> Option<String> {
    let path = WStr::new(exe);
    unsafe {
        let mut hicon = HICON::default();
        let hr = SHDefExtractIconW(path.pcwstr(), 0, 0, Some(&mut hicon), None, 48);
        if hr != S_OK || hicon.is_invalid() {
            return None;
        }
        let png = hicon_to_png(hicon);
        let _ = DestroyIcon(hicon);
        png.map(|bytes| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
    }
}

unsafe fn hicon_to_png(hicon: HICON) -> Option<Vec<u8>> {
    let mut ii = ICONINFO::default();
    GetIconInfo(hicon, &mut ii).ok()?;
    let color = ii.hbmColor;
    let mask = ii.hbmMask;
    let cleanup = || {
        if !color.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(color.0));
        }
        if !mask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(mask.0));
        }
    };
    if color.is_invalid() {
        cleanup();
        return None;
    }
    let mut bm = BITMAP::default();
    if GetObjectW(HGDIOBJ(color.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
        cleanup();
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight);
    if w <= 0 || h <= 0 || w > 512 || h > 512 {
        cleanup();
        return None;
    }
    let hdc = CreateCompatibleDC(None);
    let mut bi = BITMAPINFO::default();
    bi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let mut px = vec![0u8; (w * h * 4) as usize];
    let lines = GetDIBits(hdc, color, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
    let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
    if lines > 0 && !has_alpha && !mask.is_invalid() {
        let mut mpx = vec![0u8; (w * h * 4) as usize];
        let mut mbi = bi;
        if GetDIBits(hdc, mask, 0, h as u32, Some(mpx.as_mut_ptr() as *mut _), &mut mbi, DIB_RGB_COLORS) > 0 {
            for (p, m) in px.chunks_exact_mut(4).zip(mpx.chunks_exact(4)) {
                p[3] = if m[0] == 0 { 255 } else { 0 };
            }
        }
    }
    let _ = DeleteDC(hdc);
    cleanup();
    if lines == 0 {
        return None;
    }
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().ok()?;
        wr.write_image_data(&px).ok()?;
    }
    Some(out)
}

/// Enumerate the live sessions on one render endpoint.
pub fn enumerate(dev: &IMMDevice, device_id: &str, cache: &mut ProcCache) -> Vec<Session> {
    let mut out = vec![];
    unsafe {
        let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { return out };
        let Ok(en) = mgr.GetSessionEnumerator() else { return out };
        let n = en.GetCount().unwrap_or(0);
        for i in 0..n {
            let Ok(ctl) = en.GetSession(i) else { continue };
            let state = ctl.GetState().unwrap_or(AudioSessionStateExpired);
            if state == AudioSessionStateExpired {
                continue;
            }
            let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
            let Ok(simple) = ctl.cast::<ISimpleAudioVolume>() else { continue };
            let pid = ctl2.GetProcessId().unwrap_or(0);
            let system = ctl2.IsSystemSoundsSession() == S_OK;
            let instance = ctl2.GetSessionInstanceIdentifier().map(|p| take_pwstr(p)).unwrap_or_default();
            let id = if instance.is_empty() { format!("{device_id}|{pid}|{i}") } else { instance };
            let display = ctl.GetDisplayName().map(|p| take_pwstr(p)).unwrap_or_default();
            let info = if system { ProcInfo { exe: None, name: None, seen: Instant::now() } } else { cache.info(pid) };
            let key = if system {
                "system".to_string()
            } else {
                info.exe.as_deref().map(source_key_from_exe).unwrap_or_else(|| format!("pid-{pid}"))
            };
            let name = if system {
                "System sounds".to_string()
            } else if let Some(n) = info.name.clone().filter(|n| n.len() < 64) {
                n
            } else if !display.is_empty() && !display.starts_with('@') {
                display
            } else if let Some(e) = &info.exe {
                pretty_exe_name(e)
            } else {
                format!("Process {pid}")
            };
            let icon = info.exe.as_deref().and_then(|e| cache.icon(e));
            let volume = simple.GetMasterVolume().unwrap_or(1.0);
            let muted = simple.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            let chan = ctl.cast::<IChannelAudioVolume>().ok();
            let mut channels = vec![];
            if let Some(c) = &chan {
                let count = c.GetChannelCount().unwrap_or(0).min(16) as usize;
                let labels = channel_labels(0, count);
                for (ci, label) in labels.into_iter().enumerate() {
                    let v = c.GetChannelVolume(ci as u32).unwrap_or(1.0);
                    channels.push(Channel { index: ci as u16, label, volume: v, muted: v <= 0.0001 });
                }
            }
            let meter = ctl.cast::<IAudioMeterInformation>().ok();
            out.push(Session {
                source: Source {
                    id,
                    key,
                    pid,
                    name,
                    exe: info.exe.clone(),
                    icon,
                    device_id: device_id.to_string(),
                    active: state == AudioSessionStateActive,
                    system,
                    volume,
                    muted,
                    channels,
                },
                simple,
                channels: chan,
                meter,
            });
        }
    }
    out
}
