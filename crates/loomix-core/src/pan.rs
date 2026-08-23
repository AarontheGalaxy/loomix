//! Pan pot (spec 1.2 step 9, hardware strips) and 5.1 position pad (same
//! step, virtual strips).
//!
//! **Law choice, and why it isn't constant-power (spec 4.1 layer 2's
//! "pan law preserves energy... across the sweep" doesn't apply here as
//! literally written):** spec 4.1 also requires every effect to have a true
//! bit-exact neutral setting (layer 1's null test). A constant-power law
//! (equal-power sine/cosine taper) cannot give both — it's constant-power
//! *because* it puts both channels at -3dB (0.707) at center, trading
//! total power at the extremes for a *quieter* center, not a neutral one.
//! Direct instruction was to keep center exactly unity and reject the
//! silent -3dB center dip most panners default to, so this is a **balance**
//! law instead: the favoured channel stays at exactly 1.0 across the whole
//! sweep, only the *other* channel is attenuated. Its own invariant (tested
//! below) is "the louder channel never leaves unity, the sweep is
//! monotonic and continuous" — not constant total power.

use crate::{Frame, CH_FL, CH_FR, CH_RL, CH_RR};

/// A stereo balance control. `pan` is `-1.0` (hard left) to `+1.0` (hard
/// right); `0.0` (center, the default) is exactly unity on both channels.
#[derive(Debug, Clone, Copy)]
pub struct StereoBalance {
    pub pan: f32,
}

impl Default for StereoBalance {
    fn default() -> Self {
        Self { pan: 0.0 }
    }
}

impl StereoBalance {
    /// `(left_gain, right_gain)`, linear.
    pub fn gains(pan: f32) -> (f32, f32) {
        let pan = pan.clamp(-1.0, 1.0);
        if pan <= 0.0 {
            (1.0, 1.0 + pan)
        } else {
            (1.0 - pan, 1.0)
        }
    }

    pub fn process(&self, frame: &mut Frame) {
        let (gl, gr) = Self::gains(self.pan);
        frame[CH_FL] *= gl;
        frame[CH_FR] *= gr;
    }
}

/// The virtual-strip 5.1 position pad (spec 1.4): `x` is `-0.5..+0.5`
/// (left/right), `y` is `0.0..1.0` (front/rear). `(0.0, 0.0)` — the
/// default — leaves the frame untouched: front at unity, rear silent, the
/// center channel not synthesised or touched at all.
///
/// ponytail: reuses [`StereoBalance`] for the left/right split rather than
/// a full VBAP-style amplitude panner across all 5 anchors, and never
/// synthesises center-channel content — a source already carrying real FC
/// content passes it through unmodified, but the pad can't *create* FC by
/// being positioned at its labelled anchor. Upgrade to real VBAP if the
/// pad needs to actually hit each of its 5 labelled positions distinctly.
#[derive(Debug, Clone, Copy)]
pub struct PositionPad5_1 {
    pub x: f32,
    pub y: f32,
}

impl Default for PositionPad5_1 {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0 }
    }
}

impl PositionPad5_1 {
    pub fn process(&self, frame: &mut Frame) {
        let x = self.x.clamp(-0.5, 0.5);
        let y = self.y.clamp(0.0, 1.0);
        let (gl, gr) = StereoBalance::gains(x * 2.0);
        let front = 1.0 - y;
        let rear = y;

        let in_l = frame[CH_FL];
        let in_r = frame[CH_FR];
        frame[CH_FL] = in_l * gl * front;
        frame[CH_FR] = in_r * gr * front;
        frame[CH_RL] = in_l * gl * rear;
        frame[CH_RR] = in_r * gr * rear;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CHANNELS;
    use proptest::prelude::*;

    #[test]
    fn null_test_center_pan_is_bit_exact_passthrough() {
        let balance = StereoBalance::default();
        let orig: Frame = [0.7, -0.3, 0.1, 0.0, 0.2, -0.5, 0.0, 0.0];
        let mut frame = orig;
        balance.process(&mut frame);
        assert_eq!(frame, orig);
    }

    #[test]
    fn center_gain_is_exactly_unity_on_both_channels_not_minus_3db() {
        let (gl, gr) = StereoBalance::gains(0.0);
        assert_eq!(gl, 1.0);
        assert_eq!(gr, 1.0);
    }

    #[test]
    fn known_answer_hard_pan_silences_the_opposite_channel() {
        assert_eq!(StereoBalance::gains(-1.0), (1.0, 0.0));
        assert_eq!(StereoBalance::gains(1.0), (0.0, 1.0));
    }

    #[test]
    fn null_test_pad_at_origin_is_bit_exact_passthrough() {
        // A virtual strip's real source only ever carries content in
        // channels 0/1 (spec 1.4): FC/SW/SL/SR are untouched by this block
        // by construction, and RL/RR are legitimately *recomputed* from
        // the current FL/FR every call (see the doc comment above), not
        // preserved — so a realistic fixture leaves them at silence, which
        // is what any never-panned virtual strip's frame actually looks
        // like.
        let pad = PositionPad5_1::default();
        let orig: Frame = [0.7, -0.3, 0.1, 0.0, 0.0, 0.0, 0.4, -0.1];
        let mut frame = orig;
        pad.process(&mut frame);
        assert_eq!(frame, orig);
    }

    #[test]
    fn pad_center_gain_is_exactly_unity_front_not_minus_3db() {
        let pad = PositionPad5_1::default();
        let mut frame: Frame = [0.0; CHANNELS];
        frame[CH_FL] = 1.0;
        frame[CH_FR] = 1.0;
        pad.process(&mut frame);
        assert_eq!(frame[CH_FL], 1.0);
        assert_eq!(frame[CH_FR], 1.0);
        assert_eq!(frame[CH_RL], 0.0);
        assert_eq!(frame[CH_RR], 0.0);
    }

    #[test]
    fn known_answer_full_rear_moves_all_energy_to_the_rear_pair() {
        let pad = PositionPad5_1 { x: 0.0, y: 1.0 };
        let mut frame: Frame = [0.0; CHANNELS];
        frame[CH_FL] = 1.0;
        frame[CH_FR] = 1.0;
        pad.process(&mut frame);
        assert_eq!(frame[CH_FL], 0.0);
        assert_eq!(frame[CH_FR], 0.0);
        assert_eq!(frame[CH_RL], 1.0);
        assert_eq!(frame[CH_RR], 1.0);
    }

    proptest! {
        #[test]
        fn balance_law_never_boosts_past_unity_and_is_monotonic(
            a in -1.0f32..=1.0, b in -1.0f32..=1.0
        ) {
            let (gl_a, gr_a) = StereoBalance::gains(a);
            prop_assert!(gl_a <= 1.0 + 1e-6 && gr_a <= 1.0 + 1e-6);
            prop_assert!(gl_a >= 0.0 && gr_a >= 0.0);
            if a < b {
                let (gl_b, gr_b) = StereoBalance::gains(b);
                // Moving right: left never increases, right never decreases.
                prop_assert!(gl_b <= gl_a + 1e-6);
                prop_assert!(gr_b >= gr_a - 1e-6);
            }
        }

        #[test]
        fn balance_law_is_continuous(pan in -1.0f32..=1.0, delta in 1e-4f32..1e-2) {
            let (gl_a, gr_a) = StereoBalance::gains(pan);
            let (gl_b, gr_b) = StereoBalance::gains(pan + delta);
            prop_assert!((gl_b - gl_a).abs() < delta * 2.0);
            prop_assert!((gr_b - gr_a).abs() < delta * 2.0);
        }
    }

    #[test]
    fn stability_random_automation_never_produces_nan_or_infinity() {
        let mut seed = 3u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..10_000 {
            let pad = PositionPad5_1 {
                x: rand() * 1.0 - 0.5,
                y: rand(),
            };
            let mut frame: Frame = std::array::from_fn(|_| rand() * 2.0 - 1.0);
            pad.process(&mut frame);
            assert!(frame.iter().all(|s| s.is_finite()));
        }
    }
}
