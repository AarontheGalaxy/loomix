//! One Audio EQ Cookbook (RBJ) biquad, mono, direct form I. Shared by every
//! M5 block that needs a peaking/shelving/band filter — the gate sidechain,
//! the virtual-strip 3-band EQ, the Color pad's tonal shaping, and Karaoke —
//! one implementation, reused, instead of one reimplementation per block
//! (spec 4.1 layer 1's "coefficients derived from the Audio EQ Cookbook
//! formulas" tolerance applies identically everywhere this is used; M6's
//! parametric EQ engine reuses it again).

#[derive(Debug, Clone, Copy)]
pub struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
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
}

/// A single-channel biquad filter. `None` means bypassed: a guaranteed
/// bit-exact passthrough, not "coefficients that happen to converge to
/// identity" (floating point round-trip through a real filter is not
/// guaranteed exact even at nominally-unity settings).
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    coeffs: Option<BiquadCoeffs>,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    pub fn bypassed() -> Self {
        Self::default()
    }

    pub fn set_coeffs(&mut self, coeffs: BiquadCoeffs) {
        self.coeffs = Some(coeffs);
    }

    pub fn bypass(&mut self) {
        self.coeffs = None;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
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
}
