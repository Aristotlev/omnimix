//! Musical band layout (named by what you hear) and built-in presets.

use serde::{Deserialize, Serialize};

use super::eq::{Band, EqSettings, FilterKind};

/// Ten bands on ISO octave centers, named by the sound they shape.
pub const MUSICAL: [(&str, FilterKind, f32, f32); 10] = [
    ("Sub", FilterKind::LowShelf, 40.0, 0.71),
    ("Bass", FilterKind::Peak, 64.0, 1.1),
    ("Lows", FilterKind::Peak, 125.0, 1.1),
    ("Body", FilterKind::Peak, 250.0, 1.1),
    ("Mud", FilterKind::Peak, 500.0, 1.1),
    ("Mids", FilterKind::Peak, 1000.0, 1.1),
    ("Claps", FilterKind::Peak, 2000.0, 1.1),
    ("Presence", FilterKind::Peak, 4000.0, 1.1),
    ("Hats", FilterKind::Peak, 8000.0, 1.1),
    ("Air", FilterKind::HighShelf, 14000.0, 0.71),
];

pub fn musical_bands() -> Vec<Band> {
    MUSICAL.iter().map(|(l, k, f, q)| Band::new(l, *k, *f, *q)).collect()
}

/// Gains for the ten musical bands, in order.
pub const BUILTIN: [(&str, [f32; 10]); 12] = [
    ("Flat", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Bass Boost", [5.0, 6.0, 3.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Punch", [1.0, 4.5, 1.0, -0.5, -2.0, 0.0, 3.0, 1.0, 3.0, 1.0]),
    ("V-Shape", [4.0, 4.0, 2.0, 0.0, -2.0, -2.5, 0.0, 2.0, 4.0, 3.0]),
    ("Vocal Clarity", [-2.0, -1.0, 0.0, -1.0, -2.0, 1.0, 3.0, 3.0, 1.0, 0.0]),
    ("Podcast", [-6.0, -3.0, 0.0, -1.0, -2.0, 0.5, 2.0, 3.0, 0.0, -1.0]),
    ("Footsteps", [-3.0, -2.0, -1.0, 0.0, 0.0, 1.0, 3.0, 4.0, 2.0, 0.0]),
    ("Cinema", [5.0, 3.0, 0.0, 0.0, -1.0, 0.0, 1.0, 2.0, 1.0, 2.0]),
    ("Warm", [1.0, 1.5, 2.0, 2.0, 0.0, 0.0, -0.5, -1.0, -2.0, -2.0]),
    ("Bright", [0.0, 0.0, 0.0, -1.0, -1.0, 0.0, 1.0, 2.5, 3.5, 4.0]),
    ("Night", [-3.0, -2.0, 0.0, 0.0, 0.0, 0.0, 0.0, -2.0, -3.0, -3.0]),
    ("Treble Tamer", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -2.0, -4.0, -4.0]),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub name: String,
    pub builtin: bool,
    pub eq: EqSettings,
}

pub fn builtin_presets() -> Vec<Preset> {
    BUILTIN
        .iter()
        .map(|(name, gains)| {
            let mut eq = EqSettings { enabled: true, preset: Some((*name).to_string()), ..Default::default() };
            for (b, g) in eq.bands.iter_mut().zip(gains.iter()) {
                b.gain_db = *g;
            }
            Preset { name: (*name).to_string(), builtin: true, eq }
        })
        .collect()
}
