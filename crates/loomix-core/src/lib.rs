//! Pure DSP and mixing engine, described in `docs/SPEC.md`. M3 (spec 3.4)
//! adds the engine core: 8 strips, 8 buses, the assignment matrix,
//! per-bus gain layers, mute, solo, mono, the fader law and metering, with
//! no effects processing yet — those land M5 onward.

// Unsafe is needed to implement `GlobalAlloc` for the test-only allocator in
// `rt_assert`, so the forbid only applies to the shipped (non-test) build.
#![cfg_attr(not(test), forbid(unsafe_code))]

pub mod bus;
pub mod engine;
pub mod fader;
pub mod meter;
pub mod render;
pub mod rt_assert;
pub mod strip;

pub use bus::Bus;
pub use engine::Engine;
pub use fader::{gain_db_to_linear, gain_linear_to_db};
pub use meter::Meter;
pub use strip::Strip;

/// Potato-level parity (spec 1.1): 8 hardware/virtual input strips.
pub const NUM_STRIPS: usize = 8;
/// Potato-level parity (spec 1.1): 5 physical (A1..A5) + 3 virtual
/// (B1..B3) output buses.
pub const NUM_BUSES: usize = 8;
/// Channels per bus (spec 1.1): `FL FR FC SW RL RR SL SR`.
pub const CHANNELS: usize = 8;

/// One sample per channel, one instant in time.
pub type Frame = [f32; CHANNELS];
