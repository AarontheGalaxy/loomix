//! Gate (spec 1.2 step 5, spec 1.3, hardware strips only): a stereo
//! downward gate with a band-pass sidechain detector. The macro knob
//! (0..10) is Loomix's own documented curve over the detail parameters
//! (see `docs/DSP.md`), not a reproduction of Voicemeeter's unpublished
//! one; `knob <= 0.0` is a true bypass, not just a very permissive gate.

use crate::biquad::{q_from_bandwidth_octaves, Biquad, BiquadCoeffs};
use crate::fader::{gain_db_to_linear, gain_linear_to_db};
use crate::knob_curve::{fraction, lerp};

/// spec 1.3: "1.5 octave band pass on the detector."
const SIDECHAIN_BANDWIDTH_OCTAVES: f32 = 1.5;
/// Envelope-follower smoothing ahead of the threshold comparison, fixed —
/// not one of spec's listed detail parameters.
const DETECTOR_SMOOTHING_MS: f32 = 5.0;

#[derive(Debug, Clone, Copy)]
pub struct GateParams {
    /// spec 1.3: -60..-10 dB.
    pub threshold_db: f32,
    /// spec 1.3: -60..-10 dB, or `None` for "OFF meaning minus infinity"
    /// (no floor — the gate may reduce all the way to silence).
    pub damping_max_db: Option<f32>,
    /// spec 1.3: 100..4000 Hz. Not driven by the macro knob.
    pub bp_sidechain_hz: f32,
    /// spec 1.3: 0..1000 ms.
    pub attack_ms: f32,
    /// spec 1.3: 0..5000 ms.
    pub hold_ms: f32,
    /// spec 1.3: 0..5000 ms.
    pub release_ms: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        Self {
            threshold_db: -30.0,
            damping_max_db: None,
            bp_sidechain_hz: 1000.0,
            attack_ms: 20.0,
            hold_ms: 200.0,
            release_ms: 150.0,
        }
    }
}

pub struct Gate {
    knob: f32,
    bypass: bool,
    params: GateParams,
    sample_rate: f32,
    sidechain: Biquad,
    envelope: f32,
    gain: f32,
    hold_remaining_samples: u32,
}

impl Gate {
    pub fn new(sample_rate: f32) -> Self {
        let mut gate = Self {
            knob: 0.0,
            bypass: true,
            params: GateParams::default(),
            sample_rate,
            sidechain: Biquad::bypassed(),
            envelope: 0.0,
            gain: 0.0,
            hold_remaining_samples: 0,
        };
        gate.update_sidechain_filter();
        gate
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.update_sidechain_filter();
    }

    /// The macro knob (spec 1.3): `knob <= 0.0` bypasses the gate entirely
    /// (spec 4.1's required true-neutral setting). Loomix's own curve,
    /// documented in `docs/DSP.md`, table "Gate".
    pub fn set_knob(&mut self, knob: f32) {
        self.knob = knob;
        self.bypass = knob <= 0.0;
        if self.bypass {
            return;
        }
        let t = fraction(knob);
        self.params.threshold_db = lerp(-60.0, -10.0, t);
        self.params.damping_max_db = if t >= 0.999 {
            None
        } else {
            Some(lerp(-10.0, -60.0, t))
        };
        self.params.attack_ms = lerp(50.0, 5.0, t);
        self.params.hold_ms = lerp(500.0, 50.0, t);
        self.params.release_ms = lerp(300.0, 80.0, t);
    }

    /// Sets the sidechain band-pass center frequency directly, independent
    /// of the macro knob (spec 1.3 lists it as its own detail control).
    pub fn set_bp_sidechain_hz(&mut self, hz: f32) {
        self.params.bp_sidechain_hz = hz;
        self.update_sidechain_filter();
    }

    fn update_sidechain_filter(&mut self) {
        let q = q_from_bandwidth_octaves(SIDECHAIN_BANDWIDTH_OCTAVES);
        self.sidechain.set_coeffs(BiquadCoeffs::band_pass(
            self.sample_rate,
            self.params.bp_sidechain_hz,
            q,
        ));
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

        let detector_in = (*left + *right) * 0.5;
        let filtered = self.sidechain.process(detector_in);
        let smoothing = self.one_pole_coeff(DETECTOR_SMOOTHING_MS);
        self.envelope += (filtered.abs() - self.envelope) * smoothing;
        let envelope_db = gain_linear_to_db(self.envelope.max(1e-10));

        let attack_coeff = self.one_pole_coeff(self.params.attack_ms);
        let release_coeff = self.one_pole_coeff(self.params.release_ms);
        let hold_samples = ((self.params.hold_ms / 1000.0) * self.sample_rate) as u32;

        if envelope_db > self.params.threshold_db {
            self.hold_remaining_samples = hold_samples;
            self.gain += (1.0 - self.gain) * attack_coeff;
        } else if self.hold_remaining_samples > 0 {
            self.hold_remaining_samples -= 1;
        } else {
            let floor = self
                .params
                .damping_max_db
                .map(gain_db_to_linear)
                .unwrap_or(0.0);
            self.gain += (floor - self.gain) * release_coeff;
        }

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
        let mut gate = Gate::new(SR);
        gate.set_knob(0.0);
        for n in 0..1000 {
            let orig_l = (n as f32 * 0.013).sin();
            let orig_r = (n as f32 * 0.027).cos();
            let mut l = orig_l;
            let mut r = orig_r;
            gate.process(&mut l, &mut r);
            assert_eq!(l, orig_l);
            assert_eq!(r, orig_r);
        }
    }

    #[test]
    fn known_answer_opens_on_loud_signal_and_closes_after_hold_and_release() {
        let mut gate = Gate::new(SR);
        gate.set_knob(10.0); // fastest attack/hold/release, most sensitive
        gate.set_bp_sidechain_hz(1000.0);

        // Silence: gate should stay closed (gain -> 0).
        for _ in 0..2000 {
            let mut l = 0.0;
            let mut r = 0.0;
            gate.process(&mut l, &mut r);
        }
        assert!(
            gate.gain < 0.01,
            "gate should be closed on silence, gain={}",
            gate.gain
        );

        // Loud in-band tone: gate should open within its attack time.
        for n in 0..2000 {
            let s = (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / SR).sin();
            let mut l = s;
            let mut r = s;
            gate.process(&mut l, &mut r);
        }
        assert!(
            gate.gain > 0.9,
            "gate should be open after a loud tone, gain={}",
            gate.gain
        );

        // Back to silence: held open for hold_ms, then releases toward 0.
        let hold_samples = ((gate.params.hold_ms / 1000.0) * SR) as usize;
        for _ in 0..hold_samples / 2 {
            let mut l = 0.0;
            let mut r = 0.0;
            gate.process(&mut l, &mut r);
        }
        assert!(gate.gain > 0.9, "gate should still be held open mid-hold");

        for _ in 0..hold_samples + (SR as usize) {
            let mut l = 0.0;
            let mut r = 0.0;
            gate.process(&mut l, &mut r);
        }
        assert!(
            gate.gain < 0.05,
            "gate should have released to closed, gain={}",
            gate.gain
        );
    }

    #[test]
    fn sidechain_filter_ignores_energy_far_outside_the_band() {
        let mut in_band = Gate::new(SR);
        in_band.set_knob(10.0);
        in_band.set_bp_sidechain_hz(1000.0);
        let mut out_of_band = Gate::new(SR);
        out_of_band.set_knob(10.0);
        out_of_band.set_bp_sidechain_hz(1000.0);

        for n in 0..4000 {
            let in_tone = (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / SR).sin();
            let mut l1 = in_tone;
            let mut r1 = in_tone;
            in_band.process(&mut l1, &mut r1);

            let out_tone = (2.0 * std::f32::consts::PI * 50.0 * n as f32 / SR).sin();
            let mut l2 = out_tone;
            let mut r2 = out_tone;
            out_of_band.process(&mut l2, &mut r2);
        }

        assert!(
            in_band.gain > out_of_band.gain + 0.3,
            "an in-band tone should open the gate more than an equal-amplitude out-of-band tone: {} vs {}",
            in_band.gain,
            out_of_band.gain
        );
    }

    /// Regression guard for a specific failure mode found in the
    /// compressor and denoiser (see `docs/ARCHITECTURE.md`, M5): an
    /// attack/release follower fed a raw, unsmoothed instantaneous
    /// detector value chases the input waveform's own ripple instead of
    /// its level, which for a gate would show up as chatter — rapid
    /// open/close toggling — on sustained material that sits near
    /// threshold, rather than settling to one state. The gate's `envelope`
    /// field already smooths the sidechain-filtered detector (5ms) before
    /// any threshold comparison, unlike the compressor/denoiser's original
    /// bug, so this asserts that protection actually holds under the
    /// harder case (broadband noise, far more envelope variance than a
    /// clean tone, straddling the exact open/close boundary) rather than
    /// just trusting the structural argument.
    #[test]
    fn sustained_near_threshold_noise_does_not_chatter() {
        for offset_db in [
            -6.0, -3.0, -1.0, 0.0, 1.0, 3.0, 6.0, 9.0, 10.0, 10.5, 11.0, 12.0,
        ] {
            let mut gate = Gate::new(SR);
            gate.set_knob(5.0); // threshold -35dB
            gate.set_bp_sidechain_hz(1000.0);

            // uniform noise's rectified average is amplitude/2 (-6dB from peak).
            let target_peak_db = -35.0 + 6.0 + offset_db;
            let amplitude = 10f32.powf(target_peak_db / 20.0);
            let mut seed = 123u32;
            let mut rand = move || {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
            };

            let mut transitions = 0u32;
            let mut was_open = false;
            for n in 0..(SR as usize * 2) {
                let s = rand() * amplitude;
                let mut l = s;
                let mut r = s;
                gate.process(&mut l, &mut r);
                if n > SR as usize {
                    let is_open = gate.gain > 0.5;
                    if is_open != was_open {
                        transitions += 1;
                    }
                    was_open = is_open;
                }
            }
            // At most the single settling transition, at any offset
            // relative to threshold including right at the boundary
            // (found empirically to fall between +10 and +11dB here) —
            // never repeated flapping.
            assert!(
                transitions <= 1,
                "gate chattered at offset {offset_db}dB from threshold: {transitions} transitions in one settled second"
            );
        }
    }

    #[test]
    fn stability_random_automation_never_produces_nan_or_infinity() {
        let mut gate = Gate::new(SR);
        let mut seed = 42u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..48_000 * 10 / 100 {
            gate.set_knob(rand() * 10.0);
            gate.set_bp_sidechain_hz(100.0 + rand() * 3900.0);
            let mut l = rand() * 2.0 - 1.0;
            let mut r = rand() * 2.0 - 1.0;
            gate.process(&mut l, &mut r);
            assert!(l.is_finite() && r.is_finite());
            assert!(gate.gain.is_finite() && gate.gain >= 0.0);
        }
    }
}
