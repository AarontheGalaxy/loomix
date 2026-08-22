//! CoreAudio integration. Populated starting M4 (device enumeration, hog
//! mode, clock master selection, drift-corrected resampling). One of the two
//! places in the workspace allowed to use `unsafe` (spec section 4.2).
//!
//! The algorithmic pieces -- clock master selection, drift estimation, the
//! resampler, hot-plug handling, hog-mode fallback -- are pure functions
//! over plain data, with no CoreAudio calls and no wall-clock time, so they
//! are covered by ordinary `cargo test` with synthetic inputs. The
//! CoreAudio glue that drives real devices stays thin on top of them and is
//! verified separately (`driver` CI job, manual hardware soak).

pub mod clock;
pub mod device;
pub mod drift;
pub mod hog;
pub mod hotplug;
pub mod resample;
