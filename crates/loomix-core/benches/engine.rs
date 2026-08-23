//! Spec 4.1 layer 9's full-engine benchmark, owed since M3 (see
//! `rt_assert.rs`'s placeholder comment). Configured as a "mostly muted"
//! mixer, not an all-8-active one: every strip carries real, engaged
//! processing (gate/compressor/denoiser/EQ all on, not neutral/bypassed),
//! but only one of the eight is actually unmuted -- the shape M7's
//! composite/mute-scoping decision (`docs/ARCHITECTURE.md`) was measured
//! against, since that's the realistic case for a musician who leaves most
//! strips configured but idle.

use criterion::{criterion_group, criterion_main, Criterion};
use loomix_core::strip_dsp::StripChain;
use loomix_core::{Engine, Frame, CHANNELS, NUM_BUSES, NUM_STRIPS};

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK_LEN: usize = 128;

fn loaded_engine() -> Engine {
    let mut engine = Engine::new();
    for (s, strip) in engine.strips.iter_mut().enumerate() {
        match &mut strip.chain {
            StripChain::Hardware(chain) => {
                chain.gate.set_knob(5.0);
                chain.compressor.set_knob(5.0);
                chain.denoiser.set_knob(5.0);
                chain.eq.on = true;
            }
            StripChain::Virtual(chain) => {
                chain.eq.set_gains(3.0, -3.0, 2.0);
            }
        }
        strip.bus_assign = [true; NUM_BUSES];
        strip.mute = s != 0; // only strip 0 stays unmuted
    }
    engine
}

fn tone_block() -> Vec<Frame> {
    (0..BLOCK_LEN)
        .map(|n| {
            let s = (2.0 * std::f32::consts::PI * 440.0 * n as f32 / SAMPLE_RATE).sin() * 0.5;
            let mut f = [0.0; CHANNELS];
            f[0] = s;
            f[1] = s;
            f
        })
        .collect()
}

fn bench_mostly_muted(c: &mut Criterion) {
    let mut engine = loaded_engine();
    let block = tone_block();
    let inputs: Vec<Vec<Frame>> = (0..NUM_STRIPS).map(|_| block.clone()).collect();
    let input_refs: Vec<&[Frame]> = inputs.iter().map(|v| v.as_slice()).collect();
    let mut out_bufs: Vec<Vec<Frame>> = vec![vec![[0.0; CHANNELS]; BLOCK_LEN]; NUM_BUSES];

    c.bench_function("engine_process_block_mostly_muted", |b| {
        b.iter(|| {
            let mut out_refs: Vec<&mut [Frame]> =
                out_bufs.iter_mut().map(|v| v.as_mut_slice()).collect();
            engine.process_block(&input_refs, &mut out_refs);
        });
    });
}

criterion_group!(benches, bench_mostly_muted);
criterion_main!(benches);
