//! The engine core: 8 strips, 8 buses, the full assignment matrix (spec
//! 3.4 M3), plus (M5 onward) each strip's own effects chain (spec 1.2's
//! denoiser-through-limiter steps, `strip_dsp.rs`), run once per input
//! frame ahead of the per-bus summing below. Per-bus signal flow follows
//! spec 1.2's per-output-bus steps 1 (sum via gain layer), 3 (bus mode,
//! M7), 4 (bus EQ, M6), 5 (mono) and 6-7 (mute, gain); step 2 (FX returns)
//! is M8.

use crate::bus::{Bus, BusMono};
use crate::bus_mode::{self, BusMode};
use crate::fader::gain_db_to_linear;
use crate::meter::Meter;
use crate::patch::{CompositeSource, Patch};
use crate::strip::Strip;
use crate::{Frame, CHANNELS, NUM_BUSES, NUM_STRIPS};

/// spec 1.11 lists 44.1/48/88.2/96/176.4/192kHz as selectable; 48kHz is
/// the engine's starting point until `loomix-hal`/`loomix-app` (M4) sets
/// the real device rate via [`Engine::set_sample_rate`].
const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;

pub struct Engine {
    pub strips: [Strip; NUM_STRIPS],
    pub buses: [Bus; NUM_BUSES],
    /// spec 1.11's composite/insert patch config, shared globally across
    /// any bus using it -- not per-bus, see `patch.rs`.
    pub patch: Patch,
    strip_meters: [Meter; NUM_STRIPS],
    bus_meters: [Meter; NUM_BUSES],
    sample_rate: f32,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            strips: std::array::from_fn(|i| Strip::for_topology_index(i, DEFAULT_SAMPLE_RATE)),
            buses: std::array::from_fn(|_| Bus::new(DEFAULT_SAMPLE_RATE)),
            patch: Patch::default(),
            strip_meters: [Meter::default(); NUM_STRIPS],
            bus_meters: [Meter::default(); NUM_BUSES],
            sample_rate: DEFAULT_SAMPLE_RATE,
        }
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn strip_meter(&self, strip: usize) -> &Meter {
        &self.strip_meters[strip]
    }

    pub fn bus_meter(&self, bus: usize) -> &Meter {
        &self.bus_meters[bus]
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Propagates to every strip's effects chain and every bus's EQ
    /// (filter/delay-line state is sample-rate dependent) — spec 1.11's
    /// device-selection-driven rate change, wired through by `loomix-app`
    /// (M4's clock-master selection).
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        for strip in &mut self.strips {
            strip.chain.set_sample_rate(sample_rate);
        }
        for bus in &mut self.buses {
            bus.eq.set_sample_rate(sample_rate);
        }
    }

    /// Renders one block. `strip_inputs[s]` and `bus_outputs[b]` must each
    /// have the same length (the block's frame count); `strip_inputs` must
    /// have [`NUM_STRIPS`] entries and `bus_outputs` [`NUM_BUSES`].
    ///
    /// Never allocates, locks or performs I/O: every buffer is caller
    /// owned, every per-strip/per-bus state array is fixed size (spec 3.3).
    /// The loop is sample-outer, strip-middle, bus-inner: each strip's
    /// effects chain still runs exactly once per input frame (the M5
    /// invariant -- running it once per bus it feeds would corrupt its
    /// envelope/filter state), but this shape also gives the per-sample
    /// bus loop direct access to every strip's just-computed frame, which
    /// spec 1.6's Composite bus mode needs (it can source any strip's
    /// signal into any bus's channel, not just strips assigned to that
    /// bus) without recomputing -- and so re-running -- a stateful chain a
    /// second time.
    pub fn process_block(&mut self, strip_inputs: &[&[Frame]], bus_outputs: &mut [&mut [Frame]]) {
        debug_assert_eq!(strip_inputs.len(), NUM_STRIPS);
        debug_assert_eq!(bus_outputs.len(), NUM_BUSES);

        for (meter, input) in self.strip_meters.iter_mut().zip(strip_inputs.iter()) {
            meter.observe(input);
        }

        let any_solo = self.strips.iter().any(|s| s.solo);
        let any_composite_bus = self.buses.iter().any(|b| b.mode == BusMode::Composite);
        let composite_needs_processed = any_composite_bus && self.patch.composite_post_fader;

        // spec 1.2 places the fader (step 12) before mute/solo (step 13):
        // a composite POST-fader tap has to see a muted strip's real
        // signal, so that strip's chain has to run even while muted.
        // Every other strip stays gated on mute/solo exactly as before,
        // so a mixer with most strips muted still only pays for the
        // strips actually in use -- see `docs/ARCHITECTURE.md`'s M7 entry
        // for the bench numbers behind this split (running all 8 chains
        // unconditionally regressed the mostly-muted case measurably).
        // The flip side, also logged there: a strip kept warm this way
        // keeps tracking its gate/compressor envelopes while muted, so
        // unmuting it doesn't start from a frozen, stale state the way an
        // always-skipped strip's does.
        let mut strip_active = [false; NUM_STRIPS];
        for (s, strip) in self.strips.iter().enumerate() {
            let ordinarily_unmuted = !(strip.mute || (any_solo && !strip.solo));
            let composite_tapped =
                composite_needs_processed && self.patch.composite_references_strip(s);
            strip_active[s] = ordinarily_unmuted || composite_tapped;
        }

        let block_len = strip_inputs[0].len();

        for n in 0..block_len {
            let mut processed: [Frame; NUM_STRIPS] = [[0.0; CHANNELS]; NUM_STRIPS];
            for (s, strip) in self.strips.iter_mut().enumerate() {
                if !strip_active[s] {
                    continue; // stays silent; chain state stays frozen, same as before this milestone
                }
                let mut p = strip_inputs[s][n];
                strip.chain.process(&mut p);
                if strip.mono {
                    sum_to_mono(&mut p);
                }
                processed[s] = p;
            }

            for (b, out) in bus_outputs.iter_mut().enumerate() {
                let mut frame = [0.0; CHANNELS];
                for (s, strip) in self.strips.iter().enumerate() {
                    if strip.mute || (any_solo && !strip.solo) || !strip.bus_assign[b] {
                        continue;
                    }
                    let gain = gain_db_to_linear(strip.gain_layer_db(b));
                    if gain == 0.0 {
                        continue;
                    }
                    for (o, c) in frame.iter_mut().zip(processed[s].iter()) {
                        *o += c * gain;
                    }
                }

                if self.buses[b].mode == BusMode::Composite {
                    // spec 1.6: "the 8 channels are filled from the
                    // composite patch" -- a per-channel replacement of the
                    // ordinary sum above, not an addition to it. A
                    // `Default` slot leaves `frame[c]` at that ordinary
                    // sum (spec 1.11: "index 0 means the default bus
                    // channel").
                    for (c, slot) in self.patch.composite.iter().enumerate() {
                        if let CompositeSource::Strip {
                            strip: src_strip,
                            channel: src_channel,
                        } = *slot
                        {
                            frame[c] = if self.patch.composite_post_fader {
                                processed[src_strip][src_channel]
                                    * gain_db_to_linear(self.strips[src_strip].gain_layer_db(b))
                            } else {
                                strip_inputs[src_strip][n][src_channel]
                            };
                        }
                    }
                } else {
                    frame = bus_mode::transform(self.buses[b].mode, frame);
                }

                // spec 1.2 step 4, before step 5 (mono) below -- see
                // `tests::bus_eq_runs_before_stereo_reverse_provably` and
                // `tests::bus_mode_runs_before_eq_provably` for the order
                // proofs.
                for (c, sample) in frame.iter_mut().enumerate() {
                    *sample = self.buses[b].eq.process_channel(c, *sample);
                }

                match self.buses[b].mono {
                    BusMono::Off => {}
                    BusMono::Mono => sum_to_mono(&mut frame),
                    BusMono::StereoReverse => frame.swap(0, 1),
                }

                let bus_gain = if self.buses[b].mute {
                    0.0
                } else {
                    gain_db_to_linear(self.buses[b].gain_db())
                };
                for sample in frame.iter_mut() {
                    *sample *= bus_gain;
                }

                out[n] = frame;
            }
        }

        for (b, out) in bus_outputs.iter().enumerate() {
            self.bus_meters[b].observe(out);
        }
    }
}

/// Sums channels 0 and 1 (the stereo pair) to mono in place; channels 2..7
/// are untouched (spec 1.5's mono button swaps/sums "channels 1 and 2").
fn sum_to_mono(frame: &mut Frame) {
    let mid = (frame[0] + frame[1]) * 0.5;
    frame[0] = mid;
    frame[1] = mid;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt_assert::assert_realtime;

    fn silent_inputs(len: usize) -> Vec<Vec<Frame>> {
        vec![vec![[0.0; CHANNELS]; len]; NUM_STRIPS]
    }

    #[test]
    fn default_engine_routes_every_strip_to_bus_zero_at_unity() {
        let mut engine = Engine::new();
        let mut inputs = silent_inputs(4);
        for block in inputs.iter_mut() {
            for frame in block.iter_mut() {
                frame[0] = 1.0;
            }
        }
        let input_refs: Vec<&[Frame]> = inputs.iter().map(|v| v.as_slice()).collect();

        let mut out_bufs: Vec<Vec<Frame>> = vec![vec![[0.0; CHANNELS]; 4]; NUM_BUSES];
        {
            let mut out_refs: Vec<&mut [Frame]> =
                out_bufs.iter_mut().map(|v| v.as_mut_slice()).collect();
            engine.process_block(&input_refs, &mut out_refs);
        }

        // Every strip defaults to A1 (bus 0) only, at 0 dB: bus 0 sums all
        // 8 unity strips, every other bus stays silent.
        for frame in &out_bufs[0] {
            assert!((frame[0] - NUM_STRIPS as f32).abs() < 1e-6);
        }
        for out in &out_bufs[1..] {
            for frame in out {
                assert_eq!(frame[0], 0.0);
            }
        }
    }

    #[test]
    fn neutral_settings_are_a_bit_exact_passthrough() {
        // Null test (spec 4.1 layer 1): one strip, one bus, 0 dB gain, no
        // mute/solo/mono — the bus output must equal the strip input
        // exactly, not just approximately.
        let mut engine = Engine::new();
        for strip in &mut engine.strips {
            strip.bus_assign = [false; NUM_BUSES];
        }
        engine.strips[0].bus_assign[0] = true;

        let probe: Vec<Frame> = (0..16)
            .map(|n| {
                let mut f = [0.0; CHANNELS];
                f[0] = (n as f32 * 0.37).sin();
                f[1] = (n as f32 * 0.61).cos();
                f
            })
            .collect();
        let silence = vec![[0.0; CHANNELS]; probe.len()];
        let input_refs: Vec<&[Frame]> = std::iter::once(probe.as_slice())
            .chain(std::iter::repeat_n(silence.as_slice(), NUM_STRIPS - 1))
            .collect();

        let mut out_bufs: Vec<Vec<Frame>> = vec![vec![[0.0; CHANNELS]; probe.len()]; NUM_BUSES];
        {
            let mut out_refs: Vec<&mut [Frame]> =
                out_bufs.iter_mut().map(|v| v.as_mut_slice()).collect();
            engine.process_block(&input_refs, &mut out_refs);
        }

        assert_eq!(&out_bufs[0], &probe);
    }

    #[test]
    fn solo_globally_silences_non_soloed_strips() {
        let mut engine = Engine::new();
        engine.strips[0].bus_assign = [true; NUM_BUSES];
        engine.strips[1].bus_assign = [true; NUM_BUSES];
        engine.strips[1].solo = true;

        let mut a: Frame = [0.0; CHANNELS];
        a[0] = 1.0;
        let inputs: Vec<Vec<Frame>> = (0..NUM_STRIPS)
            .map(|s| vec![if s == 0 || s == 1 { a } else { [0.0; CHANNELS] }])
            .collect();
        let input_refs: Vec<&[Frame]> = inputs.iter().map(|v| v.as_slice()).collect();
        let mut out_bufs: Vec<Vec<Frame>> = vec![vec![[0.0; CHANNELS]; 1]; NUM_BUSES];
        {
            let mut out_refs: Vec<&mut [Frame]> =
                out_bufs.iter_mut().map(|v| v.as_mut_slice()).collect();
            engine.process_block(&input_refs, &mut out_refs);
        }

        for out in &out_bufs {
            // Only strip 1 (soloed) contributes; strip 0 is silenced by solo.
            assert!((out[0][0] - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn bus_eq_runs_before_stereo_reverse_provably() {
        // spec 1.2 places bus EQ (step 4) before the mono/stereo-reverse
        // transform (step 5). Proven, not asserted: a per-channel EQ
        // setting and a channel *swap* only disagree about which channel
        // ends up boosted if the two operations are genuinely non-
        // commutative, which is why this uses StereoReverse plus a
        // channel-specific EQ setting rather than checking "EQ changed
        // the output" -- that alone would pass regardless of which order
        // actually runs, and would prove nothing about wiring order.
        use crate::biquad::test_support::{goertzel_magnitude, sine};
        use crate::biquad::{Biquad, BiquadCoeffs};
        use crate::parametric_eq::{EqCellParams, EqCellType};

        let mut engine = Engine::new();
        for strip in &mut engine.strips {
            strip.bus_assign = [false; NUM_BUSES];
        }
        engine.strips[0].bus_assign[0] = true;
        engine.buses[0].mono = BusMono::StereoReverse;
        engine.buses[0].eq.on = true;
        engine.buses[0].eq.set_cell(
            0,
            0,
            EqCellParams {
                on: true,
                cell_type: EqCellType::Peak,
                freq_hz: 1000.0,
                gain_db: 18.0,
                q: 1.0,
            },
        );
        // Channel 1's EQ is left at its neutral default -- unboosted.

        let tone = sine(4096, 48_000.0, 1000.0);
        let probe: Vec<Frame> = tone
            .iter()
            .map(|&s| {
                let mut f = [0.0; CHANNELS];
                f[0] = s; // channel 1 starts silent
                f
            })
            .collect();
        let silence = vec![[0.0; CHANNELS]; probe.len()];
        let input_refs: Vec<&[Frame]> = std::iter::once(probe.as_slice())
            .chain(std::iter::repeat_n(silence.as_slice(), NUM_STRIPS - 1))
            .collect();

        let mut out_bufs: Vec<Vec<Frame>> = vec![vec![[0.0; CHANNELS]; probe.len()]; NUM_BUSES];
        {
            let mut out_refs: Vec<&mut [Frame]> =
                out_bufs.iter_mut().map(|v| v.as_mut_slice()).collect();
            engine.process_block(&input_refs, &mut out_refs);
        }
        // After StereoReverse, whatever channel 0 became ends up in
        // channel 1.
        let actual_ch1: Vec<f32> = out_bufs[0].iter().map(|f| f[1]).collect();
        let actual_gain = goertzel_magnitude(&actual_ch1, 1000.0, 48_000.0)
            / goertzel_magnitude(&tone, 1000.0, 48_000.0);
        let actual_db = 20.0 * actual_gain.log10();

        // Hypothesis A (EQ first, the declared order): boost channel 0,
        // then swap -- channel 1 should carry the *boosted* tone. Built
        // independently with a raw `Biquad`, not by calling the engine
        // under test.
        let mut boost = Biquad::bypassed();
        boost.set_coeffs(BiquadCoeffs::peaking(48_000.0, 1000.0, 1.0, 18.0));
        let hyp_a: Vec<f32> = tone.iter().map(|&x| boost.process(x)).collect();
        let a_gain = goertzel_magnitude(&hyp_a, 1000.0, 48_000.0)
            / goertzel_magnitude(&tone, 1000.0, 48_000.0);
        let a_db = 20.0 * a_gain.log10();

        // Hypothesis B (swap first, the wrong order): channel 1 ends up
        // carrying the *original, unboosted* tone, since channel 1's own
        // EQ setting was left neutral -- unity, 0dB, by construction.
        let b_db = 0.0;

        assert!(
            (a_db - b_db).abs() > 6.0,
            "hypotheses aren't distinguishable: a={a_db}dB b={b_db}dB"
        );
        assert!(
            (actual_db - a_db).abs() < 0.5,
            "actual ({actual_db}dB) should match hypothesis A/EQ-first ({a_db}dB)"
        );
        assert!(
            (actual_db - b_db).abs() > 6.0,
            "actual ({actual_db}dB) should NOT match hypothesis B/swap-first ({b_db}dB)"
        );
    }

    #[test]
    fn realtime_process_block_does_not_allocate() {
        let mut engine = Engine::new();
        let inputs = silent_inputs(32);
        let input_refs: Vec<&[Frame]> = inputs.iter().map(|v| v.as_slice()).collect();
        let mut out_bufs: Vec<Vec<Frame>> = vec![vec![[0.0; CHANNELS]; 32]; NUM_BUSES];
        let mut out_refs: Vec<&mut [Frame]> =
            out_bufs.iter_mut().map(|v| v.as_mut_slice()).collect();

        assert_realtime(|| engine.process_block(&input_refs, &mut out_refs));
    }

    #[test]
    fn realtime_composite_mode_does_not_allocate() {
        // The composite-overlay path is new code on the audio thread --
        // this proves it, not just the default all-Normal path above.
        let mut engine = Engine::new();
        engine.buses[0].mode = BusMode::Composite;
        engine.patch.composite[0] = CompositeSource::Strip {
            strip: 1,
            channel: 0,
        };
        engine.patch.composite_post_fader = true;

        let inputs = silent_inputs(32);
        let input_refs: Vec<&[Frame]> = inputs.iter().map(|v| v.as_slice()).collect();
        let mut out_bufs: Vec<Vec<Frame>> = vec![vec![[0.0; CHANNELS]; 32]; NUM_BUSES];
        let mut out_refs: Vec<&mut [Frame]> =
            out_bufs.iter_mut().map(|v| v.as_mut_slice()).collect();

        assert_realtime(|| engine.process_block(&input_refs, &mut out_refs));
    }

    #[test]
    fn bus_mode_runs_before_eq_provably() {
        // spec 1.2 places the bus mode transform (step 3) before the bus
        // EQ (step 4). StereoRepeat and a channel-2-specific EQ boost are
        // non-commutative: pre-transform, channel 2 (FC) is silent (only
        // FL/FR carry the tone), so an EQ boost there only has something
        // to boost once StereoRepeat has already copied FL onto it.
        use crate::biquad::test_support::{goertzel_magnitude, sine};
        use crate::biquad::{Biquad, BiquadCoeffs};
        use crate::parametric_eq::{EqCellParams, EqCellType};
        use crate::CH_FC;

        let mut engine = Engine::new();
        for strip in &mut engine.strips {
            strip.bus_assign = [false; NUM_BUSES];
        }
        engine.strips[0].bus_assign[0] = true;
        engine.buses[0].mode = BusMode::StereoRepeat;
        engine.buses[0].eq.on = true;
        engine.buses[0].eq.set_cell(
            CH_FC,
            0,
            EqCellParams {
                on: true,
                cell_type: EqCellType::Peak,
                freq_hz: 1000.0,
                gain_db: 18.0,
                q: 1.0,
            },
        );

        let tone = sine(4096, 48_000.0, 1000.0);
        let probe: Vec<Frame> = tone
            .iter()
            .map(|&s| {
                let mut f = [0.0; CHANNELS];
                f[0] = s; // FR and the rest start silent
                f
            })
            .collect();
        let silence = vec![[0.0; CHANNELS]; probe.len()];
        let input_refs: Vec<&[Frame]> = std::iter::once(probe.as_slice())
            .chain(std::iter::repeat_n(silence.as_slice(), NUM_STRIPS - 1))
            .collect();

        let mut out_bufs: Vec<Vec<Frame>> = vec![vec![[0.0; CHANNELS]; probe.len()]; NUM_BUSES];
        {
            let mut out_refs: Vec<&mut [Frame]> =
                out_bufs.iter_mut().map(|v| v.as_mut_slice()).collect();
            engine.process_block(&input_refs, &mut out_refs);
        }
        let actual_fc: Vec<f32> = out_bufs[0].iter().map(|f| f[CH_FC]).collect();
        let actual_gain = goertzel_magnitude(&actual_fc, 1000.0, 48_000.0)
            / goertzel_magnitude(&tone, 1000.0, 48_000.0);
        let actual_db = 20.0 * actual_gain.log10();

        // Hypothesis A (mode first, the declared order): StereoRepeat
        // copies the tone onto FC, then EQ boosts it.
        let mut boost = Biquad::bypassed();
        boost.set_coeffs(BiquadCoeffs::peaking(48_000.0, 1000.0, 1.0, 18.0));
        let hyp_a: Vec<f32> = tone.iter().map(|&x| boost.process(x)).collect();
        let a_gain = goertzel_magnitude(&hyp_a, 1000.0, 48_000.0)
            / goertzel_magnitude(&tone, 1000.0, 48_000.0);
        let a_db = 20.0 * a_gain.log10();

        // Hypothesis B (EQ first, the wrong order): FC is silent
        // pre-transform, so boosting it boosts nothing; StereoRepeat then
        // overwrites FC with the original, unboosted tone.
        let b_db = 0.0;

        assert!(
            (a_db - b_db).abs() > 6.0,
            "hypotheses aren't distinguishable: a={a_db}dB b={b_db}dB"
        );
        assert!(
            (actual_db - a_db).abs() < 0.5,
            "actual ({actual_db}dB) should match hypothesis A/mode-first ({a_db}dB)"
        );
        assert!(
            (actual_db - b_db).abs() > 6.0,
            "actual ({actual_db}dB) should NOT match hypothesis B/EQ-first ({b_db}dB)"
        );
    }

    fn run_single_block(engine: &mut Engine, inputs: &[Vec<Frame>], len: usize) -> Vec<Vec<Frame>> {
        let input_refs: Vec<&[Frame]> = inputs.iter().map(|v| v.as_slice()).collect();
        let mut out_bufs: Vec<Vec<Frame>> = vec![vec![[0.0; CHANNELS]; len]; NUM_BUSES];
        {
            let mut out_refs: Vec<&mut [Frame]> =
                out_bufs.iter_mut().map(|v| v.as_mut_slice()).collect();
            engine.process_block(&input_refs, &mut out_refs);
        }
        out_bufs
    }

    #[test]
    fn composite_default_slot_falls_back_to_the_ordinary_sum() {
        let inputs: Vec<Vec<Frame>> = (0..NUM_STRIPS)
            .map(|s| {
                (0..8)
                    .map(|n| {
                        let mut f = [0.0; CHANNELS];
                        f[0] = (s as f32 + 1.0) * 0.01 * (n as f32 + 1.0);
                        f
                    })
                    .collect()
            })
            .collect();

        let mut normal = Engine::new();
        normal.strips[0].bus_assign[0] = true;
        normal.strips[2].bus_assign[0] = true;
        let normal_out = run_single_block(&mut normal, &inputs, 8);

        let mut composite = Engine::new();
        composite.strips[0].bus_assign[0] = true;
        composite.strips[2].bus_assign[0] = true;
        composite.buses[0].mode = BusMode::Composite; // patch is all-Default
        let composite_out = run_single_block(&mut composite, &inputs, 8);

        assert_eq!(normal_out[0], composite_out[0]);
    }

    #[test]
    fn composite_pre_fader_tap_bypasses_bus_assign_gain_layer_and_mute() {
        let mut engine = Engine::new();
        for strip in &mut engine.strips {
            strip.bus_assign = [false; NUM_BUSES];
        }
        engine.buses[0].mode = BusMode::Composite;
        engine.patch.composite[0] = CompositeSource::Strip {
            strip: 5,
            channel: 0,
        };
        // None of these should matter to a PRE-fader tap: strip 5 isn't
        // assigned to bus 0, is muted, and has a large negative gain layer.
        engine.strips[5].mute = true;
        engine.strips[5].set_gain_layer_db(0, -60.0);

        let mut probe = [0.0; CHANNELS];
        probe[0] = 0.37;
        let inputs: Vec<Vec<Frame>> = (0..NUM_STRIPS)
            .map(|s| vec![if s == 5 { probe } else { [0.0; CHANNELS] }])
            .collect();
        let out = run_single_block(&mut engine, &inputs, 1);

        assert_eq!(out[0][0][0], 0.37);
    }

    #[test]
    fn composite_post_fader_tap_reflects_the_processed_signal_and_gain_layer_and_ignores_mute() {
        use crate::strip_dsp::StripChain;

        let mut engine = Engine::new();
        for strip in &mut engine.strips {
            strip.bus_assign = [false; NUM_BUSES];
        }
        engine.buses[0].mode = BusMode::Composite;
        engine.patch.composite[0] = CompositeSource::Strip {
            strip: 0,
            channel: 0,
        };
        engine.patch.composite_post_fader = true;

        // A muted strip, so this also proves POST ignores mute (spec 1.2
        // puts the fader, step 12, before mute, step 13).
        engine.strips[0].mute = true;
        engine.strips[0].set_gain_layer_db(0, -6.0);
        if let StripChain::Hardware(chain) = &mut engine.strips[0].chain {
            chain.pan.pan = 0.5; // StereoBalance::gains: (L, R) = (0.5, 1.0)
        } else {
            panic!("strip 0 should be a hardware chain");
        }

        let mut probe = [0.0; CHANNELS];
        probe[0] = 0.8;
        let inputs: Vec<Vec<Frame>> = (0..NUM_STRIPS)
            .map(|s| vec![if s == 0 { probe } else { [0.0; CHANNELS] }])
            .collect();
        let out = run_single_block(&mut engine, &inputs, 1);

        let expected = 0.8 * 0.5 * gain_db_to_linear(-6.0);
        assert!(
            (out[0][0][0] - expected).abs() < 1e-6,
            "expected {expected}, got {}",
            out[0][0][0]
        );
    }

    #[test]
    fn composite_replaces_only_its_own_channel_not_the_whole_bus() {
        let mut engine = Engine::new();
        for strip in &mut engine.strips {
            strip.bus_assign = [false; NUM_BUSES];
        }
        engine.strips[1].bus_assign[0] = true; // ordinary contributor to bus 0
        engine.buses[0].mode = BusMode::Composite;
        // Channel 0 is patched from strip 3; channel 1 stays Default and
        // should still show strip 1's ordinary contribution.
        engine.patch.composite[0] = CompositeSource::Strip {
            strip: 3,
            channel: 0,
        };

        let mut strip1_frame = [0.0; CHANNELS];
        strip1_frame[1] = 0.55;
        let mut strip3_frame = [0.0; CHANNELS];
        strip3_frame[0] = 0.21;
        let inputs: Vec<Vec<Frame>> = (0..NUM_STRIPS)
            .map(|s| {
                vec![match s {
                    1 => strip1_frame,
                    3 => strip3_frame,
                    _ => [0.0; CHANNELS],
                }]
            })
            .collect();
        let out = run_single_block(&mut engine, &inputs, 1);

        assert_eq!(
            out[0][0][0], 0.21,
            "channel 0 should be strip 3's patched value"
        );
        assert_eq!(
            out[0][0][1], 0.55,
            "channel 1 (Default) should still carry strip 1's ordinary contribution"
        );
    }

    #[test]
    fn insert_patch_and_pre_post_switch_are_inert_this_milestone() {
        // spec 2.3 defers the actual insert send/return path past M7 (no
        // AUv3 host slot or hardware loop exists yet), so the config
        // fields exist but must not change engine output on their own --
        // same "config exists, no audio path yet" shape as M5's deferred
        // Color-pad reverb.
        let inputs: Vec<Vec<Frame>> = (0..NUM_STRIPS)
            .map(|s| {
                vec![{
                    let mut f = [0.0; CHANNELS];
                    f[0] = 0.1 * (s as f32 + 1.0);
                    f
                }]
            })
            .collect();

        let mut default_patch = Engine::new();
        let default_out = run_single_block(&mut default_patch, &inputs, 1);

        let mut all_inserted = Engine::new();
        all_inserted.patch.insert = [true; 22];
        all_inserted.patch.insert_post_fx = true;
        let inserted_out = run_single_block(&mut all_inserted, &inputs, 1);

        assert_eq!(default_out, inserted_out);
    }
}
