//! The fader law shared by every gain control in the engine (spec 1.3, 1.5:
//! strip gain, bus gain, gain layers). One pair of pure functions, no
//! lookup table, so "continuous" and "-inf is silence" fall out of the
//! closed-form math instead of needing special-cased edges.

/// Converts a dB value to a linear amplitude multiplier. `db` may be
/// [`f32::NEG_INFINITY`], which yields exactly `0.0` (IEEE 754: `10^-inf`).
pub fn gain_db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// The inverse of [`gain_db_to_linear`]. `0.0` yields exactly
/// [`f32::NEG_INFINITY`] (IEEE 754: `log(0)`).
pub fn gain_linear_to_db(linear: f32) -> f32 {
    20.0 * linear.log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn unity_gain_is_zero_db() {
        assert!((gain_db_to_linear(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn known_answers_match_the_cookbook_formula() {
        // -6.0206 dB is the textbook half-amplitude point.
        assert!((gain_db_to_linear(-6.0206) - 0.5).abs() < 1e-4);
        assert!((gain_db_to_linear(20.0) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn negative_infinity_is_exact_digital_silence() {
        assert_eq!(gain_db_to_linear(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn zero_linear_round_trips_to_negative_infinity() {
        let db = gain_linear_to_db(0.0);
        assert!(db.is_infinite() && db.is_sign_negative());
    }

    proptest! {
        #[test]
        fn monotonic_across_the_fader_range(a in -60.0f32..=12.0, b in -60.0f32..=12.0) {
            if a < b {
                prop_assert!(gain_db_to_linear(a) < gain_db_to_linear(b));
            }
        }

        #[test]
        fn continuous_across_the_fader_range(db in -60.0f32..=12.0, delta in 1e-4f32..1e-2) {
            // A small change in dB must produce a small, bounded change in
            // linear gain: no jump discontinuities anywhere in range.
            let a = gain_db_to_linear(db);
            let b = gain_db_to_linear(db + delta);
            prop_assert!((b - a).abs() < delta * 2.0);
        }

        #[test]
        fn round_trips_within_tolerance(db in -60.0f32..=12.0) {
            let round_tripped = gain_linear_to_db(gain_db_to_linear(db));
            prop_assert!((round_tripped - db).abs() < 1e-3);
        }
    }
}
