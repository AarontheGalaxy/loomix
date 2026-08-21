//! An output bus's M3 state: spec 1.2's per-bus chain reduced to what's
//! left once bus mode (12 modes, M7), bus EQ (M6) and FX returns (M8) are
//! out of scope — the sum of assigned strips, the mono button, mute and
//! the bus gain fader (spec 1.5).

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

#[derive(Debug, Clone)]
pub struct Bus {
    pub mute: bool,
    pub mono: BusMono,
    gain_db: f32,
}

impl Default for Bus {
    fn default() -> Self {
        Self {
            mute: false,
            mono: BusMono::Off,
            gain_db: 0.0, // spec 1.5: "Gain fader ... Default" is unity, same law as strips
        }
    }
}

impl Bus {
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

    #[test]
    fn default_bus_is_unity_unmuted_off() {
        let bus = Bus::default();
        assert!(!bus.mute);
        assert_eq!(bus.mono, BusMono::Off);
        assert_eq!(bus.gain_db(), 0.0);
    }
}
