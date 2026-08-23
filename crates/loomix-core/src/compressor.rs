//! Compressor (spec 1.2 step 6, spec 1.3, hardware strips only): stereo,
//! link-detected (both channels see the same detector, and the same gain
//! reduction, so a compressed stereo signal doesn't shift image). Soft-knee
//! characteristic curve (DAFX/Reiss-style), macro knob (0..10) mapped by
//! Loomix's own documented curve (`docs/DSP.md`), `knob <= 0.0` a true
//! bypass per spec 4.1's neutral-setting requirement.

use crate::fader::{gain_db_to_linear, gain_linear_to_db};
use crate::knob_curve::{fraction, lerp};

#[derive(Debug, Clone, Copy)]
pub struct CompParams {
    /// spec 1.3: -24..+24 dB. Not driven by the macro knob.
    pub input_gain_db: f32,
    /// spec 1.3: 1..8.
    pub ratio: f32,
    /// spec 1.3: -40..-3 dB.
    pub threshold_db: f32,
    /// spec 1.3: 0..200 ms.
    pub attack_ms: f32,
    /// spec 1.3: 0..5000 ms.
    pub release_ms: f32,
    /// spec 1.3: 0..1 (soft to hard). 0.0 = hard knee.
    pub knee: f32,
    /// spec 1.3: -24..+24 dB. Not driven by the macro knob.
    pub output_gain_db: f32,
    /// spec 1.3: default on.
    pub auto_makeup: bool,
}

impl Default for CompParams {
    fn default() -> Self {
        Self {
            input_gain_db: 0.0,
            ratio: 1.0,
            threshold_db: -3.0,
            attack_ms: 20.0,
            release_ms: 200.0,
            knee: 0.5,
            output_gain_db: 0.0,
            auto_makeup: true,
        }
    }
}

/// Knee width in dB at `knee == 1.0` (fully soft). Our own choice, not a
/// spec value — the spec's `knee` range is unitless 0..1.
const MAX_KNEE_WIDTH_DB: f32 = 24.0;
/// Fixed RMS-estimation time constant ahead of the attack/release follower.
/// Without this, an attack fast enough to chase a signal's actual envelope
/// also chases the ripple of a raw rectified audio-rate waveform, ratcheting
/// the detector toward the peak instead of a stable level estimate — not
/// one of spec's listed detail parameters, so fixed rather than exposed.
const DETECTOR_RMS_MS: f32 = 5.0;

/// The compressor's static (attack/release-free) gain-reduction curve, in
/// dB, negative or zero. Soft-knee formula: <https://www.eecs.qmul.ac.uk/~josh/documents/2012/GiannoulisMassbergReiss-dynamicrangecompression-JAES2012.pdf>.
fn static_gain_reduction_db(level_db: f32, threshold_db: f32, ratio: f32, knee: f32) -> f32 {
    let knee_width = knee * MAX_KNEE_WIDTH_DB;
    let over = level_db - threshold_db;
    let slope = 1.0 / ratio - 1.0;
    if knee_width <= 1e-6 {
        if over > 0.0 {
            over * slope
        } else {
            0.0
        }
    } else if 2.0 * over < -knee_width {
        0.0
    } else if 2.0 * over.abs() <= knee_width {
        slope * (over + knee_width / 2.0).powi(2) / (2.0 * knee_width)
    } else {
        over * slope
    }
}

pub struct Compressor {
    knob: f32,
    bypass: bool,
    pub params: CompParams,
    sample_rate: f32,
    mean_square: f32,
    envelope: f32,
    gain_reduction_db: f32,
}

impl Compressor {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            knob: 0.0,
            bypass: true,
            params: CompParams::default(),
            sample_rate,
            mean_square: 0.0,
            envelope: 0.0,
            gain_reduction_db: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    pub fn gain_reduction_db(&self) -> f32 {
        self.gain_reduction_db
    }

    /// The macro knob (spec 1.3): `knob <= 0.0` bypasses the compressor
    /// entirely. Loomix's own curve, documented in `docs/DSP.md`, table
    /// "Compressor".
    pub fn set_knob(&mut self, knob: f32) {
        self.knob = knob;
        self.bypass = knob <= 0.0;
        if self.bypass {
            return;
        }
        let t = fraction(knob);
        self.params.threshold_db = lerp(-3.0, -40.0, t);
        self.params.ratio = lerp(1.0, 8.0, t);
        self.params.attack_ms = lerp(50.0, 5.0, t);
        self.params.release_ms = lerp(400.0, 80.0, t);
        self.params.knee = lerp(1.0, 0.0, t);
    }

    fn one_pole_coeff(&self, time_ms: f32) -> f32 {
        if time_ms <= 0.0 {
            return 1.0;
        }
        1.0 - (-1.0 / (self.sample_rate * (time_ms / 1000.0))).exp()
    }

    pub fn process(&mut self, left: &mut f32, right: &mut f32) {
        if self.bypass {
            return;
        }

        let in_gain = gain_db_to_linear(self.params.input_gain_db);
        let l = *left * in_gain;
        let r = *right * in_gain;

        let inst_power = (l * l + r * r) * 0.5; // linked, both channels
        let rms_coeff = self.one_pole_coeff(DETECTOR_RMS_MS);
        self.mean_square += (inst_power - self.mean_square) * rms_coeff;
        let detector = self.mean_square.max(0.0).sqrt();
        let coeff = if detector > self.envelope {
            self.one_pole_coeff(self.params.attack_ms)
        } else {
            self.one_pole_coeff(self.params.release_ms)
        };
        self.envelope += (detector - self.envelope) * coeff;
        let level_db = gain_linear_to_db(self.envelope.max(1e-10));

        self.gain_reduction_db = static_gain_reduction_db(
            level_db,
            self.params.threshold_db,
            self.params.ratio,
            self.params.knee,
        );
        let gr_linear = gain_db_to_linear(self.gain_reduction_db);

        let makeup_db = if self.params.auto_makeup {
            -(self.params.threshold_db * (1.0 / self.params.ratio - 1.0)) / 2.0
        } else {
            0.0
        };
        let out_gain = gr_linear * gain_db_to_linear(self.params.output_gain_db + makeup_db);

        *left = l * out_gain;
        *right = r * out_gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn null_test_knob_zero_is_bit_exact_passthrough() {
        let mut comp = Compressor::new(SR);
        comp.set_knob(0.0);
        for n in 0..1000 {
            let orig_l = (n as f32 * 0.013).sin();
            let orig_r = (n as f32 * 0.027).cos();
            let mut l = orig_l;
            let mut r = orig_r;
            comp.process(&mut l, &mut r);
            assert_eq!(l, orig_l);
            assert_eq!(r, orig_r);
        }
    }

    #[test]
    fn known_answer_static_curve_matches_the_formula_above_threshold() {
        // Hard knee, ratio 4, threshold -20dB: 10dB over threshold should
        // reduce to 2.5dB over (spec's own ratio definition).
        let gr = static_gain_reduction_db(-10.0, -20.0, 4.0, 0.0);
        assert!((gr - -7.5).abs() < 1e-4, "gr={gr}"); // 10dB over -> 2.5dB over -> -7.5dB reduction
    }

    #[test]
    fn known_answer_below_threshold_has_no_reduction() {
        assert_eq!(static_gain_reduction_db(-30.0, -20.0, 4.0, 0.0), 0.0);
    }

    #[test]
    fn soft_knee_is_continuous_across_the_knee_boundaries() {
        let threshold = -20.0;
        let ratio = 4.0;
        let knee = 1.0; // 24dB wide
        let mut prev = static_gain_reduction_db(threshold - 20.0, threshold, ratio, knee);
        let mut level = threshold - 20.0 + 0.05;
        while level <= threshold + 20.0 {
            let cur = static_gain_reduction_db(level, threshold, ratio, knee);
            assert!(
                (cur - prev).abs() < 0.5,
                "discontinuity near {level}dB: {prev} -> {cur}"
            );
            prev = cur;
            level += 0.05;
        }
    }

    #[test]
    fn a_steady_tone_above_threshold_settles_to_the_static_curve() {
        let mut comp = Compressor::new(SR);
        comp.set_knob(10.0); // threshold -40dB, ratio 8, fastest times
        comp.params.auto_makeup = false;
        comp.params.knee = 0.0; // hard knee, so the static formula applies exactly

        // A full-scale tone, well settled after 1 second at the fastest
        // release time this milestone's curve allows.
        for n in 0..(SR as usize) {
            let s = (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / SR).sin();
            let mut l = s;
            let mut r = s;
            comp.process(&mut l, &mut r);
        }

        // The detector is RMS-based (see DETECTOR_RMS_MS): a full-scale sine's
        // RMS is amplitude / sqrt(2).
        let expected = static_gain_reduction_db(
            gain_linear_to_db(1.0 / std::f32::consts::SQRT_2),
            comp.params.threshold_db,
            comp.params.ratio,
            comp.params.knee,
        );
        assert!(
            (comp.gain_reduction_db - expected).abs() < 1.0,
            "measured {} expected ~{}",
            comp.gain_reduction_db,
            expected
        );
    }

    #[test]
    fn stability_random_automation_never_produces_nan_or_infinity() {
        let mut comp = Compressor::new(SR);
        let mut seed = 7u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..48_000 * 10 / 100 {
            comp.set_knob(rand() * 10.0);
            let mut l = rand() * 2.0 - 1.0;
            let mut r = rand() * 2.0 - 1.0;
            comp.process(&mut l, &mut r);
            assert!(l.is_finite() && r.is_finite());
        }
    }
}
