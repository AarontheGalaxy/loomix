//! An input strip's state: mute/solo/mono, the bus assignment matrix row,
//! the 8 per-bus gain layers (spec 1.3's `GainLayer[j]`, 1.15) from M3, plus
//! (M5 onward) the per-strip effects chain (spec 1.2's denoiser through
//! limiter steps) in [`StripChain`].

use crate::strip_dsp::StripChain;
use crate::NUM_BUSES;

/// spec 1.1's fixed Potato topology: strips 0..4 are hardware, 5..7 are
/// virtual, and among the virtual strips index 6 ("Voicemeeter Aux Input")
/// is specifically the one Karaoke (spec 1.4) applies to.
pub fn topology_is_hardware(strip_index: usize) -> bool {
    strip_index < 5
}

/// spec 1.1: "Voicemeeter Aux Input | Virtual strip 2 (strip index 6)";
/// spec 1.4: "Karaoke button ... Present on the AUX virtual strip."
pub fn topology_is_aux(strip_index: usize) -> bool {
    strip_index == 6
}

pub struct Strip {
    pub mute: bool,
    pub solo: bool,
    pub mono: bool,
    /// Bus assignment matrix row: `bus_assign[b]` is this strip's A1..A5,
    /// B1..B3 toggle for bus `b` (spec 1.3, item 14).
    pub bus_assign: [bool; NUM_BUSES],
    gain_layer_db: [f32; NUM_BUSES],
    pub chain: StripChain,
}

impl Strip {
    /// Builds strip `index` per spec 1.1's fixed Potato topology (hardware
    /// vs virtual, and which virtual strip is the Karaoke-capable AUX one).
    /// `Engine` is the only caller that should need this — everything else
    /// should just read `strip.chain`'s variant.
    pub fn for_topology_index(index: usize, sample_rate: f32) -> Self {
        let mut bus_assign = [false; NUM_BUSES];
        bus_assign[0] = true; // spec 1.3: "Bus assign ... Default A1 on"
        let chain = if topology_is_hardware(index) {
            StripChain::hardware(sample_rate)
        } else {
            StripChain::virtual_strip(sample_rate, topology_is_aux(index))
        };
        Self {
            mute: false,
            solo: false,
            mono: false,
            bus_assign,
            gain_layer_db: [0.0; NUM_BUSES], // spec 1.3: "Gain fader ... Default 0"
            chain,
        }
    }

    pub fn gain_layer_db(&self, bus: usize) -> f32 {
        self.gain_layer_db[bus]
    }

    pub fn set_gain_layer_db(&mut self, bus: usize, db: f32) {
        self.gain_layer_db[bus] = db;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn default_strip_matches_spec_defaults() {
        let strip = Strip::for_topology_index(0, SR);
        assert!(!strip.mute && !strip.solo && !strip.mono);
        assert!(strip.bus_assign[0]);
        assert!(strip.bus_assign[1..].iter().all(|&on| !on));
        assert!((0..NUM_BUSES).all(|b| strip.gain_layer_db(b) == 0.0));
    }

    #[test]
    fn gain_layers_are_independent_per_bus() {
        let mut strip = Strip::for_topology_index(0, SR);
        strip.set_gain_layer_db(3, -6.0);
        assert_eq!(strip.gain_layer_db(3), -6.0);
        assert_eq!(strip.gain_layer_db(0), 0.0);
    }

    #[test]
    fn topology_matches_spec_1_1s_potato_layout() {
        for i in 0..5 {
            assert!(topology_is_hardware(i), "strip {i} should be hardware");
        }
        for i in 5..8 {
            assert!(!topology_is_hardware(i), "strip {i} should be virtual");
        }
        assert!(
            topology_is_aux(6),
            "strip 6 (Aux Input) should be the karaoke-capable one"
        );
        assert!(!topology_is_aux(5));
        assert!(!topology_is_aux(7));
    }

    #[test]
    fn for_topology_index_builds_the_matching_chain_variant() {
        assert!(matches!(
            Strip::for_topology_index(0, SR).chain,
            crate::strip_dsp::StripChain::Hardware(_)
        ));
        assert!(matches!(
            Strip::for_topology_index(5, SR).chain,
            crate::strip_dsp::StripChain::Virtual(_)
        ));
    }
}
