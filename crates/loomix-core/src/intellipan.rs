//! Intellipan (spec 1.2 step 8, spec 1.3, hardware strips only): three
//! mutually exclusive pad modes — Color, Position, Modulation — one active
//! at a time, switching resets the other two modes' internal state (no
//! state leaks between them).
//!
//! **Reverb/room-effect scope, decided explicitly, not silently dropped:**
//! spec 1.3 describes Color as "3 band tonal shaping plus a small reverb on
//! the upper half" and Position as "binaural placement with a small room
//! effect". Building a strip-local reverb now, ahead of M9's real
//! send/return reverb engine, means a second reverb implementation and a
//! migration later — skipped on direct instruction for Color, and the same
//! reasoning applies identically to Position's room effect (also a small
//! reverb-family algorithm), so it's skipped here too and logged the same
//! way. **`ColorPad`'s `y` and `PositionPad`'s `y` are accepted and stored
//! but do not affect audio this milestone** — `y == 0.0` neutrality does
//! *not* prove the reverb/room path works, because there is no reverb/room
//! path yet. Both land when M9's reverb engine exists.

use crate::biquad::{Biquad, BiquadCoeffs};
use crate::Frame;

const TILT_BASS_HZ: f32 = 200.0;
const TILT_TREBLE_HZ: f32 = 5000.0;
const MAX_TILT_DB: f32 = 12.0;

/// Color mode: a bass/treble tilt driven by `x` (a single axis standing in
/// for spec's "3 band tonal shaping" — see `docs/DSP.md`). `y` is reserved
/// for the deferred reverb (see module doc).
#[derive(Default)]
pub struct ColorPad {
    pub x: f32,
    pub y: f32,
    sample_rate: f32,
    bass_l: Biquad,
    bass_r: Biquad,
    treble_l: Biquad,
    treble_r: Biquad,
}

impl ColorPad {
    pub fn new(sample_rate: f32) -> Self {
        let mut p = Self {
            sample_rate,
            ..Default::default()
        };
        p.recompute();
        p
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    pub fn set_position(&mut self, x: f32, y: f32) {
        self.x = x.clamp(-0.5, 0.5);
        self.y = y.clamp(0.0, 1.0);
        self.recompute();
    }

    /// Returns to the just-constructed state, in place -- no allocation,
    /// unlike constructing a fresh `ColorPad` and moving it in. Used by
    /// [`IntellipanPads::set_mode`] so switching pad modes on the audio
    /// thread (spec 3.3) never leaks a previous session's position.
    pub fn reset(&mut self) {
        self.x = 0.0;
        self.y = 0.0;
        self.recompute();
    }

    fn recompute(&mut self) {
        let tilt_db = (self.x / 0.5) * MAX_TILT_DB;
        if tilt_db == 0.0 {
            self.bass_l.bypass();
            self.bass_r.bypass();
            self.treble_l.bypass();
            self.treble_r.bypass();
            return;
        }
        let bass = BiquadCoeffs::peaking(self.sample_rate, TILT_BASS_HZ, 0.7, -tilt_db);
        let treble = BiquadCoeffs::peaking(self.sample_rate, TILT_TREBLE_HZ, 0.7, tilt_db);
        self.bass_l.set_coeffs(bass);
        self.bass_r.set_coeffs(bass);
        self.treble_l.set_coeffs(treble);
        self.treble_r.set_coeffs(treble);
    }

    pub fn process(&mut self, frame: &mut Frame) {
        frame[0] = self.treble_l.process(self.bass_l.process(frame[0]));
        frame[1] = self.treble_r.process(self.bass_r.process(frame[1]));
    }
}

/// Position mode: binaural placement via interaural level + time
/// difference. `y` is reserved for the deferred room effect.
pub struct PositionPad {
    pub x: f32,
    pub y: f32,
    sample_rate: f32,
    buf_l: [f32; Self::BUF_LEN],
    buf_r: [f32; Self::BUF_LEN],
    write_idx: usize,
}

impl PositionPad {
    const BUF_LEN: usize = 2048;
    /// A generous, not physiologically literal, max interaural delay —
    /// real ITD tops out around 0.6-0.7ms; this pad is a placement
    /// control, not a binaural-accuracy claim.
    const MAX_ITD_MS: f32 = 1.0;

    pub fn new(sample_rate: f32) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            sample_rate,
            buf_l: [0.0; Self::BUF_LEN],
            buf_r: [0.0; Self::BUF_LEN],
            write_idx: 0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    pub fn set_position(&mut self, x: f32, y: f32) {
        self.x = x.clamp(-0.5, 0.5);
        self.y = y.clamp(0.0, 1.0);
    }

    /// Returns to the just-constructed state, in place -- see
    /// [`ColorPad::reset`]. `.fill()` on the existing arrays rather than
    /// assigning a fresh `[0.0; BUF_LEN]` literal, so this never
    /// materialises a second BUF_LEN-sized array on the stack.
    pub fn reset(&mut self) {
        self.x = 0.0;
        self.y = 0.0;
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.write_idx = 0;
    }

    pub fn process(&mut self, frame: &mut Frame) {
        let (gl, gr) = crate::pan::StereoBalance::gains(self.x * 2.0);
        self.buf_l[self.write_idx] = frame[0];
        self.buf_r[self.write_idx] = frame[1];

        // x < 0 (left): the left channel arrives first, so delay the right
        // channel (it lags); x > 0: delay the left channel. x == 0: no
        // delay on either side — a guaranteed identity read, not a
        // near-zero fractional one.
        let delay_samples = (self.x.abs() / 0.5) * (Self::MAX_ITD_MS / 1000.0) * self.sample_rate;
        let (left_delay, right_delay) = if self.x < 0.0 {
            (0.0, delay_samples)
        } else {
            (delay_samples, 0.0)
        };

        frame[0] = gl * self.read_delayed(&self.buf_l, left_delay);
        frame[1] = gr * self.read_delayed(&self.buf_r, right_delay);

        self.write_idx = (self.write_idx + 1) % Self::BUF_LEN;
    }

    fn read_delayed(&self, buf: &[f32; Self::BUF_LEN], delay_samples: f32) -> f32 {
        if delay_samples <= 0.0 {
            return buf[self.write_idx];
        }
        let d0 = delay_samples.floor();
        let frac = delay_samples - d0;
        let len = Self::BUF_LEN as isize;
        let idx = |back: isize| -> usize {
            (((self.write_idx as isize - back) % len + len) % len) as usize
        };
        let a = buf[idx(d0 as isize)];
        let b = buf[idx(d0 as isize + 1)];
        a + (b - a) * frac
    }
}

/// Modulation mode: a modulated (chorus-style) delay line; `fx_x < 0`
/// (left half of the pad, per spec) enables feedback, `fx_y` is depth.
/// ponytail: one modulated-delay algorithm stands in for spec's "chorus,
/// phasing, feedback modulation" as a single 2D-pad effect rather than
/// three discrete DSP algorithms — a genuinely different phaser (allpass
/// cascade) is the upgrade path if this reads as too similar across the
/// pad's range.
pub struct ModulationPad {
    pub x: f32,
    pub y: f32,
    sample_rate: f32,
    buf_l: [f32; Self::BUF_LEN],
    buf_r: [f32; Self::BUF_LEN],
    write_idx: usize,
    phase: f32,
}

impl ModulationPad {
    const BUF_LEN: usize = 8192;
    const LFO_RATE_HZ: f32 = 0.8;
    const BASE_DELAY_MS: f32 = 15.0;
    const MAX_DEPTH_MS: f32 = 8.0;
    const MAX_FEEDBACK: f32 = 0.5;

    pub fn new(sample_rate: f32) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            sample_rate,
            buf_l: [0.0; Self::BUF_LEN],
            buf_r: [0.0; Self::BUF_LEN],
            write_idx: 0,
            phase: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    pub fn set_position(&mut self, x: f32, y: f32) {
        self.x = x.clamp(-0.5, 0.5);
        self.y = y.clamp(0.0, 1.0);
    }

    /// Returns to the just-constructed state, in place -- see
    /// [`ColorPad::reset`] / [`PositionPad::reset`].
    pub fn reset(&mut self) {
        self.x = 0.0;
        self.y = 0.0;
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.write_idx = 0;
        self.phase = 0.0;
    }

    pub fn process(&mut self, frame: &mut Frame) {
        if self.y <= 0.0 {
            return;
        }

        let feedback = if self.x < 0.0 {
            (-self.x / 0.5) * Self::MAX_FEEDBACK
        } else {
            0.0
        };

        let lfo = (self.phase * 2.0 * std::f32::consts::PI).sin();
        self.phase += Self::LFO_RATE_HZ / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let depth_ms = self.y * Self::MAX_DEPTH_MS;
        let delay_samples = ((Self::BASE_DELAY_MS + lfo * depth_ms) / 1000.0) * self.sample_rate;

        let out_l = self.read_delayed(&self.buf_l, delay_samples);
        let out_r = self.read_delayed(&self.buf_r, delay_samples);

        self.buf_l[self.write_idx] = frame[0] + out_l * feedback;
        self.buf_r[self.write_idx] = frame[1] + out_r * feedback;
        self.write_idx = (self.write_idx + 1) % Self::BUF_LEN;

        frame[0] = (frame[0] + out_l) * 0.5;
        frame[1] = (frame[1] + out_r) * 0.5;
    }

    fn read_delayed(&self, buf: &[f32; Self::BUF_LEN], delay_samples: f32) -> f32 {
        let d0 = delay_samples.floor().max(0.0);
        let frac = delay_samples - d0;
        let len = Self::BUF_LEN as isize;
        let idx = |back: isize| -> usize {
            (((self.write_idx as isize - back) % len + len) % len) as usize
        };
        let a = buf[idx(d0 as isize)];
        let b = buf[idx(d0 as isize + 1)];
        a + (b - a) * frac
    }
}

/// Spec 1.18: "right click the 2D pad, cycle Color, Position, Modulation" —
/// a live UI control (M10), not just a construction-time choice, so mode
/// switching has to happen on the audio thread (spec 3.3) without
/// allocating. `IntellipanPads` (below) is the RT-safe answer: all three
/// pads are allocated once and kept alive permanently, `mode` just picks
/// which one `process`/`set_position` dispatch to — switching never boxes
/// a new pad the way an enum-of-boxes swap would.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntellipanMode {
    #[default]
    Color,
    Position,
    Modulation,
}

/// All three pads are boxed individually and held permanently (not an
/// enum of boxes any more — see [`IntellipanMode`]'s doc comment for why):
/// `Position`/`Modulation`'s delay-line buffers (2048 and 8192 `f32`
/// samples respectively, sized for high sample rates) and `Color`'s
/// coefficient-ramping biquads are each big enough on their own that an
/// *unboxed* struct field would blow the parent `HardwareChain`'s stack
/// footprint across 8 strips — the same reasoning the old boxed-enum
/// design used, just three permanent boxes instead of one swapped box.
/// Constructing all three costs one extra allocation per hardware strip
/// (5 strips, done once at startup, spec 3.3 only forbids allocating on
/// the audio thread) in exchange for mode switching being a pure
/// discriminant write plus an in-place [`ColorPad::reset`]-family call —
/// zero allocation, provable the same way every other real-time claim in
/// this crate is (`tests::switching_mode_does_not_allocate`).
pub struct IntellipanPads {
    mode: IntellipanMode,
    color: Box<ColorPad>,
    position: Box<PositionPad>,
    modulation: Box<ModulationPad>,
}

impl IntellipanPads {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            mode: IntellipanMode::default(),
            color: Box::new(ColorPad::new(sample_rate)),
            position: Box::new(PositionPad::new(sample_rate)),
            modulation: Box::new(ModulationPad::new(sample_rate)),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.color.set_sample_rate(sample_rate);
        self.position.set_sample_rate(sample_rate);
        self.modulation.set_sample_rate(sample_rate);
    }

    pub fn mode(&self) -> IntellipanMode {
        self.mode
    }

    /// The pad currently selected by [`Self::mode`]'s own `x`/`y`, for
    /// mirroring into a UI reconciliation snapshot.
    pub fn position(&self) -> (f32, f32) {
        match self.mode {
            IntellipanMode::Color => (self.color.x, self.color.y),
            IntellipanMode::Position => (self.position.x, self.position.y),
            IntellipanMode::Modulation => (self.modulation.x, self.modulation.y),
        }
    }

    /// Switches the active pad. Resets whichever pad becomes newly active
    /// to its just-constructed state (module doc, spec 1.18's "cycle"
    /// gesture implies re-entering a mode starts clean, matching the old
    /// enum-of-boxes design's actual behaviour of reconstructing it) --
    /// never the pad being switched *away* from, which keeps running
    /// silently in the background exactly as before (it simply isn't
    /// `process`ed while inactive).
    pub fn set_mode(&mut self, mode: IntellipanMode) {
        self.mode = mode;
        match mode {
            IntellipanMode::Color => self.color.reset(),
            IntellipanMode::Position => self.position.reset(),
            IntellipanMode::Modulation => self.modulation.reset(),
        }
    }

    pub fn set_position(&mut self, x: f32, y: f32) {
        match self.mode {
            IntellipanMode::Color => self.color.set_position(x, y),
            IntellipanMode::Position => self.position.set_position(x, y),
            IntellipanMode::Modulation => self.modulation.set_position(x, y),
        }
    }

    pub fn process(&mut self, frame: &mut Frame) {
        match self.mode {
            IntellipanMode::Color => self.color.process(frame),
            IntellipanMode::Position => self.position.process(frame),
            IntellipanMode::Modulation => self.modulation.process(frame),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CHANNELS;

    const SR: f32 = 48_000.0;

    #[test]
    fn null_test_color_at_center_x_is_bit_exact_passthrough() {
        let mut pad = ColorPad::new(SR);
        for n in 0..1000 {
            let orig: Frame = std::array::from_fn(|c| ((n * (c as i32 + 1)) as f32 * 0.017).sin());
            let mut frame = orig;
            pad.process(&mut frame);
            assert_eq!(frame, orig);
        }
    }

    #[test]
    fn null_test_position_at_origin_is_bit_exact_passthrough() {
        let mut pad = PositionPad::new(SR);
        for n in 0..1000 {
            let mut frame: Frame = [0.0; CHANNELS];
            frame[0] = (n as f32 * 0.013).sin();
            frame[1] = (n as f32 * 0.027).cos();
            let orig = frame;
            pad.process(&mut frame);
            assert_eq!(frame, orig);
        }
    }

    #[test]
    fn null_test_modulation_at_y_zero_is_bit_exact_passthrough() {
        let mut pad = ModulationPad::new(SR);
        pad.set_position(-0.5, 0.0); // feedback engaged, but y=0 must still bypass
        for n in 0..1000 {
            let orig_l = (n as f32 * 0.013).sin();
            let orig_r = (n as f32 * 0.027).cos();
            let mut frame: Frame = [0.0; CHANNELS];
            frame[0] = orig_l;
            frame[1] = orig_r;
            pad.process(&mut frame);
            assert_eq!(frame[0], orig_l);
            assert_eq!(frame[1], orig_r);
        }
    }

    #[test]
    fn known_answer_color_boosts_treble_and_cuts_bass_at_positive_x() {
        let mut pad = ColorPad::new(SR);
        pad.set_position(0.5, 0.0);
        let mut frame: Frame = [0.0; CHANNELS];
        frame[0] = 1.0;
        frame[1] = 1.0;
        pad.process(&mut frame);
        // A DC-ish impulse through a boosted-treble/cut-bass tilt: just
        // assert the filters are actually engaged (output differs).
        assert_ne!(frame[0], 1.0);
    }

    #[test]
    fn known_answer_position_hard_left_silences_the_right_channel_gain() {
        let mut pad = PositionPad::new(SR);
        pad.set_position(-0.5, 0.0);
        for _ in 0..10 {
            let mut frame: Frame = [0.0; CHANNELS];
            frame[0] = 1.0;
            frame[1] = 1.0;
            pad.process(&mut frame);
            assert_eq!(frame[1], 0.0);
        }
    }

    #[test]
    fn mode_switching_does_not_leak_state_between_modes() {
        let mut pad = IntellipanPads::new(SR);
        pad.set_mode(IntellipanMode::Modulation);
        pad.set_position(-0.5, 1.0); // feedback + depth engaged
        let mut driven: Frame = [0.0; CHANNELS];
        driven[0] = 1.0;
        driven[1] = 1.0;
        for _ in 0..500 {
            pad.process(&mut driven);
        }

        // Switching to Color must behave exactly like a never-touched
        // Color pad: `set_mode` resets whichever pad becomes newly active
        // (module doc), so Modulation's delay-line state can't leak into
        // it even though, unlike the old enum-of-boxes design, the same
        // `ColorPad` allocation is reused rather than rebuilt.
        pad.set_mode(IntellipanMode::Color);
        let mut fresh = IntellipanPads::new(SR);
        let mut probe_a: Frame = [0.0; CHANNELS];
        probe_a[0] = 0.3;
        probe_a[1] = -0.4;
        let mut probe_b = probe_a;
        pad.process(&mut probe_a);
        fresh.process(&mut probe_b);
        assert_eq!(probe_a, probe_b);
    }

    /// The same proof as above, in the other direction: switching *back*
    /// to Modulation after driving it hard must also read as fresh, not
    /// resume wherever its buffer/phase were left. The old enum-of-boxes
    /// design got this for free (every switch rebuilt the pad); the new
    /// permanent-allocation design only gets it if `set_mode` actually
    /// resets the pad it switches *into*, which is the one behaviour this
    /// test exists to pin down that the leak test above doesn't (that one
    /// never switches back).
    #[test]
    fn switching_back_into_a_previously_driven_mode_is_also_reset() {
        let mut pad = IntellipanPads::new(SR);
        pad.set_mode(IntellipanMode::Modulation);
        pad.set_position(-0.5, 1.0);
        let mut driven: Frame = [1.0; CHANNELS];
        for _ in 0..500 {
            pad.process(&mut driven);
        }
        pad.set_mode(IntellipanMode::Color); // away...
        pad.set_mode(IntellipanMode::Modulation); // ...and back

        let mut fresh = IntellipanPads::new(SR);
        fresh.set_mode(IntellipanMode::Modulation);
        pad.set_position(-0.5, 1.0);
        fresh.set_position(-0.5, 1.0);

        let mut probe_a: Frame = [0.0; CHANNELS];
        probe_a[0] = 0.3;
        probe_a[1] = -0.4;
        let mut probe_b = probe_a;
        pad.process(&mut probe_a);
        fresh.process(&mut probe_b);
        assert_eq!(
            probe_a, probe_b,
            "re-entering Modulation should read as fresh, not resume its old buffer/phase"
        );
    }

    /// Spec 1.18's "right click cycles pad mode" gesture is now a live UI
    /// action (M10), reachable from the audio thread's own command drain
    /// (`loomix-app::control::EngineCommand::apply`) -- proves the module
    /// doc's actual RT-safety claim rather than just asserting it: mode
    /// switching and repositioning must never allocate, which the old
    /// enum-of-boxes design (reconstructing a fresh `Box` per switch)
    /// would have failed outright.
    #[test]
    fn switching_mode_and_repositioning_does_not_allocate() {
        use crate::rt_assert::assert_realtime;

        let mut pad = IntellipanPads::new(SR);
        assert_realtime(|| {
            pad.set_mode(IntellipanMode::Modulation);
            pad.set_position(-0.3, 0.7);
            pad.set_mode(IntellipanMode::Position);
            pad.set_position(0.2, 0.4);
            pad.set_mode(IntellipanMode::Color);
            pad.set_position(0.1, 0.0);
            let mut frame: Frame = [0.1; CHANNELS];
            pad.process(&mut frame);
        });
    }

    #[test]
    fn stability_random_automation_never_produces_nan_or_infinity() {
        let mut color = ColorPad::new(SR);
        let mut position = PositionPad::new(SR);
        let mut modulation = ModulationPad::new(SR);
        let mut seed = 17u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..10_000 {
            let x = rand() - 0.5;
            let y = rand();
            color.set_position(x, y);
            position.set_position(x, y);
            modulation.set_position(x, y);
            let mut frame: Frame = std::array::from_fn(|_| rand() * 2.0 - 1.0);
            color.process(&mut frame);
            position.process(&mut frame);
            modulation.process(&mut frame);
            assert!(frame.iter().all(|s| s.is_finite()));
        }
    }
}
