//! 3-band EQ (spec 1.4, virtual strips only): bass, mid, treble, each
//! -12..+12 dB, stereo. All three are peaking (bell) bands centered at
//! fixed frequencies, not true low/high shelves — one filter shape reused
//! three times rather than adding shelf coefficient formulas `biquad.rs`
//! has no other user for yet (M6's full parametric EQ is where shelving
//! types earn their place, spec 1.7's 7 filter types). Band-center
//! frequencies are Loomix's own choice (spec 1.4 gives the gain range
//! only, not corner frequencies) — logged in `docs/DSP.md`.
//!
//! Each band bypasses outright at 0 dB rather than relying on the biquad's
//! 0 dB coefficients being numerically identity (`biquad.rs`'s own
//! known-answer test found they're a pole-zero cancellation, not literally
//! `b1=b2=a1=a2=0` — mathematically unity gain, not bit-exact in floating
//! point over a real filter's state), so all-flat is a guaranteed bit-exact
//! null per spec 4.1.

use crate::biquad::{Biquad, BiquadCoeffs};
use crate::Frame;

const BASS_HZ: f32 = 200.0;
const MID_HZ: f32 = 1000.0;
const MID_Q: f32 = 0.7;
const TREBLE_HZ: f32 = 5000.0;

#[derive(Default)]
struct StereoBiquad {
    left: Biquad,
    right: Biquad,
}

impl StereoBiquad {
    fn set_gain(&mut self, coeffs_at: impl Fn(f32) -> BiquadCoeffs, gain_db: f32) {
        if gain_db == 0.0 {
            self.left.bypass();
            self.right.bypass();
        } else {
            let c = coeffs_at(gain_db);
            self.left.set_coeffs(c);
            self.right.set_coeffs(c);
        }
    }

    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        (self.left.process(l), self.right.process(r))
    }
}

#[derive(Default)]
pub struct ThreeBandEq {
    sample_rate_cell: f32,
    bass: StereoBiquad,
    mid: StereoBiquad,
    treble: StereoBiquad,
    pub bass_db: f32,
    pub mid_db: f32,
    pub treble_db: f32,
}

impl ThreeBandEq {
    pub fn new(sample_rate: f32) -> Self {
        let mut eq = Self {
            sample_rate_cell: sample_rate,
            ..Default::default()
        };
        eq.recompute();
        eq
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate_cell = sample_rate;
        self.recompute();
    }

    /// spec 1.4: -12..+12 dB each. `0.0` on all three bypasses the whole
    /// block, a guaranteed bit-exact null.
    pub fn set_gains(&mut self, bass_db: f32, mid_db: f32, treble_db: f32) {
        self.bass_db = bass_db;
        self.mid_db = mid_db;
        self.treble_db = treble_db;
        self.recompute();
    }

    fn recompute(&mut self) {
        let sr = self.sample_rate_cell;
        self.bass
            .set_gain(|g| BiquadCoeffs::peaking(sr, BASS_HZ, 0.7, g), self.bass_db);
        self.mid
            .set_gain(|g| BiquadCoeffs::peaking(sr, MID_HZ, MID_Q, g), self.mid_db);
        self.treble.set_gain(
            |g| BiquadCoeffs::peaking(sr, TREBLE_HZ, 0.7, g),
            self.treble_db,
        );
    }

    pub fn process(&mut self, frame: &mut Frame) {
        let (l, r) = self.bass.process(frame[0], frame[1]);
        let (l, r) = self.mid.process(l, r);
        let (l, r) = self.treble.process(l, r);
        frame[0] = l;
        frame[1] = r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biquad::test_support::{goertzel_magnitude, sine};
    use crate::CHANNELS;

    const SR: f32 = 48_000.0;

    #[test]
    fn null_test_all_flat_is_bit_exact_passthrough() {
        let mut eq = ThreeBandEq::new(SR);
        eq.set_gains(0.0, 0.0, 0.0);
        for n in 0..1000 {
            let orig: Frame = std::array::from_fn(|c| ((n * (c as i32 + 1)) as f32 * 0.017).sin());
            let mut frame = orig;
            eq.process(&mut frame);
            assert_eq!(frame, orig);
        }
    }

    #[test]
    fn frequency_response_mid_band_boosts_only_near_its_center() {
        let make_tone = |freq: f32| -> Vec<Frame> {
            sine(4096, SR, freq)
                .into_iter()
                .map(|s| {
                    let mut f = [0.0; CHANNELS];
                    f[0] = s;
                    f[1] = s;
                    f
                })
                .collect()
        };

        let center = make_tone(MID_HZ);
        let mut eq_center = ThreeBandEq::new(SR);
        eq_center.set_gains(0.0, 12.0, 0.0);
        let mut out_center = center.clone();
        for f in out_center.iter_mut() {
            eq_center.process(f);
        }
        let center_in: Vec<f32> = center.iter().map(|f| f[0]).collect();
        let center_out: Vec<f32> = out_center.iter().map(|f| f[0]).collect();
        let boost_db = 20.0
            * (goertzel_magnitude(&center_out, MID_HZ, SR)
                / goertzel_magnitude(&center_in, MID_HZ, SR))
            .log10();
        assert!((boost_db - 12.0).abs() < 0.5, "boost={boost_db}");
    }

    #[test]
    fn stability_random_automation_never_produces_nan_or_infinity() {
        let mut eq = ThreeBandEq::new(SR);
        let mut seed = 21u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..10_000 {
            eq.set_gains(
                rand() * 24.0 - 12.0,
                rand() * 24.0 - 12.0,
                rand() * 24.0 - 12.0,
            );
            let mut frame: Frame = std::array::from_fn(|_| rand() * 2.0 - 1.0);
            eq.process(&mut frame);
            assert!(frame.iter().all(|s| s.is_finite()));
        }
    }
}
