//! Windows 10/11 backend for OmniMix.
//!
//! * Control plane: MMDevice API endpoints, IAudioEndpointVolume, session
//!   manager (ISimpleAudioVolume / IChannelAudioVolume), meters.
//! * Connection detection: device topology -> KS filter -> PnP tree ->
//!   USB hub IOCTLs (Type-C flag, speed, VID/PID), KS jack descriptions (AUX),
//!   Bluetooth profile, HDMI/DP jack subtype.
//! * DSP plane (driverless, Windows 10 2004+): process-loopback tap of the app,
//!   app parked on a muted sink endpoint via per-app routing, engine bus
//!   rendered to the real device. Every system change is journaled so a crash
//!   never leaves an app muted or misrouted.

#[cfg(windows)]
pub mod connection;
#[cfg(windows)]
pub mod devices;
#[cfg(windows)]
pub mod journal;
#[cfg(windows)]
pub mod policy;
#[cfg(windows)]
pub mod sessions;
#[cfg(windows)]
pub mod streams;
#[cfg(windows)]
pub mod system;
#[cfg(windows)]
pub mod util;

#[cfg(windows)]
mod backend;
#[cfg(windows)]
pub use backend::WindowsBackend;
