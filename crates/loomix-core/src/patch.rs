//! Spec 1.11's System Settings patch config: the composite bus patch and
//! the insert patch, both shown once in that one dialog rather than once
//! per bus — spec 1.15's single `Patch.*` remote-parameter namespace
//! reflects the same thing. Any bus set to `BusMode::Composite`
//! (`bus_mode.rs`) reads this same, shared [`Patch::composite`].
//!
//! Spec 1.11/1.15 number composite/insert sources as a flat `0..22` index —
//! Voicemeeter's own count of raw hardware input channels across its
//! (Windows/ASIO-specific) hardware strips, which has no equivalent
//! meaning here: this engine already gives every strip a fixed 8-channel
//! `Frame` regardless of role (spec 3.4 M3's log). [`CompositeSource`]
//! addresses a source directly as `(strip, channel)` against that existing
//! model instead of inventing an undocumented mapping from a number this
//! codebase has no other use for — see `docs/ARCHITECTURE.md`'s M7 entry.

use crate::CHANNELS;

/// One composite-patch slot's source (spec 1.11: "index 0 means the
/// default bus channel, 1 to 22 select an input channel").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompositeSource {
    /// Falls back to whatever the bus's ordinary strip-assignment sum
    /// already produced for this channel (`engine.rs`'s Composite fill).
    #[default]
    Default,
    Strip {
        strip: usize,
        channel: usize,
    },
}

/// spec 1.11's patch settings. Global, not per-bus.
pub struct Patch {
    pub composite: [CompositeSource; CHANNELS],
    /// spec 1.11 "Composite pre fader or post fader switch"; spec 1.15
    /// `Patch.PostFaderComposite`. Spec states no default for this switch;
    /// `false` (PRE) matches the order both places list the two options in.
    pub composite_post_fader: bool,
    /// spec 1.11 "Insert patch: an on/off toggle for each of the 22 input
    /// channels"; spec 1.15 `Patch.insert[k]` (0 to 21). Config only this
    /// milestone: spec 2.3 defers the actual send/return path (an AUv3
    /// host slot or a hardware channel loop) past M7, so these toggles
    /// have no audio effect yet — same deferral shape as M5's Color-pad
    /// reverb, see `docs/ARCHITECTURE.md`.
    pub insert: [bool; 22],
    /// spec 1.11 "Insert point pre FX or post FX switch"; spec 1.15
    /// `Patch.PostFxInsert`. Same "no stated default, PRE listed first"
    /// reasoning as `composite_post_fader`.
    pub insert_post_fx: bool,
}

impl Default for Patch {
    fn default() -> Self {
        Self {
            composite: [CompositeSource::Default; CHANNELS],
            composite_post_fader: false,
            insert: [false; 22],
            insert_post_fx: false,
        }
    }
}

impl Patch {
    /// True if `strip` is the source of any non-default composite slot.
    /// `Engine::process_block` uses this to decide whether a muted strip's
    /// chain still has to run so a POST-fader composite tap sees real
    /// audio rather than silence — see `docs/ARCHITECTURE.md`'s M7 entry
    /// for why this is scoped to exactly the strips that need it rather
    /// than run unconditionally.
    pub fn composite_references_strip(&self, strip: usize) -> bool {
        self.composite
            .iter()
            .any(|slot| matches!(slot, CompositeSource::Strip { strip: s, .. } if *s == strip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_patch_is_all_default_slots_and_switches_off() {
        let patch = Patch::default();
        assert!(patch
            .composite
            .iter()
            .all(|&s| s == CompositeSource::Default));
        assert!(!patch.composite_post_fader);
        assert!(patch.insert.iter().all(|&on| !on));
        assert!(!patch.insert_post_fx);
    }

    #[test]
    fn composite_references_strip_only_true_for_a_referenced_strip() {
        let mut patch = Patch::default();
        patch.composite[3] = CompositeSource::Strip {
            strip: 5,
            channel: 0,
        };
        assert!(patch.composite_references_strip(5));
        assert!(!patch.composite_references_strip(0));
        assert!(!patch.composite_references_strip(3)); // slot index isn't the strip index
    }

    #[test]
    fn all_default_slots_reference_no_strip() {
        let patch = Patch::default();
        for s in 0..8 {
            assert!(!patch.composite_references_strip(s));
        }
    }
}
