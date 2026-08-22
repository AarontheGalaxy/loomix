//! Clock master selection and the internal-clock fallback (spec sections
//! 1.11, 1.19). Pure: no CoreAudio, no wall-clock time, so it's testable
//! offline the same way as everything else in this module.

/// A CoreAudio device object ID.
pub type DeviceId = u32;

/// The engine's clock master. "The main output device is the clock
/// master. Everything else is resampled to it... The engine can run on an
/// internal clock with no output device selected" (spec 1.19, 1.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSource {
    Device(DeviceId),
    Internal,
}

/// Resolves the clock master from the configured main-output device and
/// the devices currently alive. Falls back to the internal clock whenever
/// the configured device isn't actually present -- none configured, or
/// configured but disconnected.
pub fn resolve_clock_source(configured: Option<DeviceId>, alive: &[DeviceId]) -> ClockSource {
    match configured {
        Some(id) if alive.contains(&id) => ClockSource::Device(id),
        _ => ClockSource::Internal,
    }
}

/// A deterministic tick source for the internal-clock fallback. Driven by
/// a logical frame counter rather than `sleep`/wall time, so it's exact
/// and instant under test.
#[derive(Debug, Clone, Copy)]
pub struct InternalClock {
    sample_rate_hz: u32,
    frames_produced: u64,
}

impl InternalClock {
    pub fn new(sample_rate_hz: u32) -> Self {
        Self {
            sample_rate_hz,
            frames_produced: 0,
        }
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    pub fn frames_produced(&self) -> u64 {
        self.frames_produced
    }

    /// Advances by one block of `block_frames` frames.
    pub fn tick(&mut self, block_frames: u32) {
        self.frames_produced += block_frames as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_device_alive_wins() {
        let source = resolve_clock_source(Some(7), &[3, 7, 9]);
        assert_eq!(source, ClockSource::Device(7));
    }

    #[test]
    fn configured_device_not_alive_falls_back_to_internal() {
        // Disconnected -- spec 1.19: "when a device disappears the engine
        // stops. Auto restart options exist for exactly this reason", but
        // resolution itself must not pick a device that isn't there.
        let source = resolve_clock_source(Some(7), &[3, 9]);
        assert_eq!(source, ClockSource::Internal);
    }

    #[test]
    fn no_device_configured_is_internal() {
        let source = resolve_clock_source(None, &[3, 7, 9]);
        assert_eq!(source, ClockSource::Internal);
    }

    #[test]
    fn does_not_confuse_the_configured_device_with_another_alive_one() {
        let source = resolve_clock_source(Some(7), &[3, 9]);
        assert_ne!(source, ClockSource::Device(3));
        assert_ne!(source, ClockSource::Device(9));
    }

    #[test]
    fn internal_clock_advances_by_exact_frame_counts_no_wall_time() {
        let mut clock = InternalClock::new(48_000);
        for _ in 0..100 {
            clock.tick(128);
        }
        assert_eq!(clock.frames_produced(), 100 * 128);
        assert_eq!(clock.sample_rate_hz(), 48_000);
    }
}
