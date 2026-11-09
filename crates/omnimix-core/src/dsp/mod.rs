//! DSP building blocks. Pure Rust, no allocation on the hot path once warmed up.

pub mod asrc;
pub mod biquad;
pub mod eq;
pub mod limiter;
pub mod presets;

pub use eq::{Band, EqChain, EqParams, EqSettings, FilterKind, MAX_BANDS};
pub use presets::Preset;

#[inline]
pub fn db_to_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

#[inline]
pub fn lin_to_db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// `n` log-spaced frequencies from `lo` to `hi` inclusive.
pub fn log_freqs(n: usize, lo: f64, hi: f64) -> Vec<f64> {
    if n < 2 {
        return vec![lo];
    }
    let (a, b) = (lo.ln(), hi.ln());
    (0..n).map(|i| (a + (b - a) * i as f64 / (n - 1) as f64).exp()).collect()
}
