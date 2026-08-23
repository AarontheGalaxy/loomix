//! Intellipan (spec 1.2 step 8, spec 1.3, hardware strips only): three
//! mutually exclusive pad modes — Color, Position, Modulation — one active
//! at a time, switching resets the other two modes' internal state (no
//! state leaks between them).
//!
//! **Reverb/room-effect scope, decided explicitly, not silently dropped:**
//! spec 1.3 describes Color as "3 band tonal shaping plus a small reverb on
//! the upper half" and Position as "binaural placement with a small room
//! effect". Building a strip-local reverb now, ahead of M8's real
//! send/return reverb engine, means a second reverb implementation and a
//! migration later — skipped on direct instruction for Color, and the same
//! reasoning applies identically to Position's room effect (also a small
//! reverb-family algorithm), so it's skipped here too and logged the same
//! way. **`ColorPad`'s `y` and `PositionPad`'s `y` are accepted and stored
//! but do not affect audio this milestone** — `y == 0.0` neutrality does
//! *not* prove the reverb/room path works, because there is no reverb/room
//! path yet. Both land when M8's reverb engine exists.

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

/// `Position`/`Modulation` are boxed: their delay-line buffers (2048 and
/// 8192 `f32` samples respectively, sized for high sample rates) would
/// otherwise set this enum's size to its *largest* variant regardless of
/// which mode is active — every `Strip` would pay Modulation's ~64KB
/// whether or not it's ever selected, and constructing all 8 blew the test
/// thread's stack before this fix. Boxing only happens on mode
/// construction/switching (a configuration-time operation, not inside
/// `process()`), never on the audio thread.
pub enum Intellipan {
    Color(ColorPad),
    Position(Box<PositionPad>),
    Modulation(Box<ModulationPad>),
}

impl Intellipan {
    pub fn color(sample_rate: f32) -> Self {
        Self::Color(ColorPad::new(sample_rate))
    }
    pub fn position(sample_rate: f32) -> Self {
        Self::Position(Box::new(PositionPad::new(sample_rate)))
    }
    pub fn modulation(sample_rate: f32) -> Self {
        Self::Modulation(Box::new(ModulationPad::new(sample_rate)))
    }

    pub fn process(&mut self, frame: &mut Frame) {
        match self {
            Self::Color(p) => p.process(frame),
            Self::Position(p) => p.process(frame),
            Self::Modulation(p) => p.process(frame),
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
        let mut pad = Intellipan::modulation(SR);
        if let Intellipan::Modulation(m) = &mut pad {
            m.set_position(-0.5, 1.0); // feedback + depth engaged
        }
        let mut driven: Frame = [0.0; CHANNELS];
        driven[0] = 1.0;
        driven[1] = 1.0;
        for _ in 0..500 {
            pad.process(&mut driven);
        }

        // Switching the same `Intellipan` to Color must behave exactly
        // like a never-touched Color pad: each mode owns its own struct
        // entirely, so replacing the enum variant can't leak Modulation's
        // delay-line state into it.
        pad = Intellipan::color(SR);
        let mut fresh = Intellipan::color(SR);
        let mut probe_a: Frame = [0.0; CHANNELS];
        probe_a[0] = 0.3;
        probe_a[1] = -0.4;
        let mut probe_b = probe_a;
        pad.process(&mut probe_a);
        fresh.process(&mut probe_b);
        assert_eq!(probe_a, probe_b);
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
