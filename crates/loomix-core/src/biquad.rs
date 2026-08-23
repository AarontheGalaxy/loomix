//! One Audio EQ Cookbook (RBJ) biquad, mono, direct form I. Shared by every
//! M5 block that needs a peaking/shelving/band filter — the gate sidechain,
//! the virtual-strip 3-band EQ, the Color pad's tonal shaping, and Karaoke —
//! one implementation, reused, instead of one reimplementation per block
//! (spec 4.1 layer 1's "coefficients derived from the Audio EQ Cookbook
//! formulas" tolerance applies identically everywhere this is used; M6's
//! parametric EQ engine reuses it again, which is what adds
//! `low_pass`/`high_pass`/`low_shelf`/`high_shelf`/`notch` below — spec
//! 1.7's 7 cell types, the two not already covered by an M5 caller).
//!
//! **Coefficient changes are smoothed, not applied instantaneously.** M6's
//! cells are swept live by a user dragging a control (freq/gain/Q every
//! control-rate tick) and switched between types while engaged — applying
//! new coefficients to a filter's existing `(x1,x2,y1,y2)` state with no
//! transition computes one sample as if the *entire* input history had
//! always been running through the new filter, which is a real, audible
//! discontinuity, worst when a cell-type switch changes the whole transfer
//! function at once. [`Biquad::set_coeffs`] ramps linearly from the current
//! coefficient set to the new one over [`COEFF_RAMP_SAMPLES`] samples
//! (applied every `process()` call, not gated on a separate "tick" method,
//! so no caller needs to know a ramp is happening) rather than swapping in
//! one step. See `tests::coefficient_sweeps_do_not_click_...` below for the
//! proof this is both necessary (an instantaneous swap on the same input
//! demonstrably clicks) and sufficient (the smoothed version stays far
//! below that).
//!
//! This only smooths *engaged→engaged* transitions. Entering or leaving
//! bypass ([`Biquad::bypass`], or [`Biquad::set_coeffs`] called on a filter
//! that was bypassed) stays instantaneous, matching every other block's
//! already-established convention that a neutral/bypassed setting is a
//! true bit-exact identity function with no transitional state at all
//! (spec 4.1 layer 1) — and matching the real UI gesture too, since turning
//! a cell fully on/off is a discrete click of a toggle, not something
//! continuously dragged.
//!
//! `ponytail:` the ramp linearly interpolates the raw `b0/b1/b2/a1/a2`
//! coefficients, not a stable filter-design parameter (pole radius/angle,
//! or RBJ's own `S`-domain). For two well-formed, nearby coefficient sets
//! (the realistic case: a knob sweep, or a switch between this cell's own
//! 7 valid types) the intermediate interpolated sets stay well-behaved in
//! practice, but nothing here proves every intermediate step is itself a
//! stable filter for two arbitrarily divergent endpoints. Upgrade path if
//! an extreme parameter jump is ever found to produce an audible artefact
//! or instability during the ramp: interpolate in a parameter domain
//! instead of raw coefficients.

const COEFF_RAMP_SAMPLES: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadCoeffs {
    /// Linearly interpolates every coefficient toward `to` by `t` (`0.0` is
    /// `self`, `1.0` is `to`). Backs [`Biquad`]'s coefficient-change
    /// smoothing; see the module doc comment for why raw-coefficient
    /// interpolation is the deliberately simple choice here.
    fn lerp(self, to: Self, t: f32) -> Self {
        Self {
            b0: self.b0 + (to.b0 - self.b0) * t,
            b1: self.b1 + (to.b1 - self.b1) * t,
            b2: self.b2 + (to.b2 - self.b2) * t,
            a1: self.a1 + (to.a1 - self.a1) * t,
            a2: self.a2 + (to.a2 - self.a2) * t,
        }
    }
}

/// Converts a bandwidth in octaves to the equivalent Q, per the Audio EQ
/// Cookbook's alpha-from-bandwidth relation. Shared by anything that's
/// specified in octaves rather than Q directly (spec 1.3's gate sidechain:
/// "1.5 octave band pass on the detector").
pub fn q_from_bandwidth_octaves(bandwidth_octaves: f32) -> f32 {
    1.0 / (2.0 * ((std::f32::consts::LN_2 / 2.0) * bandwidth_octaves).sinh())
}

impl BiquadCoeffs {
    /// A peaking (bell) filter: boosts or cuts a band around `freq_hz`.
    /// `gain_db == 0.0` is mathematically the identity, but callers that
    /// need a *guaranteed* bit-exact passthrough should bypass the
    /// [`Biquad`] entirely (see its `None` state) rather than rely on that.
    pub fn peaking(sample_rate: f32, freq_hz: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();

        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }

    /// Constant 0dB-peak-gain band pass: passes `freq_hz`, attenuates
    /// everything else. Used as the gate's sidechain detector filter.
    pub fn band_pass(sample_rate: f32, freq_hz: f32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();

        let a0 = 1.0 + alpha;
        Self {
            b0: alpha / a0,
            b1: 0.0,
            b2: -alpha / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// A notch spanning `bandwidth_octaves` around `center_hz`, at full
    /// depth (a true null at the center frequency). Karaoke's vocal-band
    /// modes mix this with the dry signal rather than using it directly, so
    /// they can control how much of the band is actually removed.
    pub fn notch_from_bandwidth(sample_rate: f32, center_hz: f32, bandwidth_octaves: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * center_hz / sample_rate;
        let alpha = w0.sin() * ((std::f32::consts::LN_2 / 2.0) * bandwidth_octaves).sinh();
        let cos_w0 = w0.cos();

        let a0 = 1.0 + alpha;
        Self {
            b0: 1.0 / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: 1.0 / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// A Q-parameterised notch: full null at `freq_hz`, width set by `q`
    /// (spec 1.7's shared 1..100 range across all 7 cell types) rather than
    /// [`notch_from_bandwidth`]'s octave-bandwidth parameterisation —
    /// Karaoke keeps using the bandwidth form it was built for, M6's cells
    /// use this one so every cell type shares one `q` field.
    pub fn notch(sample_rate: f32, freq_hz: f32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();

        let a0 = 1.0 + alpha;
        Self {
            b0: 1.0 / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: 1.0 / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// Low pass: -3dB at `freq_hz` for `q == 0.707`, standard RBJ form.
    pub fn low_pass(sample_rate: f32, freq_hz: f32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();

        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos_w0) / 2.0) / a0,
            b1: (1.0 - cos_w0) / a0,
            b2: ((1.0 - cos_w0) / 2.0) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// High pass: -3dB at `freq_hz` for `q == 0.707`, standard RBJ form.
    pub fn high_pass(sample_rate: f32, freq_hz: f32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();

        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 + cos_w0) / 2.0) / a0,
            b1: (-(1.0 + cos_w0)) / a0,
            b2: ((1.0 + cos_w0) / 2.0) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// Low shelf: `gain_db` applies below `freq_hz`, unity above. RBJ's
    /// canonical shelf formula is parameterised by shelf-slope `S`, not
    /// `Q` — spec 1.7 gives every cell type one shared `q` (1..100) range
    /// instead, so `alpha` here uses the same `sin(w0)/(2*Q)` form every
    /// other cell type in this file uses (Loomix's own choice where the
    /// spec underspecifies, same category as `docs/DSP.md`'s Karaoke/
    /// macro-knob entries), not a `Q`-to-`S` conversion.
    pub fn low_shelf(sample_rate: f32, freq_hz: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let sqrt_a = a.sqrt();

        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
        Self {
            b0: (a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha)) / a0,
            b1: (2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
            b2: (a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha)) / a0,
            a1: (-2.0 * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
            a2: ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha) / a0,
        }
    }

    /// High shelf: `gain_db` applies above `freq_hz`, unity below. Same
    /// `Q`-not-`S` parameterisation as [`low_shelf`](Self::low_shelf).
    pub fn high_shelf(sample_rate: f32, freq_hz: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let sqrt_a = a.sqrt();

        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
        Self {
            b0: (a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha)) / a0,
            b1: (-2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
            b2: (a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha)) / a0,
            a1: (2.0 * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
            a2: ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha) / a0,
        }
    }
}

/// An in-progress linear ramp from one coefficient set to another, over
/// [`COEFF_RAMP_SAMPLES`] calls to `process()`. See the module doc comment.
#[derive(Debug, Clone, Copy)]
struct Ramp {
    from: BiquadCoeffs,
    to: BiquadCoeffs,
    step: u32,
}

/// A single-channel biquad filter. `None` means bypassed: a guaranteed
/// bit-exact passthrough, not "coefficients that happen to converge to
/// identity" (floating point round-trip through a real filter is not
/// guaranteed exact even at nominally-unity settings).
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    coeffs: Option<BiquadCoeffs>,
    ramp: Option<Ramp>,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    pub fn bypassed() -> Self {
        Self::default()
    }

    /// Engaging from bypass (`coeffs` was `None`) is instantaneous — there
    /// is no prior engaged filter to ramp away from. Changing an already-
    /// engaged filter's coefficients starts (or retargets, if a ramp was
    /// already in flight) a [`COEFF_RAMP_SAMPLES`]-sample linear ramp
    /// instead of swapping in one step; see the module doc comment.
    pub fn set_coeffs(&mut self, coeffs: BiquadCoeffs) {
        match self.coeffs {
            Some(current) if current != coeffs => {
                self.ramp = Some(Ramp {
                    from: current,
                    to: coeffs,
                    step: 0,
                });
            }
            Some(_) => {} // identical to what's already (possibly mid-ramp toward) — nothing to do
            None => self.coeffs = Some(coeffs),
        }
    }

    /// Instantaneous, like every other block's bypass/neutral setting
    /// (spec 4.1 layer 1) — not ramped, on purpose; see the module doc
    /// comment.
    pub fn bypass(&mut self) {
        self.coeffs = None;
        self.ramp = None;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        if let Some(ramp) = &mut self.ramp {
            ramp.step += 1;
            self.coeffs = Some(if ramp.step >= COEFF_RAMP_SAMPLES {
                let to = ramp.to;
                self.ramp = None;
                to
            } else {
                ramp.from
                    .lerp(ramp.to, ramp.step as f32 / COEFF_RAMP_SAMPLES as f32)
            });
        }
        let Some(c) = self.coeffs else { return x };
        let y = c.b0 * x + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    /// A full-amplitude sine tone, for single-channel filter tests that
    /// don't need `render.rs`'s 8-channel [`crate::Frame`] machinery.
    pub fn sine(samples: usize, sample_rate: f32, freq_hz: f32) -> Vec<f32> {
        (0..samples)
            .map(|n| (2.0 * std::f32::consts::PI * freq_hz * n as f32 / sample_rate).sin())
            .collect()
    }

    /// Goertzel magnitude of `freq_hz` in `signal`, same algorithm as
    /// `render::goertzel_magnitude`, over a plain `f32` buffer.
    pub fn goertzel_magnitude(signal: &[f32], freq_hz: f32, sample_rate: f32) -> f32 {
        let n = signal.len() as f32;
        let k = (0.5 + (n * freq_hz) / sample_rate).floor();
        let omega = (2.0 * std::f32::consts::PI / n) * k;
        let coeff = 2.0 * omega.cos();
        let (mut q1, mut q2) = (0.0f32, 0.0f32);
        for &x in signal {
            let q0 = coeff * q1 - q2 + x;
            q2 = q1;
            q1 = q0;
        }
        (q1 * q1 + q2 * q2 - q1 * q2 * coeff).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn known_answer_matches_the_cookbook_formula() {
        // fs=48000, f0=1000, Q=1.0, gain=+6dB, independently evaluated
        // (Python, math.sin/cos, not this file's code) straight from the
        // Audio EQ Cookbook peaking-EQ formula (spec 4.1 layer 1).
        let c = BiquadCoeffs::peaking(SR, 1000.0, 1.0, 6.0);
        assert!((c.b0 - 1.043_953_1).abs() < 1e-6, "b0={}", c.b0);
        assert!((c.b1 - -1.895_320_7).abs() < 1e-6, "b1={}", c.b1);
        assert!((c.b2 - 0.867_722_3).abs() < 1e-6, "b2={}", c.b2);
        assert!((c.a1 - -1.895_320_7).abs() < 1e-6, "a1={}", c.a1);
        assert!((c.a2 - 0.911_675_4).abs() < 1e-6, "a2={}", c.a2);
    }

    #[test]
    fn null_test_bypassed_biquad_is_bit_exact_passthrough() {
        let mut bq = Biquad::bypassed();
        let signal = sine(64, SR, 1234.0);
        let out: Vec<f32> = signal.iter().map(|&x| bq.process(x)).collect();
        assert_eq!(out, signal);
    }

    #[test]
    fn frequency_response_peaking_boosts_at_center_and_leaves_far_bins_alone() {
        let mut bq = Biquad::bypassed();
        bq.set_coeffs(BiquadCoeffs::peaking(SR, 1000.0, 2.0, 12.0));

        let at_center = sine(4096, SR, 1000.0);
        let out: Vec<f32> = at_center.iter().map(|&x| bq.process(x)).collect();
        let boosted = goertzel_magnitude(&out, 1000.0, SR);
        let reference = goertzel_magnitude(&at_center, 1000.0, SR);
        let measured_db = 20.0 * (boosted / reference).log10();
        assert!(
            (measured_db - 12.0).abs() < 0.1,
            "expected ~12dB boost at center, measured {measured_db}dB"
        );

        let mut bq_far = Biquad::bypassed();
        bq_far.set_coeffs(BiquadCoeffs::peaking(SR, 1000.0, 2.0, 12.0));
        let far = sine(4096, SR, 8000.0);
        let out_far: Vec<f32> = far.iter().map(|&x| bq_far.process(x)).collect();
        let far_gain =
            goertzel_magnitude(&out_far, 8000.0, SR) / goertzel_magnitude(&far, 8000.0, SR);
        assert!(
            20.0 * far_gain.log10() < 1.0,
            "far bin should be left near-untouched by a Q=2 peak"
        );
    }

    #[test]
    fn frequency_response_band_pass_passes_center_and_attenuates_far() {
        let mut bq = Biquad::bypassed();
        bq.set_coeffs(BiquadCoeffs::band_pass(SR, 1000.0, 2.0));
        let center = sine(4096, SR, 1000.0);
        let out: Vec<f32> = center.iter().map(|&x| bq.process(x)).collect();
        let center_gain =
            goertzel_magnitude(&out, 1000.0, SR) / goertzel_magnitude(&center, 1000.0, SR);
        assert!(
            (center_gain - 1.0).abs() < 0.05,
            "0dB peak gain expected at center"
        );

        let mut bq_far = Biquad::bypassed();
        bq_far.set_coeffs(BiquadCoeffs::band_pass(SR, 1000.0, 2.0));
        let far = sine(4096, SR, 100.0);
        let out_far: Vec<f32> = far.iter().map(|&x| bq_far.process(x)).collect();
        let far_gain =
            goertzel_magnitude(&out_far, 100.0, SR) / goertzel_magnitude(&far, 100.0, SR);
        assert!(far_gain < 0.3, "far bin should be strongly attenuated");
    }

    #[test]
    fn notch_nulls_the_center_and_passes_a_far_tone() {
        let mut bq = Biquad::bypassed();
        bq.set_coeffs(BiquadCoeffs::notch_from_bandwidth(SR, 1000.0, 1.0));
        let center = sine(4096, SR, 1000.0);
        let out: Vec<f32> = center.iter().map(|&x| bq.process(x)).collect();
        let center_gain =
            goertzel_magnitude(&out, 1000.0, SR) / goertzel_magnitude(&center, 1000.0, SR);
        assert!(center_gain < 0.05, "center frequency should be nulled");

        let mut bq_far = Biquad::bypassed();
        bq_far.set_coeffs(BiquadCoeffs::notch_from_bandwidth(SR, 1000.0, 1.0));
        let far = sine(4096, SR, 50.0);
        let out_far: Vec<f32> = far.iter().map(|&x| bq_far.process(x)).collect();
        let far_gain = goertzel_magnitude(&out_far, 50.0, SR) / goertzel_magnitude(&far, 50.0, SR);
        assert!(
            (far_gain - 1.0).abs() < 0.1,
            "far tone should pass close to unity"
        );
    }

    #[test]
    fn stability_random_automation_never_produces_nan_or_infinity() {
        let mut bq = Biquad::bypassed();
        let mut seed = 12345u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..10_000 {
            let freq = 50.0 + rand() * 15_000.0;
            let q = 0.3 + rand() * 10.0;
            let gain = -24.0 + rand() * 48.0;
            bq.set_coeffs(BiquadCoeffs::peaking(SR, freq, q, gain));
            let x = rand() * 2.0 - 1.0;
            let y = bq.process(x);
            assert!(y.is_finite(), "biquad produced a non-finite sample");
        }
    }

    #[test]
    fn stability_random_automation_covers_every_cell_type() {
        // Same shape as the peaking-only stability test above, extended to
        // the 4 new constructors so a random walk through any of spec 1.7's
        // 7 cell types is covered, not just peaking.
        let mut bq = Biquad::bypassed();
        let mut seed = 777u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for i in 0..10_000 {
            let freq = 50.0 + rand() * 15_000.0;
            let q = 0.5 + rand() * 99.5; // spec 1.7's shared cell range: 1..100
            let gain = -36.0 + rand() * 54.0; // spec 1.7's extended UI scale: -36..+18
            let coeffs = match i % 5 {
                0 => BiquadCoeffs::low_pass(SR, freq, q),
                1 => BiquadCoeffs::high_pass(SR, freq, q),
                2 => BiquadCoeffs::low_shelf(SR, freq, q, gain),
                3 => BiquadCoeffs::high_shelf(SR, freq, q, gain),
                _ => BiquadCoeffs::notch(SR, freq, q),
            };
            bq.set_coeffs(coeffs);
            let x = rand() * 2.0 - 1.0;
            let y = bq.process(x);
            assert!(y.is_finite(), "biquad produced a non-finite sample");
        }
    }

    #[test]
    fn known_answer_low_pass_matches_the_cookbook_formula() {
        // fs=48000, f0=1000, Q=1.0, independently evaluated (Python) same
        // as the peaking known-answer test above.
        let c = BiquadCoeffs::low_pass(SR, 1000.0, 1.0);
        assert!((c.b0 - 0.004_015_505).abs() < 1e-6, "b0={}", c.b0);
        assert!((c.b1 - 0.008_031_01).abs() < 1e-6, "b1={}", c.b1);
        assert!((c.b2 - 0.004_015_505).abs() < 1e-6, "b2={}", c.b2);
        assert!((c.a1 - -1.861_408_4).abs() < 1e-6, "a1={}", c.a1);
        assert!((c.a2 - 0.877_470_5).abs() < 1e-6, "a2={}", c.a2);
    }

    #[test]
    fn known_answer_high_pass_matches_the_cookbook_formula() {
        let c = BiquadCoeffs::high_pass(SR, 1000.0, 1.0);
        assert!((c.b0 - 0.934_719_73).abs() < 1e-6, "b0={}", c.b0);
        assert!((c.b1 - -1.869_439_5).abs() < 1e-6, "b1={}", c.b1);
        assert!((c.b2 - 0.934_719_73).abs() < 1e-6, "b2={}", c.b2);
        assert!((c.a1 - -1.861_408_4).abs() < 1e-6, "a1={}", c.a1);
        assert!((c.a2 - 0.877_470_5).abs() < 1e-6, "a2={}", c.a2);
    }

    #[test]
    fn known_answer_notch_matches_the_cookbook_formula() {
        let c = BiquadCoeffs::notch(SR, 1000.0, 1.0);
        assert!((c.b0 - 0.938_735_2).abs() < 1e-6, "b0={}", c.b0);
        assert!((c.b1 - -1.861_408_4).abs() < 1e-6, "b1={}", c.b1);
        assert!((c.b2 - 0.938_735_2).abs() < 1e-6, "b2={}", c.b2);
        assert!((c.a1 - -1.861_408_4).abs() < 1e-6, "a1={}", c.a1);
        assert!((c.a2 - 0.877_470_5).abs() < 1e-6, "a2={}", c.a2);
    }

    #[test]
    fn known_answer_low_shelf_matches_the_cookbook_formula() {
        // fs=48000, f0=1000, Q=1.0, gain=+6dB.
        let c = BiquadCoeffs::low_shelf(SR, 1000.0, 1.0, 6.0);
        assert!((c.b0 - 1.024_36).abs() < 1e-6, "b0={}", c.b0);
        assert!((c.b1 - -1.878_552_1).abs() < 1e-6, "b1={}", c.b1);
        assert!((c.b2 - 0.877_130_1).abs() < 1e-6, "b2={}", c.b2);
        assert!((c.a1 - -1.884_273).abs() < 1e-6, "a1={}", c.a1);
        assert!((c.a2 - 0.895_769_2).abs() < 1e-6, "a2={}", c.a2);
    }

    #[test]
    fn known_answer_high_shelf_matches_the_cookbook_formula() {
        let c = BiquadCoeffs::high_shelf(SR, 1000.0, 1.0, 6.0);
        assert!((c.b0 - 1.947_813_6).abs() < 1e-6, "b0={}", c.b0);
        assert!((c.b1 - -3.670_212_5).abs() < 1e-6, "b1={}", c.b1);
        assert!((c.b2 - 1.744_791_4).abs() < 1e-6, "b2={}", c.b2);
        assert!((c.a1 - -1.833_878_8).abs() < 1e-6, "a1={}", c.a1);
        assert!((c.a2 - 0.856_271_3).abs() < 1e-6, "a2={}", c.a2);
    }

    #[test]
    fn frequency_response_low_pass_passes_low_and_attenuates_high() {
        let mut bq = Biquad::bypassed();
        bq.set_coeffs(BiquadCoeffs::low_pass(SR, 1000.0, 0.707));
        let low = sine(4096, SR, 100.0);
        let out: Vec<f32> = low.iter().map(|&x| bq.process(x)).collect();
        let low_gain = goertzel_magnitude(&out, 100.0, SR) / goertzel_magnitude(&low, 100.0, SR);
        assert!(
            (low_gain - 1.0).abs() < 0.05,
            "passband should be near unity"
        );

        let mut bq_high = Biquad::bypassed();
        bq_high.set_coeffs(BiquadCoeffs::low_pass(SR, 1000.0, 0.707));
        let high = sine(4096, SR, 8000.0);
        let out_high: Vec<f32> = high.iter().map(|&x| bq_high.process(x)).collect();
        let high_gain =
            goertzel_magnitude(&out_high, 8000.0, SR) / goertzel_magnitude(&high, 8000.0, SR);
        assert!(
            high_gain < 0.3,
            "well above cutoff should be strongly attenuated"
        );
    }

    #[test]
    fn frequency_response_high_pass_passes_high_and_attenuates_low() {
        let mut bq = Biquad::bypassed();
        bq.set_coeffs(BiquadCoeffs::high_pass(SR, 1000.0, 0.707));
        let high = sine(4096, SR, 8000.0);
        let out: Vec<f32> = high.iter().map(|&x| bq.process(x)).collect();
        let high_gain =
            goertzel_magnitude(&out, 8000.0, SR) / goertzel_magnitude(&high, 8000.0, SR);
        assert!(
            (high_gain - 1.0).abs() < 0.05,
            "passband should be near unity"
        );

        let mut bq_low = Biquad::bypassed();
        bq_low.set_coeffs(BiquadCoeffs::high_pass(SR, 1000.0, 0.707));
        let low = sine(4096, SR, 60.0);
        let out_low: Vec<f32> = low.iter().map(|&x| bq_low.process(x)).collect();
        let low_gain = goertzel_magnitude(&out_low, 60.0, SR) / goertzel_magnitude(&low, 60.0, SR);
        assert!(
            low_gain < 0.3,
            "well below cutoff should be strongly attenuated"
        );
    }

    #[test]
    fn frequency_response_notch_by_q_nulls_the_center_and_passes_a_far_tone() {
        let mut bq = Biquad::bypassed();
        bq.set_coeffs(BiquadCoeffs::notch(SR, 1000.0, 4.0));
        let center = sine(4096, SR, 1000.0);
        let out: Vec<f32> = center.iter().map(|&x| bq.process(x)).collect();
        let center_gain =
            goertzel_magnitude(&out, 1000.0, SR) / goertzel_magnitude(&center, 1000.0, SR);
        assert!(center_gain < 0.05, "center frequency should be nulled");

        let mut bq_far = Biquad::bypassed();
        bq_far.set_coeffs(BiquadCoeffs::notch(SR, 1000.0, 4.0));
        let far = sine(4096, SR, 200.0);
        let out_far: Vec<f32> = far.iter().map(|&x| bq_far.process(x)).collect();
        let far_gain =
            goertzel_magnitude(&out_far, 200.0, SR) / goertzel_magnitude(&far, 200.0, SR);
        assert!(
            (far_gain - 1.0).abs() < 0.1,
            "far tone should pass close to unity"
        );
    }

    #[test]
    fn frequency_response_low_shelf_boosts_below_and_leaves_far_above_alone() {
        let mut bq = Biquad::bypassed();
        bq.set_coeffs(BiquadCoeffs::low_shelf(SR, 1000.0, 0.707, 12.0));
        let low = sine(4096, SR, 100.0);
        let out: Vec<f32> = low.iter().map(|&x| bq.process(x)).collect();
        let boost_db = 20.0
            * (goertzel_magnitude(&out, 100.0, SR) / goertzel_magnitude(&low, 100.0, SR)).log10();
        assert!(
            (boost_db - 12.0).abs() < 0.5,
            "shelf plateau should read ~12dB: {boost_db}"
        );

        let mut bq_far = Biquad::bypassed();
        bq_far.set_coeffs(BiquadCoeffs::low_shelf(SR, 1000.0, 0.707, 12.0));
        let far = sine(4096, SR, 10_000.0);
        let out_far: Vec<f32> = far.iter().map(|&x| bq_far.process(x)).collect();
        let far_gain =
            goertzel_magnitude(&out_far, 10_000.0, SR) / goertzel_magnitude(&far, 10_000.0, SR);
        assert!(
            (far_gain - 1.0).abs() < 0.1,
            "well above the corner should be near unity"
        );
    }

    #[test]
    fn frequency_response_high_shelf_boosts_above_and_leaves_far_below_alone() {
        let mut bq = Biquad::bypassed();
        bq.set_coeffs(BiquadCoeffs::high_shelf(SR, 1000.0, 0.707, 12.0));
        let high = sine(4096, SR, 10_000.0);
        let out: Vec<f32> = high.iter().map(|&x| bq.process(x)).collect();
        let boost_db = 20.0
            * (goertzel_magnitude(&out, 10_000.0, SR) / goertzel_magnitude(&high, 10_000.0, SR))
                .log10();
        assert!(
            (boost_db - 12.0).abs() < 0.5,
            "shelf plateau should read ~12dB: {boost_db}"
        );

        let mut bq_far = Biquad::bypassed();
        bq_far.set_coeffs(BiquadCoeffs::high_shelf(SR, 1000.0, 0.707, 12.0));
        let far = sine(4096, SR, 80.0);
        let out_far: Vec<f32> = far.iter().map(|&x| bq_far.process(x)).collect();
        let far_gain = goertzel_magnitude(&out_far, 80.0, SR) / goertzel_magnitude(&far, 80.0, SR);
        assert!(
            (far_gain - 1.0).abs() < 0.1,
            "well below the corner should be near unity"
        );
    }

    #[test]
    fn engaging_from_bypass_is_instantaneous_not_ramped() {
        // A fresh, never-engaged Biquad has no prior filter to ramp away
        // from — `set_coeffs` must apply the very first sample at full
        // strength, not 1/64th of the way there. Every existing frequency-
        // response test in this file (and eq3.rs's) already depends on this
        // implicitly by measuring over thousands of samples where a 64-
        // sample ramp-in would be lost in the noise; this test checks it
        // directly, sample by sample.
        let c = BiquadCoeffs::peaking(SR, 1000.0, 1.0, 12.0);
        let mut bq = Biquad::bypassed();
        bq.set_coeffs(c);
        let x = 0.37;
        let y = bq.process(x);
        let expected = c.b0 * x; // b1/b2/a1/a2 terms are all zero on fresh state
        assert!((y - expected).abs() < 1e-6, "y={y} expected={expected}");
    }

    #[test]
    fn coefficient_sweeps_do_not_click_smoothed_output_stays_far_below_the_instantaneous_jump() {
        // Two simulations driven by the identical sequence of coefficient
        // choices (continuous freq/gain/Q sweeps, interleaved every 1000
        // samples with a hard cell-type switch — the worst case, since the
        // whole transfer function changes at once): one through the real,
        // smoothed `Biquad`, one hand-computing what an instantaneous
        // coefficient swap (no ramp at all — the pre-smoothing behaviour)
        // would have produced on the same accumulated filter state. This
        // is the same "wrong implementation actually fails the identical
        // scenario" technique `drift.rs` uses, applied here so the bound
        // below is proven meaningful rather than just a number nothing
        // could ever trip.
        let mut seed = 99u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };

        let start = BiquadCoeffs::peaking(SR, 1000.0, 1.0, 0.0);
        let mut smoothed = Biquad::bypassed();
        smoothed.set_coeffs(start);

        let (mut nx1, mut nx2, mut ny1, mut ny2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut naive_coeffs; // assigned before use on every loop iteration below

        let mut smoothed_max_delta = 0.0f32;
        let mut naive_max_delta = 0.0f32;
        let (mut prev_smoothed, mut prev_naive) = (0.0f32, 0.0f32);

        for n in 0..20_000u32 {
            let freq = 50.0 + rand() * 15_000.0;
            let q = 0.5 + rand() * 99.5; // spec 1.7: 1..100
            let gain = -36.0 + rand() * 54.0; // spec 1.7's extended UI scale
            let new_coeffs = match n % 4000 {
                0..=999 => BiquadCoeffs::peaking(SR, freq, q, gain),
                1000..=1999 => BiquadCoeffs::low_pass(SR, freq, q),
                2000..=2999 => BiquadCoeffs::high_shelf(SR, freq, q, gain),
                _ => BiquadCoeffs::notch(SR, freq, q),
            };

            smoothed.set_coeffs(new_coeffs);
            naive_coeffs = new_coeffs; // no transition at all — applied for this very sample

            let x = (2.0 * std::f32::consts::PI * 300.0 * n as f32 / SR).sin();

            let y_smoothed = smoothed.process(x);
            assert!(y_smoothed.is_finite());
            smoothed_max_delta = smoothed_max_delta.max((y_smoothed - prev_smoothed).abs());
            prev_smoothed = y_smoothed;

            let y_naive = naive_coeffs.b0 * x + naive_coeffs.b1 * nx1 + naive_coeffs.b2 * nx2
                - naive_coeffs.a1 * ny1
                - naive_coeffs.a2 * ny2;
            nx2 = nx1;
            nx1 = x;
            ny2 = ny1;
            ny1 = y_naive;
            naive_max_delta = naive_max_delta.max((y_naive - prev_naive).abs());
            prev_naive = y_naive;
        }

        assert!(
            naive_max_delta > 0.3,
            "test setup doesn't actually produce a naive click to compare against: {naive_max_delta}"
        );
        assert!(
            smoothed_max_delta < naive_max_delta * 0.25,
            "smoothed sweep ({smoothed_max_delta}) isn't meaningfully below the naive \
             instantaneous-swap jump ({naive_max_delta}) it's supposed to fix"
        );
        assert!(
            smoothed_max_delta < 0.1,
            "smoothed sweep still has an audible-scale sample-to-sample jump: {smoothed_max_delta}"
        );
    }
}
