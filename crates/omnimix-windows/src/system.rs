//! OS identity and the native accent color.

use windows::UI::ViewManagement::{UIColorType, UISettings};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};

use crate::util::{from_wide_buf, WStr};

fn reg_sz(root: HKEY, sub: &str, val: &str) -> Option<String> {
    let (s, v) = (WStr::new(sub), WStr::new(val));
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(root, s.pcwstr(), v.pcwstr(), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size)) };
    (r == ERROR_SUCCESS).then(|| from_wide_buf(&buf))
}

fn reg_dword(root: HKEY, sub: &str, val: &str) -> Option<u32> {
    let (s, v) = (WStr::new(sub), WStr::new(val));
    let mut out = 0u32;
    let mut size = 4u32;
    let r = unsafe { RegGetValueW(root, s.pcwstr(), v.pcwstr(), RRF_RT_REG_DWORD, None, Some(&mut out as *mut u32 as *mut _), Some(&mut size)) };
    (r == ERROR_SUCCESS).then_some(out)
}

const NT_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

pub fn build_number() -> u32 {
    reg_sz(HKEY_LOCAL_MACHINE, NT_KEY, "CurrentBuildNumber").and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

pub fn os_string() -> String {
    let build = build_number();
    let ubr = reg_dword(HKEY_LOCAL_MACHINE, NT_KEY, "UBR").unwrap_or(0);
    let mut product = reg_sz(HKEY_LOCAL_MACHINE, NT_KEY, "ProductName").unwrap_or_else(|| "Windows".into());
    if build >= 22000 {
        product = product.replace("Windows 10", "Windows 11");
    }
    let display = reg_sz(HKEY_LOCAL_MACHINE, NT_KEY, "DisplayVersion").unwrap_or_default();
    format!("{product} {display} (build {build}.{ubr})").replace("  ", " ")
}

fn hex(c: windows::UI::Color) -> String {
    format!("#{:02X}{:02X}{:02X}", c.R, c.G, c.B)
}

/// (accent, accent tuned for dark surfaces).
pub fn accent_colors() -> (Option<String>, Option<String>) {
    if let Ok(ui) = UISettings::new() {
        let a = ui.GetColorValue(UIColorType::Accent).ok().map(hex);
        let l = ui.GetColorValue(UIColorType::AccentLight2).ok().map(hex);
        if a.is_some() {
            return (a, l);
        }
    }
    // Fallback: DWM accent (ABGR).
    let v = reg_dword(HKEY_CURRENT_USER, r"Software\Microsoft\Windows\DWM", "AccentColor");
    let a = v.map(|x| format!("#{:02X}{:02X}{:02X}", x & 0xFF, (x >> 8) & 0xFF, (x >> 16) & 0xFF));
    (a, None)
}

pub fn dark_mode() -> bool {
    reg_dword(HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme").map(|v| v == 0).unwrap_or(true)
}
