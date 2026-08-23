//! The macro-knob curve shared by Gate, Compressor and Denoiser (spec 1.3:
//! each is "a rotary, 0..10, simplified macro over the full detail
//! parameters"). Voicemeeter's own internal curve is not published anywhere
//! in the reference manual this project verifies against (spec front
//! matter, "every parameter range below comes from the published Remote API
//! parameter tables" — ranges only, not the macro-to-detail mapping) — this
//! is Loomix's own documented curve, not a reproduction of theirs. The
//! per-block mapping tables live in `docs/DSP.md`.
//!
//! `knob <= 0.0` always means fully bypassed (spec 4.1's null-test
//! requirement: every effect needs a true neutral setting). Above that,
//! [`fraction`] gives a plain 0..1 progress along the 0..10 range that each
//! block's `set_knob` linearly interpolates its own detail parameters
//! against — one shared curve shape, no per-block special casing.

/// `0.0` at `knob <= 0.0`, `1.0` at `knob >= 10.0`, linear between.
pub fn fraction(knob: f32) -> f32 {
    (knob / 10.0).clamp(0.0, 1.0)
}

/// Linear interpolation, `t` expected in `0.0..=1.0` (a [`fraction`] output).
pub fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_endpoints() {
        assert_eq!(fraction(0.0), 0.0);
        assert_eq!(fraction(10.0), 1.0);
        assert_eq!(fraction(5.0), 0.5);
    }

    #[test]
    fn fraction_clamps_out_of_range_knob_values() {
        assert_eq!(fraction(-3.0), 0.0);
        assert_eq!(fraction(20.0), 1.0);
    }

    #[test]
    fn fraction_is_monotonic() {
        let mut prev = fraction(0.0);
        let mut knob = 0.1;
        while knob <= 10.0 {
            let cur = fraction(knob);
            assert!(cur >= prev);
            prev = cur;
            knob += 0.1;
        }
    }

    #[test]
    fn lerp_endpoints() {
        assert_eq!(lerp(-60.0, -10.0, 0.0), -60.0);
        assert_eq!(lerp(-60.0, -10.0, 1.0), -10.0);
    }
}
