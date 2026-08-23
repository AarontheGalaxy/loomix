//! The engine core: 8 strips, 8 buses, the full assignment matrix (spec
//! 3.4 M3), plus (M5 onward) each strip's own effects chain (spec 1.2's
//! denoiser-through-limiter steps, `strip_dsp.rs`), run once per input
//! frame ahead of the per-bus summing below. Per-bus signal flow follows
//! spec 1.2's per-output-bus steps 1 (sum via gain layer), 4 (bus EQ, M6),
//! 5 (mono) and 6-7 (mute, gain); steps 2-3 are FX returns and bus mode,
//! M7-M8.

use crate::bus::{Bus, BusMono};
use crate::fader::gain_db_to_linear;
use crate::meter::Meter;
use crate::strip::Strip;
use crate::{Frame, CHANNELS, NUM_BUSES, NUM_STRIPS};

/// spec 1.11 lists 44.1/48/88.2/96/176.4/192kHz as selectable; 48kHz is
/// the engine's starting point until `loomix-hal`/`loomix-app` (M4) sets
/// the real device rate via [`Engine::set_sample_rate`].
const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;

pub struct Engine {
    pub strips: [Strip; NUM_STRIPS],
    pub buses: [Bus; NUM_BUSES],
    strip_meters: [Meter; NUM_STRIPS],
    bus_meters: [Meter; NUM_BUSES],
    sample_rate: f32,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            strips: std::array::from_fn(|i| Strip::for_topology_index(i, DEFAULT_SAMPLE_RATE)),
            buses: std::array::from_fn(|_| Bus::new(DEFAULT_SAMPLE_RATE)),
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
    /// Strips are processed outer, buses inner (not the reverse): each
    /// strip's effects chain carries mutable envelope/filter state and
    /// must run exactly once per input frame, not once per bus it feeds.
    pub fn process_block(&mut self, strip_inputs: &[&[Frame]], bus_outputs: &mut [&mut [Frame]]) {
        debug_assert_eq!(strip_inputs.len(), NUM_STRIPS);
        debug_assert_eq!(bus_outputs.len(), NUM_BUSES);

        for (meter, input) in self.strip_meters.iter_mut().zip(strip_inputs.iter()) {
            meter.observe(input);
        }

        let any_solo = self.strips.iter().any(|s| s.solo);

        for out in bus_outputs.iter_mut() {
            out.fill([0.0; CHANNELS]);
        }

        for (s, strip) in self.strips.iter_mut().enumerate() {
            if strip.mute || (any_solo && !strip.solo) {
                continue;
            }
            for (n, in_frame) in strip_inputs[s].iter().enumerate() {
                let mut processed = *in_frame;
                strip.chain.process(&mut processed);
                if strip.mono {
                    sum_to_mono(&mut processed);
                }
                for (b, out) in bus_outputs.iter_mut().enumerate() {
                    if !strip.bus_assign[b] {
                        continue;
                    }
                    let gain = gain_db_to_linear(strip.gain_layer_db(b));
                    if gain == 0.0 {
                        continue;
                    }
                    for (o, c) in out[n].iter_mut().zip(processed.iter()) {
                        *o += c * gain;
                    }
                }
            }
        }

        for (b, out) in bus_outputs.iter_mut().enumerate() {
            // spec 1.2 step 4, before step 5 (mono) below -- see
            // `tests::bus_eq_runs_before_stereo_reverse_provably` for the
            // order proof.
            for frame in out.iter_mut() {
                for (c, sample) in frame.iter_mut().enumerate() {
                    *sample = self.buses[b].eq.process_channel(c, *sample);
                }
            }

            match self.buses[b].mono {
                BusMono::Off => {}
                BusMono::Mono => out.iter_mut().for_each(sum_to_mono),
                BusMono::StereoReverse => out.iter_mut().for_each(|f| f.swap(0, 1)),
            }

            let bus_gain = if self.buses[b].mute {
                0.0
            } else {
                gain_db_to_linear(self.buses[b].gain_db())
            };
            for frame in out.iter_mut() {
                for sample in frame.iter_mut() {
                    *sample *= bus_gain;
                }
            }

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
}
