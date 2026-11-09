//! # OmniMix core
//!
//! The OS-agnostic heart of OmniMix. Nothing in this crate touches an operating
//! system API. Platform crates (Windows today, macOS / Linux next) implement
//! [`backend::AudioBackend`] and get the whole mixer, DSP engine, routing logic,
//! persistence and UI contract for free.
//!
//! Layering:
//!
//! ```text
//!   UI (Tauri/React, or any host)  <-- serde JSON -->  mixer::Mixer
//!                                                        |   state, routing, reconcile
//!                                                        v
//!                                            backend::AudioBackend (trait)
//!                                     control plane  |  DSP plane (taps + outputs)
//!                                                        |
//!                                engine::{Bus, Strip}  <-+  real-time graph
//!                                dsp::{EqChain, Asrc, Limiter, Meter}
//! ```

pub mod backend;
pub mod dsp;
pub mod engine;
pub mod mixer;
pub mod mock;
pub mod model;
pub mod state;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const APP_NAME: &str = "OmniMix";
