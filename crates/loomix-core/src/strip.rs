//! An input strip's M3 state: the fields spec 1.2's per-strip signal flow
//! reduces to once the effects steps (denoiser, gate, comp, EQ, pan, FX
//! sends — M5 onward) are stripped out: mute, solo, mono, the bus
//! assignment matrix row, and the 8 per-bus gain layers (spec 1.3's
//! `GainLayer[j]`, 1.15).

use crate::NUM_BUSES;

#[derive(Debug, Clone)]
pub struct Strip {
    pub mute: bool,
    pub solo: bool,
    pub mono: bool,
    /// Bus assignment matrix row: `bus_assign[b]` is this strip's A1..A5,
    /// B1..B3 toggle for bus `b` (spec 1.3, item 14).
    pub bus_assign: [bool; NUM_BUSES],
    gain_layer_db: [f32; NUM_BUSES],
}

impl Default for Strip {
    fn default() -> Self {
        let mut bus_assign = [false; NUM_BUSES];
        bus_assign[0] = true; // spec 1.3: "Bus assign ... Default A1 on"
        Self {
            mute: false,
            solo: false,
            mono: false,
            bus_assign,
            gain_layer_db: [0.0; NUM_BUSES], // spec 1.3: "Gain fader ... Default 0"
        }
    }
}

impl Strip {
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

    #[test]
    fn default_strip_matches_spec_defaults() {
        let strip = Strip::default();
        assert!(!strip.mute && !strip.solo && !strip.mono);
        assert!(strip.bus_assign[0]);
        assert!(strip.bus_assign[1..].iter().all(|&on| !on));
        assert!((0..NUM_BUSES).all(|b| strip.gain_layer_db(b) == 0.0));
    }

    #[test]
    fn gain_layers_are_independent_per_bus() {
        let mut strip = Strip::default();
        strip.set_gain_layer_db(3, -6.0);
        assert_eq!(strip.gain_layer_db(3), -6.0);
        assert_eq!(strip.gain_layer_db(0), 0.0);
    }
}
