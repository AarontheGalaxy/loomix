//! The per-strip M5 processing chain (spec 1.2's ordered steps, less M6's
//! strip EQ / M7's bus modes / M8's FX sends, which aren't in scope yet).
//! One instance per strip, owned by [`crate::strip::Strip`], run exactly
//! once per input frame — every block here holds mutable envelope/filter
//! state, so it must not be invoked more than once for the same frame (see
//! `engine.rs`'s process-strips-before-buses restructuring).

use crate::compressor::Compressor;
use crate::denoiser::Denoiser;
use crate::eq3::ThreeBandEq;
use crate::gate::Gate;
use crate::intellipan::Intellipan;
use crate::karaoke::Karaoke;
use crate::limiter::Limiter;
use crate::pan::{PositionPad5_1, StereoBalance};
use crate::parametric_eq::ParametricEq;
use crate::{Frame, CH_FC};

/// spec 1.2's hardware-strip order: denoiser, gate, compressor, strip
/// parametric EQ (M6, spec 1.7: stereo, 2 channels), Intellipan pad, pan
/// pot, limiter (FX-sends step is M8, not yet here).
pub struct HardwareChain {
    pub denoiser: Denoiser,
    pub gate: Gate,
    pub compressor: Compressor,
    pub eq: ParametricEq<2>,
    pub pad: Intellipan,
    pub pan: StereoBalance,
    pub limiter: Limiter,
}

impl HardwareChain {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            denoiser: Denoiser::new(sample_rate),
            gate: Gate::new(sample_rate),
            compressor: Compressor::new(sample_rate),
            eq: ParametricEq::new(sample_rate),
            pad: Intellipan::color(sample_rate), // spec names no default pad mode; Color is as good as any, and (0,0) is neutral regardless
            pan: StereoBalance::default(),
            limiter: Limiter::default(),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.denoiser.set_sample_rate(sample_rate);
        self.gate.set_sample_rate(sample_rate);
        self.compressor.set_sample_rate(sample_rate);
        self.eq.set_sample_rate(sample_rate);
        match &mut self.pad {
            Intellipan::Color(p) => p.set_sample_rate(sample_rate),
            Intellipan::Position(p) => p.set_sample_rate(sample_rate),
            Intellipan::Modulation(p) => p.set_sample_rate(sample_rate),
        }
    }

    pub fn process(&mut self, frame: &mut Frame) {
        let (mut l, mut r) = (frame[0], frame[1]);
        self.denoiser.process(&mut l, &mut r);
        self.gate.process(&mut l, &mut r);
        self.compressor.process(&mut l, &mut r);
        l = self.eq.process_channel(0, l);
        r = self.eq.process_channel(1, r);
        frame[0] = l;
        frame[1] = r;
        self.pad.process(frame);
        self.pan.process(frame);
        self.limiter.process(frame);
    }
}

/// spec 1.2's virtual-strip order: 3-band EQ, M.C., Karaoke (AUX strip
/// only), 5.1 position pad, limiter. Spec doesn't state where M.C./Karaoke
/// sit relative to the EQ/pad (only spec 1.2's *hardware* chain is fully
/// ordered) — see `strip_dsp::tests` for the empirical, order-proving test
/// this placement is checked against, and `docs/ARCHITECTURE.md` for why.
pub struct VirtualChain {
    pub is_aux: bool,
    pub eq: ThreeBandEq,
    pub mc: bool,
    pub karaoke: Karaoke,
    pub pan_pad: PositionPad5_1,
    pub limiter: Limiter,
}

impl VirtualChain {
    pub fn new(sample_rate: f32, is_aux: bool) -> Self {
        Self {
            is_aux,
            eq: ThreeBandEq::new(sample_rate),
            mc: false,
            karaoke: Karaoke::new(sample_rate),
            pan_pad: PositionPad5_1::default(),
            limiter: Limiter::default(),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.eq.set_sample_rate(sample_rate);
        self.karaoke.set_sample_rate(sample_rate);
    }

    pub fn process(&mut self, frame: &mut Frame) {
        self.eq.process(frame);
        if self.mc {
            frame[CH_FC] = 0.0;
        }
        if self.is_aux {
            let (mut l, mut r) = (frame[0], frame[1]);
            self.karaoke.process(&mut l, &mut r);
            frame[0] = l;
            frame[1] = r;
        }
        self.pan_pad.process(frame);
        self.limiter.process(frame);
    }
}

pub enum StripChain {
    // Boxed for the same reason `Intellipan`'s Position/Modulation
    // variants are (see `docs/ARCHITECTURE.md`'s M5 entry): M6's strip EQ
    // added two `EqChannel`s (each 6 biquads plus a delay line) to
    // `HardwareChain`, and an unboxed enum is sized for its *largest*
    // variant regardless of which is active -- every `Strip`, hardware or
    // virtual, would otherwise pay the bigger side's size. Both variants
    // are boxed, not just `HardwareChain` alone: boxing only one side
    // still leaves clippy's `large_enum_variant` tripped by whichever
    // variant is left unboxed against the other's now-pointer size, and
    // M8's FX sends are going to grow both chains again anyway. Boxing
    // happens only at construction, never inside `process()`.
    Hardware(Box<HardwareChain>),
    Virtual(Box<VirtualChain>),
}

impl StripChain {
    pub fn hardware(sample_rate: f32) -> Self {
        Self::Hardware(Box::new(HardwareChain::new(sample_rate)))
    }

    pub fn virtual_strip(sample_rate: f32, is_aux: bool) -> Self {
        Self::Virtual(Box::new(VirtualChain::new(sample_rate, is_aux)))
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        match self {
            Self::Hardware(c) => c.set_sample_rate(sample_rate),
            Self::Virtual(c) => c.set_sample_rate(sample_rate),
        }
    }

    pub fn process(&mut self, frame: &mut Frame) {
        match self {
            Self::Hardware(c) => c.process(frame),
            Self::Virtual(c) => c.process(frame),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biquad::test_support::{goertzel_magnitude, sine};
    use crate::CHANNELS;

    const SR: f32 = 48_000.0;

    #[test]
    fn hardware_chain_default_is_bit_exact_passthrough() {
        let mut chain = HardwareChain::new(SR);
        for n in 0..1000 {
            let orig: Frame = std::array::from_fn(|c| ((n * (c as i32 + 1)) as f32 * 0.017).sin());
            let mut frame = orig;
            chain.process(&mut frame);
            assert_eq!(frame, orig);
        }
    }

    #[test]
    fn virtual_chain_default_is_bit_exact_passthrough() {
        let mut chain = VirtualChain::new(SR, true);
        for n in 0..1000 {
            let mut frame: Frame = [0.0; CHANNELS];
            frame[0] = (n as f32 * 0.013).sin();
            frame[1] = (n as f32 * 0.027).cos();
            let orig = frame;
            chain.process(&mut frame);
            assert_eq!(frame, orig);
        }
    }

    #[test]
    fn hardware_chain_processes_gate_before_compressor_per_spec_order() {
        // spec 1.2 states this order explicitly (unlike the virtual
        // chain), so this is a direct order assertion, not an inference:
        // a hard-gated silent signal must never reach the compressor as
        // "loud", which it would if the compressor ran first and the gate
        // second (the compressor's own gain reduction would still have
        // seen the original loud level).
        let mut chain = HardwareChain::new(SR);
        chain.gate.set_knob(10.0); // threshold -10dB: this tone won't open it
        chain.compressor.set_knob(10.0); // threshold -40dB, ratio 8: would react hard if it saw this signal
        chain.compressor.params.auto_makeup = false;

        let mut peak_gr_seen = 0.0f32;
        for n in 0..2000 {
            let s = (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / SR).sin() * 0.05; // quiet: stays gated
            let mut frame: Frame = [0.0; CHANNELS];
            frame[0] = s;
            frame[1] = s;
            chain.process(&mut frame);
            peak_gr_seen = peak_gr_seen.min(chain.compressor.gain_reduction_db());
        }
        // The gate silences the signal before the compressor's detector
        // ever sees it, so the compressor should have measured near-silence
        // and applied ~0dB reduction throughout — not the deep reduction
        // it would apply to a real -26dBFS tone.
        assert!(
            peak_gr_seen > -3.0,
            "compressor reacted as if it saw the pre-gate signal: {peak_gr_seen}dB"
        );
    }

    #[test]
    fn hardware_chain_compressor_does_not_see_the_eq_boosted_signal() {
        // spec 1.2 places the strip EQ (step 7) after the compressor (step
        // 6): the compressor's detector must see the *pre-EQ* signal. This
        // proves it, the same technique as the gate-before-compressor test
        // above -- an EQ boost large enough to drive the compressor hard
        // if it ran first, then checking the compressor barely reacted.
        let mut chain = HardwareChain::new(SR);
        chain.compressor.set_knob(10.0); // threshold -40dB, ratio 8:1
        chain.compressor.params.auto_makeup = false;
        chain.eq.on = true;
        chain.eq.set_cell(
            0,
            0,
            crate::parametric_eq::EqCellParams {
                on: true,
                cell_type: crate::parametric_eq::EqCellType::Peak,
                freq_hz: 1000.0,
                gain_db: 18.0,
                q: 1.0,
            },
        );
        chain.eq.set_cell(1, 0, chain.eq.channel_params(0).cells[0]);

        let mut peak_gr_seen = 0.0f32;
        for n in 0..2000 {
            // -46dBFS: below the compressor's -40dB threshold pre-EQ (no
            // reduction expected), but +18dB post-EQ would land at -28dB,
            // well above threshold -- if the compressor saw *that*, it'd
            // react hard.
            let s = (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / SR).sin() * 0.005;
            let mut frame: Frame = [0.0; CHANNELS];
            frame[0] = s;
            frame[1] = s;
            chain.process(&mut frame);
            peak_gr_seen = peak_gr_seen.min(chain.compressor.gain_reduction_db());
        }
        assert!(
            peak_gr_seen > -3.0,
            "compressor reacted as if it saw the post-EQ boosted signal: {peak_gr_seen}dB"
        );
    }

    /// Proves (not just asserts) the virtual chain's EQ-before-pan-pad
    /// placement: EQ and the pan pad are non-commutative when the pad is
    /// set to route content fully to the rear (`y=1.0`), since the pad
    /// zeroes the front channels it reads from. If EQ ran *after* the pad
    /// (the alternative order), it would be boosting silence in the front
    /// channels and the rear channels would carry the *unboosted* tone.
    /// This drives both hypotheses through the real block APIs and checks
    /// the actual `VirtualChain` output against both, matching only the
    /// declared order.
    #[test]
    fn virtual_chain_eq_runs_before_the_pan_pad_provably() {
        let tone = sine(4096, SR, 5000.0); // ThreeBandEq's fixed treble center

        let make_frames = |mono: &[f32]| -> Vec<Frame> {
            mono.iter()
                .map(|&s| {
                    let mut f = [0.0; CHANNELS];
                    f[0] = s;
                    f[1] = s;
                    f
                })
                .collect()
        };

        // Actual: run the real VirtualChain (EQ, then MC/karaoke no-ops,
        // then the pan pad, per this file's declared order).
        let mut actual_chain = VirtualChain::new(SR, false);
        actual_chain.eq.set_gains(0.0, 0.0, 12.0);
        actual_chain.pan_pad = PositionPad5_1 { x: 0.0, y: 1.0 };
        let mut actual = make_frames(&tone);
        for f in actual.iter_mut() {
            actual_chain.process(f);
        }
        let actual_rl: Vec<f32> = actual.iter().map(|f| f[crate::CH_RL]).collect();
        let actual_gain =
            goertzel_magnitude(&actual_rl, 5000.0, SR) / goertzel_magnitude(&tone, 5000.0, SR);

        // Hypothesis A (EQ first, matches the declared order): boost, then
        // route fully to rear. Built independently from the same public
        // block APIs, not by calling the chain under test.
        let mut eq_a = ThreeBandEq::new(SR);
        eq_a.set_gains(0.0, 0.0, 12.0);
        let pad_a = PositionPad5_1 { x: 0.0, y: 1.0 };
        let mut a = make_frames(&tone);
        for f in a.iter_mut() {
            eq_a.process(f);
            pad_a.process(f);
        }
        let a_rl: Vec<f32> = a.iter().map(|f| f[crate::CH_RL]).collect();
        let a_gain = goertzel_magnitude(&a_rl, 5000.0, SR) / goertzel_magnitude(&tone, 5000.0, SR);

        // Hypothesis B (pad first): route to rear, then boost the now-
        // silent front channels — the rear content stays unboosted.
        let mut eq_b = ThreeBandEq::new(SR);
        eq_b.set_gains(0.0, 0.0, 12.0);
        let pad_b = PositionPad5_1 { x: 0.0, y: 1.0 };
        let mut b = make_frames(&tone);
        for f in b.iter_mut() {
            pad_b.process(f);
            eq_b.process(f);
        }
        let b_rl: Vec<f32> = b.iter().map(|f| f[crate::CH_RL]).collect();
        let b_gain = goertzel_magnitude(&b_rl, 5000.0, SR) / goertzel_magnitude(&tone, 5000.0, SR);

        // The two hypotheses must actually disagree, or this test would
        // pass no matter which order the implementation uses.
        assert!(
            (a_gain - b_gain).abs() > 0.5,
            "hypotheses aren't distinguishable: a={a_gain} b={b_gain}"
        );
        assert!(
            (actual_gain - a_gain).abs() < 0.05,
            "actual ({actual_gain}) should match hypothesis A/EQ-first ({a_gain})"
        );
        assert!(
            (actual_gain - b_gain).abs() > 0.4,
            "actual ({actual_gain}) should NOT match hypothesis B/pad-first ({b_gain})"
        );
    }

    #[test]
    fn virtual_chain_mc_only_touches_the_center_channel() {
        let mut chain = VirtualChain::new(SR, false);
        chain.mc = true;
        let mut frame: Frame = [1.0; CHANNELS];
        chain.process(&mut frame);
        assert_eq!(frame[CH_FC], 0.0);
    }

    #[test]
    fn virtual_chain_karaoke_only_applies_to_the_aux_strip() {
        let tone = sine(2000, SR, 894.427); // karaoke's vocal-band center
        let run = |is_aux: bool| -> f32 {
            let mut chain = VirtualChain::new(SR, is_aux);
            chain.karaoke.mode = crate::karaoke::KaraokeMode::KV; // heavy attenuation, ignored unless is_aux
            let mut frames = tone
                .iter()
                .map(|&s| {
                    let mut f = [0.0; CHANNELS];
                    f[0] = s;
                    f[1] = s;
                    f
                })
                .collect::<Vec<_>>();
            for f in frames.iter_mut() {
                chain.process(f);
            }
            let out: Vec<f32> = frames.iter().map(|f| f[0]).collect();
            goertzel_magnitude(&out, 894.427, SR) / goertzel_magnitude(&tone, 894.427, SR)
        };
        assert!(
            run(false) > 0.9,
            "non-aux strip should ignore karaoke mode entirely"
        );
        assert!(
            run(true) < 0.5,
            "aux strip should apply karaoke's KV attenuation"
        );
    }

    #[test]
    fn stability_random_automation_never_produces_nan_or_infinity() {
        let mut hw = HardwareChain::new(SR);
        let mut vs = VirtualChain::new(SR, true);
        let mut seed = 55u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..10_000 {
            hw.denoiser.set_knob(rand() * 10.0);
            hw.gate.set_knob(rand() * 10.0);
            hw.compressor.set_knob(rand() * 10.0);
            hw.pan.pan = rand() * 2.0 - 1.0;
            let mut frame: Frame = std::array::from_fn(|_| rand() * 2.0 - 1.0);
            hw.process(&mut frame);
            assert!(frame.iter().all(|s| s.is_finite()));

            vs.eq.set_gains(
                rand() * 24.0 - 12.0,
                rand() * 24.0 - 12.0,
                rand() * 24.0 - 12.0,
            );
            vs.pan_pad = PositionPad5_1 {
                x: rand() - 0.5,
                y: rand(),
            };
            let mut frame2: Frame = std::array::from_fn(|_| rand() * 2.0 - 1.0);
            vs.process(&mut frame2);
            assert!(frame2.iter().all(|s| s.is_finite()));
        }
    }
}
