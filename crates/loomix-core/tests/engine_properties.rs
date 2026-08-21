//! Layer 1's stability requirement ("no NaN, no infinity, output
//! bounded") and layer 2's "any sequence of parameter changes leaves the
//! engine in a valid state" (spec 4.1), applied to the M3 engine. M3 has
//! no per-sample persistent DSP state to destabilise over time (no
//! filters or integrators yet — those land M5 onward), so unlike a
//! filter's 10-second automation sweep, a single block already exercises
//! everything time can affect: the parameters feeding that block's
//! arithmetic, and the meters' running peak hold.

use loomix_core::bus::BusMono;
use loomix_core::render::render_block;
use loomix_core::{gain_db_to_linear, Engine, Frame, CHANNELS, NUM_BUSES, NUM_STRIPS};
use proptest::prelude::*;

fn strip_state() -> impl Strategy<Value = (bool, bool, bool, [bool; NUM_BUSES], [f32; NUM_BUSES])> {
    (
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        proptest::collection::vec(any::<bool>(), NUM_BUSES).prop_map(|v| v.try_into().unwrap()),
        proptest::collection::vec(-60.0f32..=12.0, NUM_BUSES).prop_map(|v| v.try_into().unwrap()),
    )
}

fn bus_state() -> impl Strategy<Value = (bool, u8, f32)> {
    (any::<bool>(), 0u8..3, -60.0f32..=12.0)
}

proptest! {
    #[test]
    fn any_parameter_sequence_keeps_the_engine_finite_and_bounded(
        strip_states in proptest::collection::vec(strip_state(), NUM_STRIPS),
        bus_states in proptest::collection::vec(bus_state(), NUM_BUSES),
        samples in proptest::collection::vec(-1.0f32..=1.0, 32 * CHANNELS),
    ) {
        let mut engine = Engine::new();
        for (strip, (mute, solo, mono, assign, gains)) in engine.strips.iter_mut().zip(strip_states) {
            strip.mute = mute;
            strip.solo = solo;
            strip.mono = mono;
            strip.bus_assign = assign;
            for (b, db) in gains.into_iter().enumerate() {
                strip.set_gain_layer_db(b, db);
            }
        }
        for (bus, (mute, mono_state, gain_db)) in engine.buses.iter_mut().zip(bus_states) {
            bus.mute = mute;
            bus.mono = match mono_state {
                0 => BusMono::Off,
                1 => BusMono::Mono,
                _ => BusMono::StereoReverse,
            };
            bus.set_gain_db(gain_db);
        }

        let frame_count = samples.len() / CHANNELS;
        let mut block = vec![[0.0f32; CHANNELS]; frame_count];
        for (i, frame) in block.iter_mut().enumerate() {
            for (c, sample) in frame.iter_mut().enumerate() {
                *sample = samples[i * CHANNELS + c];
            }
        }
        let inputs: [Vec<Frame>; NUM_STRIPS] = std::array::from_fn(|_| block.clone());

        let outputs = render_block(&mut engine, &inputs);

        // Worst case: every strip contributes its input (<=1.0) through
        // its loudest gain layer (+12 dB) into a bus also at +12 dB.
        let max_gain = gain_db_to_linear(12.0);
        let bound = NUM_STRIPS as f32 * max_gain * max_gain + 1e-3;

        for out in &outputs {
            for frame in out {
                for &s in frame {
                    prop_assert!(s.is_finite(), "non-finite sample: {s}");
                    prop_assert!(s.abs() <= bound, "unbounded sample: {s} > {bound}");
                }
            }
        }
        for b in 0..NUM_BUSES {
            for c in 0..CHANNELS {
                prop_assert!(engine.bus_meter(b).peak(c).is_finite());
            }
        }
    }
}
