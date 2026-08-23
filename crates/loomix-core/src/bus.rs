//! An output bus's state: spec 1.2's per-bus chain reduced to what's left
//! once FX returns (M8) are out of scope — the sum of assigned strips, the
//! bus mode transform (M7, 12 modes, `bus_mode.rs`), the bus parametric EQ
//! (M6, spec 1.7: 8 independent channels), the mono button, mute and the
//! bus gain fader (spec 1.5).

use crate::bus_mode::BusMode;
use crate::parametric_eq::ParametricEq;
use crate::CHANNELS;

/// Spec 1.5: "first press sums to mono, second press swaps channels 1 and
/// 2 (stereo reverse), third press returns to off." Applies to the bus's
/// channel 0/1 pair; channels 2..7 are untouched here (bus modes that
/// reshuffle the full 8 channel layout land in M7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BusMono {
    #[default]
    Off,
    Mono,
    StereoReverse,
}

pub struct Bus {
    pub mute: bool,
    pub mono: BusMono,
    /// spec 1.5/1.6: which of the 12 bus modes transforms the summed
    /// signal, applied at spec 1.2 step 3 -- before the EQ below (step 4).
    /// `Composite` is filled directly by `Engine::process_block` from
    /// `Engine::patch`, not by `bus_mode::transform`.
    pub mode: BusMode,
    /// spec 1.7: independent EQ per channel, all 8 (unlike the strip EQ's
    /// stereo 2). spec 1.2 step 4: runs after summing/FX-returns, before
    /// the mono transform (step 5) below — see `engine.rs`'s bus loop and
    /// `engine::tests::bus_eq_runs_before_stereo_reverse_provably` for the
    /// order proof (a channel-specific EQ config and `StereoReverse` don't
    /// commute, which is why that combination is the test, not just "EQ
    /// changed the output").
    pub eq: ParametricEq<CHANNELS>,
    gain_db: f32,
}

impl Bus {
    /// `Strip::for_topology_index` dropped a plain `Default` for the same
    /// reason back in M5: the EQ's biquads/delay line are sample-rate
    /// dependent, so a bus can no longer be built without knowing the
    /// engine's current rate.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            mute: false,
            mono: BusMono::Off,
            mode: BusMode::Normal,
            eq: ParametricEq::new(sample_rate),
            gain_db: 0.0, // spec 1.5: "Gain fader ... Default" is unity, same law as strips
        }
    }

    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    pub fn set_gain_db(&mut self, db: f32) {
        self.gain_db = db;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn default_bus_is_unity_unmuted_off_and_eq_off() {
        let bus = Bus::new(SR);
        assert!(!bus.mute);
        assert_eq!(bus.mono, BusMono::Off);
        assert_eq!(bus.mode, BusMode::Normal);
        assert_eq!(bus.gain_db(), 0.0);
        assert!(!bus.eq.on);
    }
}
