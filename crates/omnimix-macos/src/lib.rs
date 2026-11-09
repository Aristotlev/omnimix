//! macOS backend for OmniMix (macOS 14.4+).
//!
//! * Control plane: Core Audio HAL objects. Outputs, transport type
//!   (USB, Bluetooth, BLE, HDMI, DisplayPort, Thunderbolt, AirPlay, built-in
//!   speakers vs headphone jack), volume, mute, per-channel volume, default output.
//! * Apps: HAL process objects (pid, bundle, running output, devices used).
//! * DSP plane: one Core Audio process tap per app with muteBehavior =
//!   mutedWhenTapped, read through a private aggregate device clocked by the
//!   target output. macOS has no per-app mixer, so app volume, mute and channel
//!   mute run in the OmniMix engine (`soft_app_mixer`).
//!
//! Taps and private aggregates belong to this process: if it dies, macOS
//! removes them and every app plays normally again. No journal needed.

#[cfg(target_os = "macos")]
mod backend;
#[cfg(target_os = "macos")]
pub mod ffi;
#[cfg(target_os = "macos")]
pub mod objc;
#[cfg(target_os = "macos")]
pub mod procs;
#[cfg(target_os = "macos")]
pub mod streams;

#[cfg(target_os = "macos")]
pub use backend::MacBackend;
