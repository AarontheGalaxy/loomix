//! The M3 engine core: 8 strips, 8 buses, the full assignment matrix, no
//! effects (spec 3.4 M3). Per-bus signal flow follows spec 1.2's per
//! output bus steps 1 (sum via gain layer), 5 (mono) and 6-7 (mute, gain);
//! steps 2-4 and 8 are FX returns, bus mode and bus EQ, which are M6-M8.

use crate::bus::{Bus, BusMono};
use crate::fader::gain_db_to_linear;
use crate::meter::Meter;
use crate::strip::Strip;
use crate::{Frame, CHANNELS, NUM_BUSES, NUM_STRIPS};

pub struct Engine {
    pub strips: [Strip; NUM_STRIPS],
    pub buses: [Bus; NUM_BUSES],
    strip_meters: [Meter; NUM_STRIPS],
    bus_meters: [Meter; NUM_BUSES],
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            strips: std::array::from_fn(|_| Strip::default()),
            buses: std::array::from_fn(|_| Bus::default()),
            strip_meters: [Meter::default(); NUM_STRIPS],
            bus_meters: [Meter::default(); NUM_BUSES],
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

    /// Renders one block. `strip_inputs[s]` and `bus_outputs[b]` must each
    /// have the same length (the block's frame count); `strip_inputs` must
    /// have [`NUM_STRIPS`] entries and `bus_outputs` [`NUM_BUSES`].
    ///
    /// Never allocates, locks or performs I/O: every buffer is caller
    /// owned, every per-strip/per-bus state array is fixed size (spec 3.3).
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

        for (b, out) in bus_outputs.iter_mut().enumerate() {
            for (s, strip) in self.strips.iter().enumerate() {
                if !strip.bus_assign[b] || strip.mute || (any_solo && !strip.solo) {
                    continue;
                }
                let gain = gain_db_to_linear(strip.gain_layer_db(b));
                if gain == 0.0 {
                    continue;
                }
                for (out_frame, in_frame) in out.iter_mut().zip(strip_inputs[s].iter()) {
                    let mut contribution = *in_frame;
                    if strip.mono {
                        sum_to_mono(&mut contribution);
                    }
                    for (o, c) in out_frame.iter_mut().zip(contribution.iter()) {
                        *o += c * gain;
                    }
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
