//! Output safety limiter: instant attack, smooth release, stereo-linked.
//! Guarantees |sample| <= ceiling so boosted EQ never clips the DAC.

pub struct Limiter {
    ceiling: f32,
    gain: f32,
    release: f32,
}

impl Limiter {
    pub fn new(sample_rate: f64, ceiling_db: f64, release_ms: f64) -> Limiter {
        let ceiling = 10f64.powf(ceiling_db / 20.0) as f32;
        let release = (-1.0 / (sample_rate * release_ms / 1000.0)).exp() as f32;
        Limiter { ceiling, gain: 1.0, release }
    }

    /// Current gain reduction as a linear factor (1 = none).
    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn process(&mut self, buf: &mut [f32], frames: usize, ch: usize) {
        for f in 0..frames {
            let row = &mut buf[f * ch..f * ch + ch];
            let mut peak = 0.0f32;
            for v in row.iter() {
                peak = peak.max(v.abs());
            }
            let target = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };
            if target < self.gain {
                self.gain = target;
            } else {
                self.gain = target + (self.gain - target) * self.release;
            }
            if self.gain < 0.999_999 {
                for v in row.iter_mut() {
                    *v *= self.gain;
                }
            }
            for v in row.iter_mut() {
                if !v.is_finite() {
                    *v = 0.0;
                }
                *v = v.clamp(-self.ceiling, self.ceiling);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_exceeds_ceiling() {
        let mut l = Limiter::new(48_000.0, -0.3, 80.0);
        let ceiling = 10f32.powf(-0.3 / 20.0);
        let mut buf: Vec<f32> = (0..9600).map(|i| ((i as f32) * 0.05).sin() * 3.0).collect();
        l.process(&mut buf, 4800, 2);
        assert!(buf.iter().all(|v| v.abs() <= ceiling + 1e-6));
    }

    #[test]
    fn quiet_signal_untouched() {
        let mut l = Limiter::new(48_000.0, -0.3, 80.0);
        let mut buf: Vec<f32> = (0..960).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect();
        let orig = buf.clone();
        l.process(&mut buf, 480, 2);
        assert_eq!(buf, orig);
    }
}
