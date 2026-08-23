//! Settings and preset serialisation. Populated as each milestone's state
//! needs persisting; presets must never carry device, system, MIDI or
//! network configuration (spec section 1.16).
#![forbid(unsafe_code)]

pub mod eq_file;
