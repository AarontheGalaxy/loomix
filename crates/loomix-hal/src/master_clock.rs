//! The clock master's frame position (spec 1.19: "the main output device
//! is the clock master. Everything else is resampled to it."), published
//! lock-free so every other device's real-time IOProc thread can compute
//! its own drift error against it without a mutex (spec 3.3).

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct MasterClock {
    frames: AtomicU64,
}

impl MasterClock {
    /// Called once per callback by the master device's IOProc (or the
    /// internal-clock tick, spec 1.11, when no device is selected).
    pub fn advance(&self, frames: u32) {
        self.frames.fetch_add(frames as u64, Ordering::Relaxed);
    }

    /// Called by every other device's IOProc to measure its own drift.
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomix_core::rt_assert::assert_realtime;

    #[test]
    fn advances_by_exact_frame_counts() {
        let clock = MasterClock::default();
        clock.advance(128);
        clock.advance(128);
        assert_eq!(clock.frames(), 256);
    }

    #[test]
    fn realtime_advance_and_read_do_not_allocate() {
        let clock = MasterClock::default();
        assert_realtime(|| {
            clock.advance(128);
            clock.frames()
        });
    }
}
