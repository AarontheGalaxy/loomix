//! Layer 5 (spec 4.1): the routing truth table. Generated, not hand
//! written — each function below loops over every combination in one
//! dimension of the M3 routing surface (the 8x8 assignment matrix, mute,
//! solo, per-bus gain layers, strip/bus mono) and, for every combination,
//! feeds a distinct identifiable tone per active strip and asserts
//! exactly the expected tones land at exactly the expected level on
//! every bus. This is the milestone's acceptance criterion.

use loomix_core::bus::BusMono;
use loomix_core::render::{goertzel_magnitude, render_block, sine_tone};
use loomix_core::{gain_db_to_linear, Engine, Frame, CHANNELS, NUM_BUSES, NUM_STRIPS};

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK_LEN: usize = 1024;
const BIN_HZ: f32 = SAMPLE_RATE / BLOCK_LEN as f32;

/// Bin-aligned frequencies (exact DFT bins at `BLOCK_LEN`, so Goertzel
/// reads a clean magnitude with no spectral leakage), one per strip,
/// spaced 7 bins apart so none of the 8 are anywhere near each other.
fn strip_freq(strip: usize) -> f32 {
    (50 + strip * 7) as f32 * BIN_HZ
}

fn silence() -> Vec<Frame> {
    vec![[0.0; CHANNELS]; BLOCK_LEN]
}

fn tone(strip: usize) -> Vec<Frame> {
    sine_tone(BLOCK_LEN, SAMPLE_RATE, strip_freq(strip), 0)
}

fn mag(block: &[Frame], strip: usize) -> f32 {
    goertzel_magnitude(block, 0, strip_freq(strip), SAMPLE_RATE)
}

/// A full-amplitude coherent tone's own magnitude — the "this strip is
/// present at unity gain" reference every assertion below compares against.
fn unity_mag() -> f32 {
    goertzel_magnitude(&tone(0), 0, strip_freq(0), SAMPLE_RATE)
}

fn assert_present(block: &[Frame], strip: usize, unity: f32, expected_linear_gain: f32) {
    let m = mag(block, strip);
    let expected = unity * expected_linear_gain;
    // Relative tolerance scaled to the expected level itself (with a small
    // absolute floor), not to unity: a near-zero expected level, like the
    // -42 dB tail of the gain-layer sweep, must not be swallowed by a
    // tolerance sized for the 0 dB case.
    let tolerance = expected * 0.02 + unity * 0.005;
    assert!(
        (m - expected).abs() < tolerance,
        "strip {strip}: expected magnitude ~{expected}, got {m}"
    );
}

fn assert_absent(block: &[Frame], strip: usize, unity: f32) {
    let m = mag(block, strip);
    assert!(
        m < unity * 0.01,
        "strip {strip}: expected silence, got magnitude {m}"
    );
}

#[test]
fn matrix_every_cell_routes_only_its_own_strip_to_only_its_own_bus() {
    let unity = unity_mag();
    for bus in 0..NUM_BUSES {
        for strip in 0..NUM_STRIPS {
            let mut engine = Engine::new();
            for s in &mut engine.strips {
                s.bus_assign = [false; NUM_BUSES];
            }
            engine.strips[strip].bus_assign[bus] = true;

            let inputs: [Vec<Frame>; NUM_STRIPS] =
                std::array::from_fn(|s| if s == strip { tone(s) } else { silence() });
            let outputs = render_block(&mut engine, &inputs);

            for (b, out) in outputs.iter().enumerate() {
                if b == bus {
                    assert_present(out, strip, unity, 1.0);
                } else {
                    assert_absent(out, strip, unity);
                }
            }
        }
    }
}

#[test]
fn mute_silences_a_strip_on_every_bus_it_is_assigned_to() {
    let unity = unity_mag();
    for mask in 0u32..256 {
        let mut engine = Engine::new();
        for (s, strip) in engine.strips.iter_mut().enumerate() {
            strip.bus_assign = [true; NUM_BUSES];
            strip.mute = mask & (1 << s) != 0;
        }

        let inputs: [Vec<Frame>; NUM_STRIPS] = std::array::from_fn(tone);
        let outputs = render_block(&mut engine, &inputs);

        for out in &outputs {
            for s in 0..NUM_STRIPS {
                if mask & (1 << s) != 0 {
                    assert_absent(out, s, unity);
                } else {
                    assert_present(out, s, unity, 1.0);
                }
            }
        }
    }
}

#[test]
fn solo_silences_every_non_soloed_strip_when_any_strip_is_soloed() {
    let unity = unity_mag();
    for mask in 0u32..256 {
        let mut engine = Engine::new();
        for (s, strip) in engine.strips.iter_mut().enumerate() {
            strip.bus_assign = [true; NUM_BUSES];
            strip.solo = mask & (1 << s) != 0;
        }
        let any_solo = mask != 0;

        let inputs: [Vec<Frame>; NUM_STRIPS] = std::array::from_fn(tone);
        let outputs = render_block(&mut engine, &inputs);

        for out in &outputs {
            for s in 0..NUM_STRIPS {
                let soloed = mask & (1 << s) != 0;
                if any_solo && !soloed {
                    assert_absent(out, s, unity);
                } else {
                    assert_present(out, s, unity, 1.0);
                }
            }
        }
    }
}

#[test]
fn each_bus_reads_its_own_independent_gain_layer_for_a_strip() {
    let unity = unity_mag();
    let mut engine = Engine::new();
    engine.strips[0].bus_assign = [true; NUM_BUSES];
    let gains_db: [f32; NUM_BUSES] = std::array::from_fn(|b| -6.0 * b as f32);
    for (b, db) in gains_db.iter().enumerate() {
        engine.strips[0].set_gain_layer_db(b, *db);
    }

    let inputs: [Vec<Frame>; NUM_STRIPS] =
        std::array::from_fn(|s| if s == 0 { tone(0) } else { silence() });
    let outputs = render_block(&mut engine, &inputs);

    for (b, out) in outputs.iter().enumerate() {
        assert_present(out, 0, unity, gain_db_to_linear(gains_db[b]));
    }
}

#[test]
fn bus_mono_sums_channel_zero_and_one_onto_both() {
    let unity = unity_mag();
    let mut engine = Engine::new();
    engine.strips[0].bus_assign = [false; NUM_BUSES];
    engine.strips[0].bus_assign[0] = true;
    engine.strips[1].bus_assign = [false; NUM_BUSES];
    engine.strips[1].bus_assign[0] = true;
    engine.buses[0].mono = BusMono::Mono;

    let strip0_on_ch0 = sine_tone(BLOCK_LEN, SAMPLE_RATE, strip_freq(0), 0);
    let strip1_on_ch1 = sine_tone(BLOCK_LEN, SAMPLE_RATE, strip_freq(1), 1);
    let inputs: [Vec<Frame>; NUM_STRIPS] = std::array::from_fn(|s| match s {
        0 => strip0_on_ch0.clone(),
        1 => strip1_on_ch1.clone(),
        _ => silence(),
    });
    let outputs = render_block(&mut engine, &inputs);

    let ch0_freq0 = goertzel_magnitude(&outputs[0], 0, strip_freq(0), SAMPLE_RATE);
    let ch1_freq0 = goertzel_magnitude(&outputs[0], 1, strip_freq(0), SAMPLE_RATE);
    let ch0_freq1 = goertzel_magnitude(&outputs[0], 0, strip_freq(1), SAMPLE_RATE);
    let ch1_freq1 = goertzel_magnitude(&outputs[0], 1, strip_freq(1), SAMPLE_RATE);
    for m in [ch0_freq0, ch1_freq0, ch0_freq1, ch1_freq1] {
        assert!(
            (m - unity * 0.5).abs() < unity * 0.02,
            "expected half-amplitude on both channels, got {m}"
        );
    }
}

#[test]
fn bus_stereo_reverse_swaps_channel_zero_and_one_exactly() {
    let mut engine = Engine::new();
    engine.strips[0].bus_assign = [false; NUM_BUSES];
    engine.strips[0].bus_assign[0] = true;
    engine.buses[0].mono = BusMono::StereoReverse;

    let mut input = silence();
    for (n, frame) in input.iter_mut().enumerate() {
        frame[0] = n as f32 * 0.001;
        frame[1] = -(n as f32) * 0.002;
    }
    let inputs: [Vec<Frame>; NUM_STRIPS] =
        std::array::from_fn(|s| if s == 0 { input.clone() } else { silence() });
    let outputs = render_block(&mut engine, &inputs);

    for (out_frame, in_frame) in outputs[0].iter().zip(input.iter()) {
        assert_eq!(out_frame[0], in_frame[1]);
        assert_eq!(out_frame[1], in_frame[0]);
    }
}

#[test]
fn strip_mono_sums_its_own_channel_zero_and_one_before_the_bus() {
    let unity = unity_mag();
    let mut engine = Engine::new();
    engine.strips[0].bus_assign = [false; NUM_BUSES];
    engine.strips[0].bus_assign[0] = true;
    engine.strips[0].mono = true;

    let mut input = silence();
    let strip0_ch0 = sine_tone(BLOCK_LEN, SAMPLE_RATE, strip_freq(0), 0);
    let strip0_ch1 = sine_tone(BLOCK_LEN, SAMPLE_RATE, strip_freq(1), 1);
    for n in 0..BLOCK_LEN {
        input[n][0] = strip0_ch0[n][0];
        input[n][1] = strip0_ch1[n][1];
    }
    let inputs: [Vec<Frame>; NUM_STRIPS] =
        std::array::from_fn(|s| if s == 0 { input.clone() } else { silence() });
    let outputs = render_block(&mut engine, &inputs);

    let ch0_freq0 = goertzel_magnitude(&outputs[0], 0, strip_freq(0), SAMPLE_RATE);
    let ch1_freq0 = goertzel_magnitude(&outputs[0], 1, strip_freq(0), SAMPLE_RATE);
    let ch0_freq1 = goertzel_magnitude(&outputs[0], 0, strip_freq(1), SAMPLE_RATE);
    let ch1_freq1 = goertzel_magnitude(&outputs[0], 1, strip_freq(1), SAMPLE_RATE);
    for m in [ch0_freq0, ch1_freq0, ch0_freq1, ch1_freq1] {
        assert!(
            (m - unity * 0.5).abs() < unity * 0.02,
            "expected half-amplitude on both channels, got {m}"
        );
    }
}
