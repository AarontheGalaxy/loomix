//! Limiter (spec 1.2 step 10, spec 1.3/1.4 — both hardware and virtual
//! strips). A sample-accurate brickwall: no lookahead, no attack/release
//! smoothing, because spec 1.3's Limiter row is a single threshold control
//! with no other parameters, and an instantaneous per-sample clamp is both
//! the simplest implementation and the only one that's trivially exact —
//! output never exceeds the threshold, ever, not even on the first sample
//! of a transient, and a signal at or under threshold is untouched sample
//! for sample (spec 4.1's neutral-setting null test needs exactly that).
//! ponytail: no lookahead, so a hard-clamped transient can sound abrupt;
//! upgrade to a short lookahead buffer if that's audible in practice.
//!
//! Linked across every channel of the frame (the loudest channel decides
//! the gain, applied to all), so limiting a multichannel strip never shifts
//! its image.

pub struct Limiter {
    /// spec 1.3/1.4: -40..+12 dB, default +12 (spec's own stated default —
    /// at or below that, nothing in the strip's expected signal range ever
    /// gets touched).
    pub threshold_db: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self { threshold_db: 12.0 }
    }
}

impl Limiter {
    pub fn process(&self, channels: &mut [f32]) {
        let threshold_linear = 10f32.powf(self.threshold_db / 20.0);
        let peak = channels.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));
        if peak <= threshold_linear {
            return;
        }
        let gain = threshold_linear / peak;
        for s in channels.iter_mut() {
            *s *= gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_test_default_threshold_is_bit_exact_passthrough_below_ceiling() {
        let limiter = Limiter::default();
        let orig = [0.9, -0.5, 0.0, 1.0, -1.0, 0.3, -0.3, 0.999];
        let mut chans = orig;
        limiter.process(&mut chans);
        assert_eq!(chans, orig);
    }

    #[test]
    fn known_answer_output_never_exceeds_threshold() {
        let limiter = Limiter { threshold_db: 0.0 }; // unity ceiling
        let mut chans = [2.0, -3.0, 0.5];
        limiter.process(&mut chans);
        for &s in &chans {
            assert!(s.abs() <= 1.0 + 1e-6, "sample {s} exceeds threshold");
        }
        // Linked: the ratio between channels is preserved.
        assert!((chans[0] / 2.0 - chans[1] / -3.0).abs() < 1e-6);
    }

    #[test]
    fn known_answer_exact_gain_at_the_loudest_channel() {
        let limiter = Limiter { threshold_db: 0.0 };
        let mut chans = [2.0, 1.0];
        limiter.process(&mut chans);
        assert!((chans[0] - 1.0).abs() < 1e-6);
        assert!((chans[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn stability_random_automation_never_produces_nan_or_infinity() {
        let mut seed = 5u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..48_000 {
            let limiter = Limiter {
                threshold_db: -40.0 + rand() * 52.0,
            };
            let mut chans = [rand() * 20.0 - 10.0, rand() * 20.0 - 10.0];
            limiter.process(&mut chans);
            assert!(chans.iter().all(|s| s.is_finite()));
        }
    }
}
