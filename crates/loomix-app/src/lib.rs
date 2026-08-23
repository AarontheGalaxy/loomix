//! Tauri backend. The `[[bin]]` entry point and the Tauri dependency itself
//! land with the first milestone that needs a UI surface.
#![forbid(unsafe_code)]

pub mod control;
pub mod device_wiring;
pub mod engine_io;
