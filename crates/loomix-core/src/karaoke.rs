//! Karaoke (spec 1.4, virtual AUX strip only): `Off`, `K-m`, `K-1`, `K-2`,
//! `K-v`.
//!
//! K-m ("removes the common mono content") is exact phase-cancellation:
//! `(L-R)/2` to both channels, the standard center-channel-cancellation
//! vocal remover — no filter, no interpretation needed, spec's own wording
//! names the algorithm.
//!
//! K-1/K-2/K-v are genuinely ambiguous in spec 1.4 ("K-1 keeps some bass
//! and treble, K-2 keeps more of both... K-v filters the 200 to 4000 Hz
//! vocal band") — no formula, no depth numbers. Read as: all three apply
//! the same wide notch (biquad, shared with `eq3.rs`/`gate.rs`) spanning
//! 200-4000Hz, mixed with the dry signal at increasing depth — K-2 lightest
//! (keeps the most of the original), K-1 deeper, K-v the dedicated
//! full-depth vocal-band filter spec names outright. This is Loomix's own
//! interpretation, not a reproduction of Voicemeeter's, logged in
//! `docs/DSP.md`.

use crate::biquad::{Biquad, BiquadCoeffs};

/// `sqrt(200 * 4000)`: the geometric center of spec 1.4's "200 to 4000 Hz
/// vocal band".
const VOCAL_BAND_CENTER_HZ: f32 = 894.427;
/// `log2(4000 / 200)`: the band's width in octaves.
const VOCAL_BAND_OCTAVES: f32 = 4.321928;

const K1_DEPTH: f32 = 0.85;
const K2_DEPTH: f32 = 0.5;
const KV_DEPTH: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KaraokeMode {
    #[default]
    Off,
    KM,
    K1,
    K2,
    KV,
}

pub struct Karaoke {
    pub mode: KaraokeMode,
    notch_l: Biquad,
    notch_r: Biquad,
}

impl Karaoke {
    pub fn new(sample_rate: f32) -> Self {
        let mut k = Self {
            mode: KaraokeMode::Off,
            notch_l: Biquad::bypassed(),
            notch_r: Biquad::bypassed(),
        };
        k.set_sample_rate(sample_rate);
        k
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let c = BiquadCoeffs::notch_from_bandwidth(
            sample_rate,
            VOCAL_BAND_CENTER_HZ,
            VOCAL_BAND_OCTAVES,
        );
        self.notch_l.set_coeffs(c);
        self.notch_r.set_coeffs(c);
    }

    fn depth(&self) -> Option<f32> {
        match self.mode {
            KaraokeMode::Off | KaraokeMode::KM => None,
            KaraokeMode::K1 => Some(K1_DEPTH),
            KaraokeMode::K2 => Some(K2_DEPTH),
            KaraokeMode::KV => Some(KV_DEPTH),
        }
    }

    pub fn process(&mut self, left: &mut f32, right: &mut f32) {
        match self.mode {
            KaraokeMode::Off => {}
            KaraokeMode::KM => {
                let mid = (*left - *right) * 0.5;
                *left = mid;
                *right = -mid;
            }
            KaraokeMode::K1 | KaraokeMode::K2 | KaraokeMode::KV => {
                let depth = self.depth().unwrap();
                let nl = self.notch_l.process(*left);
                let nr = self.notch_r.process(*right);
                *left = *left * (1.0 - depth) + nl * depth;
                *right = *right * (1.0 - depth) + nr * depth;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biquad::test_support::{goertzel_magnitude, sine};

    const SR: f32 = 48_000.0;

    #[test]
    fn null_test_off_is_bit_exact_passthrough() {
        let mut k = Karaoke::new(SR);
        for n in 0..1000 {
            let orig_l = (n as f32 * 0.013).sin();
            let orig_r = (n as f32 * 0.027).cos();
            let mut l = orig_l;
            let mut r = orig_r;
            k.process(&mut l, &mut r);
            assert_eq!(l, orig_l);
            assert_eq!(r, orig_r);
        }
    }

    #[test]
    fn known_answer_km_silences_identical_left_and_right() {
        let mut k = Karaoke::new(SR);
        k.mode = KaraokeMode::KM;
        for n in 0..100 {
            let s = (n as f32 * 0.1).sin();
            let mut l = s;
            let mut r = s;
            k.process(&mut l, &mut r);
            assert!(l.abs() < 1e-6, "l={l}");
            assert!(r.abs() < 1e-6, "r={r}");
        }
    }

    #[test]
    fn known_answer_km_is_exact_half_difference() {
        let mut k = Karaoke::new(SR);
        k.mode = KaraokeMode::KM;
        let mut l = 0.6f32;
        let mut r = 0.2f32;
        k.process(&mut l, &mut r);
        assert!((l - 0.2).abs() < 1e-6); // (0.6-0.2)/2
        assert!((r - -0.2).abs() < 1e-6);
    }

    #[test]
    fn frequency_response_kv_removes_vocal_band_and_leaves_bass_alone() {
        let mut k = Karaoke::new(SR);
        k.mode = KaraokeMode::KV;
        let center = sine(4096, SR, VOCAL_BAND_CENTER_HZ);
        let mut out_center = Vec::with_capacity(center.len());
        for &s in &center {
            let mut l = s;
            let mut r = s;
            k.process(&mut l, &mut r);
            out_center.push(l);
        }
        let center_gain = goertzel_magnitude(&out_center, VOCAL_BAND_CENTER_HZ, SR)
            / goertzel_magnitude(&center, VOCAL_BAND_CENTER_HZ, SR);
        assert!(
            center_gain < 0.1,
            "vocal band center should be heavily attenuated: {center_gain}"
        );

        let mut k_bass = Karaoke::new(SR);
        k_bass.mode = KaraokeMode::KV;
        let bass = sine(4096, SR, 60.0);
        let mut out_bass = Vec::with_capacity(bass.len());
        for &s in &bass {
            let mut l = s;
            let mut r = s;
            k_bass.process(&mut l, &mut r);
            out_bass.push(l);
        }
        let bass_gain =
            goertzel_magnitude(&out_bass, 60.0, SR) / goertzel_magnitude(&bass, 60.0, SR);
        assert!(
            (bass_gain - 1.0).abs() < 0.1,
            "bass should pass close to unity: {bass_gain}"
        );
    }

    #[test]
    fn k2_removes_less_than_k1_which_removes_less_than_kv() {
        let measure = |mode: KaraokeMode| -> f32 {
            let mut k = Karaoke::new(SR);
            k.mode = mode;
            let tone = sine(4096, SR, VOCAL_BAND_CENTER_HZ);
            let mut out = Vec::with_capacity(tone.len());
            for &s in &tone {
                let mut l = s;
                let mut r = s;
                k.process(&mut l, &mut r);
                out.push(l);
            }
            goertzel_magnitude(&out, VOCAL_BAND_CENTER_HZ, SR)
                / goertzel_magnitude(&tone, VOCAL_BAND_CENTER_HZ, SR)
        };
        let g_k2 = measure(KaraokeMode::K2);
        let g_k1 = measure(KaraokeMode::K1);
        let g_kv = measure(KaraokeMode::KV);
        assert!(
            g_k2 > g_k1,
            "K2 ({g_k2}) should remove less than K1 ({g_k1})"
        );
        assert!(
            g_k1 > g_kv,
            "K1 ({g_k1}) should remove less than KV ({g_kv})"
        );
    }

    #[test]
    fn stability_random_automation_never_produces_nan_or_infinity() {
        let mut k = Karaoke::new(SR);
        let modes = [
            KaraokeMode::Off,
            KaraokeMode::KM,
            KaraokeMode::K1,
            KaraokeMode::K2,
            KaraokeMode::KV,
        ];
        let mut seed = 11u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for i in 0..10_000 {
            k.mode = modes[i % modes.len()];
            let mut l = rand() * 2.0 - 1.0;
            let mut r = rand() * 2.0 - 1.0;
            k.process(&mut l, &mut r);
            assert!(l.is_finite() && r.is_finite());
        }
    }
}
