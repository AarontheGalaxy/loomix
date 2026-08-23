//! On-disk format for spec 1.7's "load and save the whole EQ set as a
//! file", and for "copy settings between strip EQs and bus EQs since the
//! parameter model is shared" when that copy crosses a save/load instead
//! of `ParametricEq::copy_all_into`'s direct live-to-live path (M6, spec
//! 3.4).

use loomix_core::parametric_eq::{EqChannelParams, ParametricEq};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// The only schema version that exists so far. A v2 gets its own
/// migration and a fixture proving v1 files still load (spec 4.1 layer 6)
/// — see `docs/ARCHITECTURE.md` for when that lands; nothing here dispatches
/// on it yet because there's nothing to dispatch to.
const CURRENT_VERSION: u32 = 1;

/// `channels.len()` is whatever the source `ParametricEq<N>` had (2 for a
/// strip, 8 for a bus) — this format doesn't fix a channel count, which is
/// what lets [`apply`](EqFile::apply) load a file saved from one channel
/// count into a `ParametricEq` of a different one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqFile {
    pub version: u32,
    pub channels: Vec<EqChannelParams>,
}

impl EqFile {
    /// Captures every channel of `eq`'s *currently active* memory (spec
    /// 1.7 doesn't distinguish A/B for the saved file itself — "load and
    /// save the whole EQ set" is read as the set currently being edited,
    /// same as every other per-memory operation in `parametric_eq.rs`).
    pub fn capture<const N: usize>(eq: &ParametricEq<N>) -> Self {
        Self {
            version: CURRENT_VERSION,
            channels: (0..N).map(|c| *eq.channel_params(c)).collect(),
        }
    }

    /// Applies this file's channels onto `eq`'s active memory, `min(
    /// self.channels.len(), N)` of them — the same shared-parameter-model
    /// cross-copy `ParametricEq::copy_all_into` does for the live case,
    /// so a strip's saved 2-channel file loads its first two channels
    /// into an 8-channel bus, and a bus's saved 8-channel file loads its
    /// first two into a 2-channel strip.
    pub fn apply<const N: usize>(&self, eq: &mut ParametricEq<N>) {
        for (c, params) in self.channels.iter().take(N).enumerate() {
            eq.set_channel_params(c, *params);
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .expect("EqFile's fields are all plain data -- serialisation cannot fail");
        std::fs::write(path, json)
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomix_core::parametric_eq::{EqCellParams, EqCellType};
    use std::path::PathBuf;

    const SR: f32 = 48_000.0;

    /// Exercises all 7 cell types across the 6 cells, plus trim and delay
    /// -- not just the default-neutral case, which round-trips trivially
    /// and wouldn't catch a field being dropped.
    fn varied_channel_params() -> EqChannelParams {
        let cell_types = [
            EqCellType::Peak,
            EqCellType::LowPass,
            EqCellType::HighPass,
            EqCellType::LowShelf,
            EqCellType::HighShelf,
            EqCellType::BandPass,
        ];
        let mut params = EqChannelParams::default();
        for (i, cell) in params.cells.iter_mut().enumerate() {
            *cell = EqCellParams {
                on: i % 2 == 0,
                cell_type: cell_types[i],
                freq_hz: 100.0 * (i as f32 + 1.0),
                gain_db: -6.0 + i as f32,
                q: 1.0 + i as f32,
            };
        }
        params.trim_db = 4.5;
        params.delay_ms = 123.0;
        params
    }

    #[test]
    fn round_trip_through_json_preserves_every_field() {
        let file = EqFile {
            version: CURRENT_VERSION,
            channels: vec![varied_channel_params(), EqChannelParams::default()],
        };
        let json = serde_json::to_string(&file).unwrap();
        let back: EqFile = serde_json::from_str(&json).unwrap();
        assert_eq!(file, back);
    }

    #[test]
    fn save_and_load_round_trip_through_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("strip.json");

        let file = EqFile {
            version: CURRENT_VERSION,
            channels: vec![varied_channel_params(), varied_channel_params()],
        };
        file.save(&path).unwrap();
        let loaded = EqFile::load(&path).unwrap();
        assert_eq!(file, loaded);
    }

    #[test]
    fn capture_then_apply_round_trips_through_a_live_eq() {
        let mut src = ParametricEq::<8>::new(SR);
        for c in 0..8 {
            src.set_channel_params(c, varied_channel_params());
        }
        let file = EqFile::capture(&src);

        let mut dst = ParametricEq::<8>::new(SR);
        file.apply(&mut dst);
        for c in 0..8 {
            assert_eq!(dst.channel_params(c), src.channel_params(c));
        }
    }

    #[test]
    fn a_two_channel_file_applied_to_an_eight_channel_eq_loads_only_the_first_two() {
        let mut strip = ParametricEq::<2>::new(SR);
        strip.set_channel_params(0, varied_channel_params());
        let mut ch1 = varied_channel_params();
        ch1.trim_db = -3.0;
        strip.set_channel_params(1, ch1);
        let file = EqFile::capture(&strip);
        assert_eq!(file.channels.len(), 2);

        let mut bus = ParametricEq::<8>::new(SR);
        for c in 0..8 {
            bus.set_trim_db(c, 99.0); // distinct marker
        }
        file.apply(&mut bus);
        assert_eq!(bus.channel_params(0), strip.channel_params(0));
        assert_eq!(bus.channel_params(1), strip.channel_params(1));
        for c in 2..8 {
            assert_eq!(
                bus.channel_params(c).trim_db,
                99.0,
                "channel {c} should be untouched by a 2-channel file"
            );
        }
    }

    #[test]
    fn an_eight_channel_file_applied_to_a_two_channel_eq_loads_only_the_first_two() {
        let mut bus = ParametricEq::<8>::new(SR);
        for c in 0..8 {
            let mut p = varied_channel_params();
            p.trim_db = c as f32;
            bus.set_channel_params(c, p);
        }
        let file = EqFile::capture(&bus);

        let mut strip = ParametricEq::<2>::new(SR);
        file.apply(&mut strip);
        assert_eq!(strip.channel_params(0).trim_db, 0.0);
        assert_eq!(strip.channel_params(1).trim_db, 1.0);
    }

    #[test]
    #[ignore] // one-shot fixture generator, not part of the regular suite
    fn print_fixture_v1_json_for_hand_review() {
        // Regenerates `testdata/fixtures/eq_v1.json`'s content on stdout,
        // same "generate deliberately, review in the diff" convention as
        // `testdata/golden/` and `testdata/bench-baseline/`. Run with
        // `cargo test -p loomix-config print_fixture_v1 -- --ignored --nocapture`.
        let mut ch0 = EqChannelParams::default();
        ch0.cells[0] = EqCellParams {
            on: true,
            cell_type: EqCellType::Peak,
            freq_hz: 1000.0,
            gain_db: 6.0,
            q: 2.0,
        };
        ch0.trim_db = 3.0;
        let ch1 = EqChannelParams {
            delay_ms: 50.0,
            ..EqChannelParams::default()
        };
        let file = EqFile {
            version: 1,
            channels: vec![ch0, ch1],
        };
        println!("{}", serde_json::to_string_pretty(&file).unwrap());
    }

    #[test]
    fn fixture_v1_file_still_loads() {
        // spec 4.1 layer 6: "every historical settings schema version has
        // a fixture file that must still load." Only v1 exists so far --
        // this is the permanent regression guard a v2 migration gets
        // checked against later, not just a throwaway round-trip.
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/fixtures/eq_v1.json");
        let file = EqFile::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(file.version, 1);
        assert_eq!(file.channels.len(), 2);
        assert!(file.channels[0].cells[0].on);
        assert_eq!(file.channels[0].cells[0].cell_type, EqCellType::Peak);
        assert_eq!(file.channels[0].cells[0].freq_hz, 1000.0);
        assert_eq!(file.channels[0].trim_db, 3.0);
        assert_eq!(file.channels[1].delay_ms, 50.0);
    }
}
