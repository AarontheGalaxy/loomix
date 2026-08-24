//! Spec 1.6's 12 bus modes: a pure transform from the bus's already-summed
//! 8-channel frame (`FL FR FC SW RL RR SL SR`) to its post-mode frame,
//! applied at spec 1.2's per-output-bus step 3 (before the bus EQ, step 4
//! — see `engine.rs`'s `bus_mode_runs_before_eq_provably`).
//!
//! `Composite` is not a formula over the summed frame at all (spec: "the 8
//! channels are filled from the composite patch, taking any strip pre or
//! post fader") — it needs cross-strip access this module doesn't have, so
//! `engine.rs` fills it directly and never calls [`transform`] for a
//! Composite-mode bus. `transform` still defines `Composite` as identity,
//! so calling it directly (as the tests below do) has a well-defined,
//! harmless answer rather than an unreachable arm.

use crate::{Frame, CHANNELS, CH_FC, CH_FL, CH_FR, CH_RL, CH_RR, CH_SL, CH_SR, CH_SW};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BusMode {
    #[default]
    Normal,
    MixDownA,
    MixDownB,
    StereoRepeat,
    Composite,
    UpMixTv,
    UpMix21,
    UpMix41,
    UpMix61,
    CenterOnly,
    LfeOnly,
    RearOnly,
}

/// Spec 1.6's published Mix Down A/B formulas literally put `RL` on the
/// right-hand side of the RIGHT channel (`R = RL + 0.7*FC + SW ...`), which
/// reads as a typo for `FR` — a right-channel formula built only from left-
/// side/center/LFE terms, with no `FR` term at all, isn't a stereo mixdown.
/// Implemented here as the corrected `R = FR + 0.7*FC + SW (...)`, per
/// spec 1.6's own instruction; `mix_down_a_right_channel_uses_fr_not_the_
/// published_rl_typo` below proves the two formulas actually disagree and
/// that this implementation picked the corrected one.
pub fn transform(mode: BusMode, frame: Frame) -> Frame {
    let fl = frame[CH_FL];
    let fr = frame[CH_FR];
    let fc = frame[CH_FC];
    let sw = frame[CH_SW];
    let rl = frame[CH_RL];
    let rr = frame[CH_RR];
    let sl = frame[CH_SL];
    let sr = frame[CH_SR];

    match mode {
        BusMode::Normal | BusMode::Composite => frame,
        BusMode::MixDownA => {
            let mut out = [0.0; CHANNELS];
            out[CH_FL] = fl + 0.7 * fc + sw + rl - sl;
            out[CH_FR] = fr + 0.7 * fc + sw - rr + sr;
            out
        }
        BusMode::MixDownB => {
            let mut out = [0.0; CHANNELS];
            out[CH_FL] = fl + 0.7 * fc + sw + rl + sl;
            out[CH_FR] = fr + 0.7 * fc + sw + rr + sr;
            out
        }
        BusMode::StereoRepeat => {
            let mut out = [0.0; CHANNELS];
            for pair in 0..CHANNELS / 2 {
                out[pair * 2] = fl;
                out[pair * 2 + 1] = fr;
            }
            out
        }
        BusMode::UpMixTv => {
            let mut out = [0.0; CHANNELS];
            out[CH_FL] = fl;
            out[CH_FR] = fr;
            out[CH_FC] = 0.2 * (fl + fr);
            out[CH_SW] = 0.5 * (fl + fr);
            out[CH_RL] = 0.7 * (fl - fr);
            out[CH_SL] = 0.7 * (fl - fr);
            out[CH_RR] = 0.7 * (fr - fl);
            out[CH_SR] = 0.7 * (fr - fl);
            out
        }
        BusMode::UpMix21 => {
            let mut out = [0.0; CHANNELS];
            out[CH_FL] = fl;
            out[CH_FR] = fr;
            out[CH_SW] = 0.5 * (fl + fr);
            out
        }
        BusMode::UpMix41 => {
            let mut out = [0.0; CHANNELS];
            out[CH_FL] = fl;
            out[CH_FR] = fr;
            out[CH_SW] = 0.5 * (fl + fr);
            out[CH_RL] = fl;
            out[CH_RR] = fr;
            out
        }
        BusMode::UpMix61 => {
            let mut out = [0.0; CHANNELS];
            out[CH_FL] = fl;
            out[CH_FR] = fr;
            out[CH_SW] = 0.5 * (fl + fr);
            out[CH_RL] = fl;
            out[CH_RR] = fr;
            out[CH_SL] = fl;
            out[CH_SR] = fr;
            out
        }
        BusMode::CenterOnly => {
            let mut out = [0.0; CHANNELS];
            out[CH_FL] = fc;
            out[CH_FR] = fc;
            out
        }
        BusMode::LfeOnly => {
            let mut out = [0.0; CHANNELS];
            out[CH_FL] = sw;
            out[CH_FR] = sw;
            out
        }
        BusMode::RearOnly => {
            let mut out = [0.0; CHANNELS];
            out[CH_FL] = rl;
            out[CH_FR] = rr;
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-6;

    // Eight distinct, non-cancelling values so a channel that's wired to
    // the wrong source reads as visibly wrong, not coincidentally right.
    fn probe() -> Frame {
        let mut f = [0.0; CHANNELS];
        f[CH_FL] = 1.0;
        f[CH_FR] = 2.0;
        f[CH_FC] = 3.0;
        f[CH_SW] = 4.0;
        f[CH_RL] = 5.0;
        f[CH_RR] = 6.0;
        f[CH_SL] = 7.0;
        f[CH_SR] = 8.0;
        f
    }

    fn assert_close(actual: Frame, expected: Frame) {
        for c in 0..CHANNELS {
            assert!(
                (actual[c] - expected[c]).abs() < EPS,
                "channel {c}: actual {} expected {}",
                actual[c],
                expected[c]
            );
        }
    }

    #[test]
    fn normal_is_a_bit_exact_identity() {
        assert_eq!(transform(BusMode::Normal, probe()), probe());
    }

    #[test]
    fn composite_is_untouched_by_transform_engine_fills_it_directly() {
        assert_eq!(transform(BusMode::Composite, probe()), probe());
    }

    #[test]
    fn mix_down_a_matches_the_corrected_formula() {
        let mut expected = [0.0; CHANNELS];
        expected[CH_FL] = 1.0 + 0.7 * 3.0 + 4.0 + 5.0 - 7.0; // 5.1
        expected[CH_FR] = 2.0 + 0.7 * 3.0 + 4.0 - 6.0 + 8.0; // 10.1
        assert_close(transform(BusMode::MixDownA, probe()), expected);
    }

    #[test]
    fn mix_down_b_matches_the_corrected_formula() {
        let mut expected = [0.0; CHANNELS];
        expected[CH_FL] = 1.0 + 0.7 * 3.0 + 4.0 + 5.0 + 7.0; // 19.1
        expected[CH_FR] = 2.0 + 0.7 * 3.0 + 4.0 + 6.0 + 8.0; // 22.1
        assert_close(transform(BusMode::MixDownB, probe()), expected);
    }

    /// spec 1.6's own instruction: implement the corrected `FR`-based
    /// formula and cover the discrepancy with a test. Feeds `FR` and `RL`
    /// deliberately far apart values so the two candidate formulas produce
    /// very different right-channel answers, then confirms the engine
    /// picked the corrected one and not the published typo.
    #[test]
    fn mix_down_a_right_channel_uses_fr_not_the_published_rl_typo() {
        let mut frame = [0.0; CHANNELS];
        frame[CH_FR] = 100.0;
        frame[CH_RL] = -999.0;

        let actual = transform(BusMode::MixDownA, frame)[CH_FR];
        let corrected_fr_based = 100.0; // FR + 0.7*FC + SW - RR + SR, all others 0
        let published_rl_based = -999.0; // the vendor doc's literal (typo) formula

        assert!((actual - corrected_fr_based).abs() < EPS);
        assert!((actual - published_rl_based).abs() > 100.0);
    }

    #[test]
    fn stereo_repeat_alternates_fl_fr_across_all_four_pairs() {
        let mut expected = [0.0; CHANNELS];
        for pair in 0..4 {
            expected[pair * 2] = 1.0;
            expected[pair * 2 + 1] = 2.0;
        }
        assert_close(transform(BusMode::StereoRepeat, probe()), expected);
    }

    #[test]
    fn up_mix_tv_derives_7_1_from_the_stereo_pair() {
        let mut expected = [0.0; CHANNELS];
        expected[CH_FL] = 1.0;
        expected[CH_FR] = 2.0;
        expected[CH_FC] = 0.2 * 3.0;
        expected[CH_SW] = 0.5 * 3.0;
        expected[CH_RL] = -0.7;
        expected[CH_SL] = -0.7;
        expected[CH_RR] = 0.7;
        expected[CH_SR] = 0.7;
        assert_close(transform(BusMode::UpMixTv, probe()), expected);
    }

    #[test]
    fn up_mix_2_1_only_populates_fl_fr_sw() {
        let mut expected = [0.0; CHANNELS];
        expected[CH_FL] = 1.0;
        expected[CH_FR] = 2.0;
        expected[CH_SW] = 1.5;
        assert_close(transform(BusMode::UpMix21, probe()), expected);
    }

    #[test]
    fn up_mix_4_1_adds_rear_to_2_1() {
        let mut expected = [0.0; CHANNELS];
        expected[CH_FL] = 1.0;
        expected[CH_FR] = 2.0;
        expected[CH_SW] = 1.5;
        expected[CH_RL] = 1.0;
        expected[CH_RR] = 2.0;
        assert_close(transform(BusMode::UpMix41, probe()), expected);
    }

    #[test]
    fn up_mix_6_1_adds_side_to_4_1() {
        let mut expected = [0.0; CHANNELS];
        expected[CH_FL] = 1.0;
        expected[CH_FR] = 2.0;
        expected[CH_SW] = 1.5;
        expected[CH_RL] = 1.0;
        expected[CH_RR] = 2.0;
        expected[CH_SL] = 1.0;
        expected[CH_SR] = 2.0;
        assert_close(transform(BusMode::UpMix61, probe()), expected);
    }

    #[test]
    fn center_only_puts_fc_on_both_channels_and_silences_the_rest() {
        let mut expected = [0.0; CHANNELS];
        expected[CH_FL] = 3.0;
        expected[CH_FR] = 3.0;
        assert_close(transform(BusMode::CenterOnly, probe()), expected);
    }

    #[test]
    fn lfe_only_puts_sw_on_both_channels_and_silences_the_rest() {
        let mut expected = [0.0; CHANNELS];
        expected[CH_FL] = 4.0;
        expected[CH_FR] = 4.0;
        assert_close(transform(BusMode::LfeOnly, probe()), expected);
    }

    #[test]
    fn rear_only_maps_rl_rr_onto_fl_fr_and_silences_the_rest() {
        let mut expected = [0.0; CHANNELS];
        expected[CH_FL] = 5.0;
        expected[CH_FR] = 6.0;
        assert_close(transform(BusMode::RearOnly, probe()), expected);
    }
}
