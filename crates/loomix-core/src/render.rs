//! Offline deterministic rendering harness (spec 3.4 M3, spec 4.1 layer 4).
//! Renders a fixed set of per-strip input buffers through an [`Engine`] in
//! one block and hands back the per-bus output — no device, no clock, no
//! wall time, so the same inputs always produce the same outputs.
//!
//! Also holds the tone/analysis helpers the routing truth-table test (spec
//! 4.1 layer 5) uses to feed an identifiable signal per strip and confirm
//! exactly the expected signal, at exactly the expected level, lands on
//! each bus: a pure sine per strip, and a Goertzel filter (a single-bin
//! DFT) to recover one frequency's magnitude from a bus's output without
//! pulling in a full FFT crate for eight known frequencies.

use crate::{Engine, Frame, CHANNELS, NUM_BUSES, NUM_STRIPS};

/// Renders one block: `inputs[s]` is strip `s`'s audio, all buffers the
/// same length. Returns one buffer per bus, same length as the inputs.
pub fn render_block(
    engine: &mut Engine,
    inputs: &[Vec<Frame>; NUM_STRIPS],
) -> [Vec<Frame>; NUM_BUSES] {
    let len = inputs[0].len();
    let input_refs: Vec<&[Frame]> = inputs.iter().map(|v| v.as_slice()).collect();
    let mut outputs: [Vec<Frame>; NUM_BUSES] = std::array::from_fn(|_| vec![[0.0; CHANNELS]; len]);
    {
        let mut output_refs: Vec<&mut [Frame]> =
            outputs.iter_mut().map(|v| v.as_mut_slice()).collect();
        engine.process_block(&input_refs, &mut output_refs);
    }
    outputs
}

/// A full-amplitude sine tone at `freq_hz`, written to `channel` of each
/// frame (every other channel stays at digital silence).
pub fn sine_tone(frames: usize, sample_rate: f32, freq_hz: f32, channel: usize) -> Vec<Frame> {
    (0..frames)
        .map(|n| {
            let t = n as f32 / sample_rate;
            let mut frame = [0.0; CHANNELS];
            frame[channel] = (2.0 * std::f32::consts::PI * freq_hz * t).sin();
            frame
        })
        .collect()
}

/// The magnitude of `freq_hz` in `block`'s `channel`, via the Goertzel
/// algorithm. Comparable in scale to another call with the same block
/// length and sample rate, which is all the truth-table test needs: it
/// checks ratios against a reference tone rendered through the identity
/// mix, not an absolute unit.
pub fn goertzel_magnitude(block: &[Frame], channel: usize, freq_hz: f32, sample_rate: f32) -> f32 {
    let n = block.len() as f32;
    let k = (0.5 + (n * freq_hz) / sample_rate).floor();
    let omega = (2.0 * std::f32::consts::PI / n) * k;
    let coeff = 2.0 * omega.cos();
    let (mut q1, mut q2) = (0.0f32, 0.0f32);
    for frame in block {
        let q0 = coeff * q1 - q2 + frame[channel];
        q2 = q1;
        q1 = q0;
    }
    (q1 * q1 + q2 * q2 - q1 * q2 * coeff).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goertzel_finds_the_tone_it_was_given_and_nothing_else() {
        let sample_rate = 48_000.0;
        let block = sine_tone(2048, sample_rate, 1000.0, 0);
        let at_tone = goertzel_magnitude(&block, 0, 1000.0, sample_rate);
        let off_tone = goertzel_magnitude(&block, 0, 3000.0, sample_rate);
        let wrong_channel = goertzel_magnitude(&block, 1, 1000.0, sample_rate);
        assert!(at_tone > 100.0);
        assert!(off_tone < at_tone * 0.01);
        assert_eq!(wrong_channel, 0.0);
    }

    #[test]
    fn render_block_is_deterministic() {
        let mut engine = Engine::new();
        let inputs: [Vec<Frame>; NUM_STRIPS] =
            std::array::from_fn(|s| sine_tone(64, 48_000.0, 200.0 + s as f32 * 50.0, 0));
        let a = render_block(&mut engine, &inputs);
        let b = render_block(&mut engine, &inputs);
        assert_eq!(a, b);
    }
}
