//! Denoiser (spec 1.2 step 4, spec 1.3, hardware strips only): an adaptive
//! noise-floor remover. Not spectral/multi-band noise reduction — that's a
//! materially bigger DSP undertaking (FFT analysis, per-bin gain, overlap-
//! add) than a single-band "amount" knob calls for. This is a single-band
//! downward expander whose threshold tracks the strip's own ambient noise
//! floor (fast to follow the floor down, slow to follow transients up, so
//! real signal doesn't get mistaken for a rising floor), which is the
//! standard cheap approximation of the same idea and matches spec 1.3's
//! "Denoiser: threshold ... Noise floor remover amount" framing (a single
//! 0..10 amount, not a frequency-domain control).
//! ponytail: single-band expander, upgrade to spectral subtraction if a
//! real noisy-mic test signal shows audible artefacts a single band can't fix.

use crate::fader::gain_db_to_linear;
use crate::knob_curve::fraction;

/// How far above the tracked floor the expander's threshold sits.
const FLOOR_MARGIN_DB: f32 = 6.0;
/// Floor-tracker time constants: fast to follow the floor down when the
/// signal actually goes quiet, slow to rise so a loud transient doesn't get
/// mistaken for a new, louder "floor".
const FLOOR_FALL_MS: f32 = 50.0;
const FLOOR_RISE_MS: f32 = 1000.0;
/// Gain-smoothing time constants, fixed (not spec parameters — spec 1.3
/// gives the denoiser only `threshold` (amount) and `bypass`).
const GAIN_ATTACK_MS: f32 = 5.0;
const GAIN_RELEASE_MS: f32 = 100.0;
/// RMS-estimation time constant ahead of both the floor tracker and the
/// gain target: without it, a fast attack chases the raw waveform's own
/// ripple (a full-scale sine dips to instantaneous zero every half-cycle)
/// instead of its actual level — same failure mode `compressor.rs`'s
/// `DETECTOR_RMS_MS` exists to avoid.
const LEVEL_RMS_MS: f32 = 5.0;
/// Expansion ratio at amount == 10 (maximum aggressiveness).
const MAX_EXPANSION_RATIO: f32 = 6.0;

pub struct Denoiser {
    knob: f32,
    /// spec 1.3: "Denoiser: bypass detail toggle", independent of the knob.
    pub explicit_bypass: bool,
    sample_rate: f32,
    mean_square: f32,
    floor_db: f32,
    gain: f32,
}

impl Denoiser {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            knob: 0.0,
            explicit_bypass: false,
            sample_rate,
            mean_square: 0.0,
            // Starts at a moderately loud assumption, not dead silence: any
            // *real* quiet noise floor sits below this, so it's learned via
            // the fast FLOOR_FALL_MS path immediately; a loud held signal
            // sits above it and only earns FLOOR_RISE_MS's slow path,
            // instead of an initial silence-guess making the very first
            // sound of any kind (quiet or loud) look identical to the
            // tracker.
            floor_db: -20.0,
            gain: 1.0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    /// The macro knob (spec 1.3): `knob <= 0.0` bypasses entirely (spec
    /// 4.1's true-neutral requirement) and, per spec's "non-zero adds
    /// latency" note, is the only state that adds none.
    pub fn set_knob(&mut self, knob: f32) {
        self.knob = knob;
    }

    fn bypassed(&self) -> bool {
        self.knob <= 0.0 || self.explicit_bypass
    }

    fn one_pole_coeff(&self, time_ms: f32) -> f32 {
        if time_ms <= 0.0 {
            return 1.0;
        }
        1.0 - (-1.0 / (self.sample_rate * (time_ms / 1000.0))).exp()
    }

    pub fn process(&mut self, left: &mut f32, right: &mut f32) {
        if self.bypassed() {
            return;
        }

        let inst_power = (*left * *left + *right * *right) * 0.5;
        let rms_coeff = self.one_pole_coeff(LEVEL_RMS_MS);
        self.mean_square += (inst_power - self.mean_square) * rms_coeff;
        let level = self.mean_square.max(0.0).sqrt();
        let level_db = 20.0 * level.max(1e-10).log10();

        let floor_coeff = if level_db < self.floor_db {
            self.one_pole_coeff(FLOOR_FALL_MS)
        } else {
            self.one_pole_coeff(FLOOR_RISE_MS)
        };
        self.floor_db += (level_db - self.floor_db) * floor_coeff;

        let threshold_db = self.floor_db + FLOOR_MARGIN_DB;
        let ratio = 1.0 + fraction(self.knob) * (MAX_EXPANSION_RATIO - 1.0);
        let target_gr_db = if level_db >= threshold_db {
            0.0
        } else {
            (level_db - threshold_db) * (ratio - 1.0)
        };
        let target_gain = gain_db_to_linear(target_gr_db);

        let gain_coeff = if target_gain < self.gain {
            self.one_pole_coeff(GAIN_ATTACK_MS)
        } else {
            self.one_pole_coeff(GAIN_RELEASE_MS)
        };
        self.gain += (target_gain - self.gain) * gain_coeff;

        *left *= self.gain;
        *right *= self.gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn null_test_knob_zero_is_bit_exact_passthrough() {
        let mut d = Denoiser::new(SR);
        d.set_knob(0.0);
        for n in 0..1000 {
            let orig_l = (n as f32 * 0.013).sin();
            let orig_r = (n as f32 * 0.027).cos();
            let mut l = orig_l;
            let mut r = orig_r;
            d.process(&mut l, &mut r);
            assert_eq!(l, orig_l);
            assert_eq!(r, orig_r);
        }
    }

    #[test]
    fn null_test_explicit_bypass_is_bit_exact_passthrough_even_with_a_nonzero_knob() {
        let mut d = Denoiser::new(SR);
        d.set_knob(8.0);
        d.explicit_bypass = true;
        for n in 0..1000 {
            let orig_l = (n as f32 * 0.013).sin();
            let orig_r = (n as f32 * 0.027).cos();
            let mut l = orig_l;
            let mut r = orig_r;
            d.process(&mut l, &mut r);
            assert_eq!(l, orig_l);
            assert_eq!(r, orig_r);
        }
    }

    #[test]
    fn known_answer_a_quiet_floor_gets_attenuated_and_a_loud_signal_does_not() {
        let mut d = Denoiser::new(SR);
        d.set_knob(10.0);

        // Establish a quiet noise floor for long enough for the tracker to
        // settle (well past FLOOR_FALL_MS).
        for n in 0..(SR as usize) {
            let noise = ((n as f32 * 0.061).sin()) * 0.001;
            let mut l = noise;
            let mut r = noise;
            d.process(&mut l, &mut r);
        }
        let quiet_gain_at_settle = d.gain;

        // A loud tone well above the floor should pass through close to
        // unity once the gain follower catches up.
        for n in 0..(SR as usize) {
            let s = (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / SR).sin();
            let mut l = s;
            let mut r = s;
            d.process(&mut l, &mut r);
        }
        assert!(
            d.gain > 0.9,
            "loud signal should pass near-unity, gain={}",
            d.gain
        );
        assert!(
            quiet_gain_at_settle < 0.9,
            "settled quiet floor should be attenuated, gain={quiet_gain_at_settle}"
        );
    }

    #[test]
    fn higher_amount_attenuates_a_quiet_floor_more() {
        let mut low = Denoiser::new(SR);
        low.set_knob(2.0);
        let mut high = Denoiser::new(SR);
        high.set_knob(10.0);

        for n in 0..(SR as usize) {
            let noise = ((n as f32 * 0.061).sin()) * 0.001;
            let mut l1 = noise;
            let mut r1 = noise;
            low.process(&mut l1, &mut r1);
            let mut l2 = noise;
            let mut r2 = noise;
            high.process(&mut l2, &mut r2);
        }

        assert!(
            high.gain < low.gain,
            "amount=10 should attenuate a settled quiet floor more than amount=2: {} vs {}",
            high.gain,
            low.gain
        );
    }

    #[test]
    fn stability_random_automation_never_produces_nan_or_infinity() {
        let mut d = Denoiser::new(SR);
        let mut seed = 99u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..48_000 * 10 / 100 {
            d.set_knob(rand() * 10.0);
            let mut l = rand() * 2.0 - 1.0;
            let mut r = rand() * 2.0 - 1.0;
            d.process(&mut l, &mut r);
            assert!(l.is_finite() && r.is_finite());
        }
    }
}
