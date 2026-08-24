//! Per-channel peak metering (spec 1.3 input meter, 1.5 output meter): a
//! peak that holds for a fixed time and then decays, not a permanent
//! running maximum — see `docs/DSP.md` for the exact numbers and why a
//! plain running max was wrong. Updated inline during `process_block` —
//! no allocation, no lock, crossed to the UI thread the same lock-free
//! way parameters cross in (spec 3.3), landing as `loomix-app::control`'s
//! `MeterSnapshot` once M8 gave this a UI thread to cross to.

use crate::{Frame, CHANNELS};

/// spec 1.3/1.5 don't publish exact meter ballistics (no such table exists
/// in the reference manual this project verifies parameter *ranges*
/// against). These match the reference behaviour typical hardware and
/// software peak meters use — hold the peak briefly, then fall at a
/// roughly constant dB/s rate — rather than a plain running maximum,
/// which reads identically whether a channel is currently loud or was
/// loud once and has been silent (or muted) ever since; see
/// `docs/DSP.md`'s "Metering" section for the numbers and the reasoning.
const HOLD_TIME_S: f32 = 1.0;
const DECAY_DB_PER_S: f32 = 20.0;

/// Below this, a decaying peak is snapped to exact digital silence rather
/// than left asymptotically approaching zero forever — a multiplicative
/// decay never reaches exactly 0.0, and letting it linger in the
/// denormal range indefinitely is exactly the cost spec 3.3's "flush
/// denormals" rule exists to avoid.
const SILENCE_FLOOR: f32 = 1e-6;

#[derive(Debug, Clone, Copy)]
pub struct Meter {
    peak_hold: [f32; CHANNELS],
    /// Samples left before this channel's held peak starts decaying.
    /// Reset to `hold_samples` every time a new, higher peak arrives.
    hold_remaining: [u32; CHANNELS],
    hold_samples: u32,
    /// Per-sample multiplicative decay: `peak *= decay_per_sample` once
    /// per sample while holding has expired, chosen so that after
    /// `sample_rate` samples (one second) the cumulative drop is exactly
    /// `DECAY_DB_PER_S` dB — see `decay_factor_per_sample`'s doc comment.
    decay_per_sample: f32,
}

impl Meter {
    /// Sample-rate dependent (hold time and decay rate are both real-time
    /// quantities), so — same move as every other sample-rate-dependent
    /// block in this crate (`Strip::for_topology_index`, `Bus::new`) —
    /// this replaces a plain `Default`.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            peak_hold: [0.0; CHANNELS],
            hold_remaining: [0; CHANNELS],
            hold_samples: hold_samples(sample_rate),
            decay_per_sample: decay_factor_per_sample(sample_rate),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.hold_samples = hold_samples(sample_rate);
        self.decay_per_sample = decay_factor_per_sample(sample_rate);
    }

    pub fn peak(&self, channel: usize) -> f32 {
        self.peak_hold[channel]
    }

    /// Clears the peak hold back to digital silence immediately, bypassing
    /// the hold/decay ballistics — a deliberate instant reset (e.g. on
    /// transport stop), not part of the normal metering behaviour above.
    pub fn reset(&mut self) {
        self.peak_hold = [0.0; CHANNELS];
        self.hold_remaining = [0; CHANNELS];
    }

    pub(crate) fn observe(&mut self, block: &[Frame]) {
        for frame in block {
            let channels = frame
                .iter()
                .zip(self.peak_hold.iter_mut())
                .zip(self.hold_remaining.iter_mut());
            for ((&sample, held), remaining) in channels {
                let level = sample.abs();
                if level > *held {
                    *held = level;
                    *remaining = self.hold_samples;
                } else if *remaining > 0 {
                    *remaining -= 1;
                } else {
                    let decayed = *held * self.decay_per_sample;
                    *held = if decayed < SILENCE_FLOOR {
                        0.0
                    } else {
                        decayed
                    };
                }
            }
        }
    }
}

fn hold_samples(sample_rate: f32) -> u32 {
    (HOLD_TIME_S * sample_rate).round() as u32
}

/// `factor^sample_rate` must equal the linear ratio for a `DECAY_DB_PER_S`
/// drop over one second (`10^(-DECAY_DB_PER_S/20)`), so
/// `factor = 10^(-DECAY_DB_PER_S / (20 * sample_rate))`.
fn decay_factor_per_sample(sample_rate: f32) -> f32 {
    10f32.powf(-DECAY_DB_PER_S / 20.0 / sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn silence(len: usize) -> Vec<Frame> {
        vec![[0.0; CHANNELS]; len]
    }

    fn loud_frame(level: f32) -> Frame {
        let mut f = [0.0; CHANNELS];
        f[0] = level;
        f
    }

    #[test]
    fn tracks_the_highest_absolute_sample_seen_within_one_call() {
        let mut meter = Meter::new(SR);
        meter.observe(&[loud_frame(0.3), loud_frame(-0.8)]);
        assert_eq!(meter.peak(0), 0.8);
        assert_eq!(meter.peak(1), 0.0);
    }

    #[test]
    fn holds_the_peak_for_the_documented_hold_time_before_decaying() {
        let mut meter = Meter::new(SR);
        meter.observe(&[loud_frame(0.9)]);
        // One sample short of the hold time: still exactly 0.9, no decay
        // has started yet.
        let hold_samples_count = hold_samples(SR) as usize;
        meter.observe(&silence(hold_samples_count - 1));
        assert_eq!(
            meter.peak(0),
            0.9,
            "peak must not move at all before the hold time elapses"
        );
    }

    #[test]
    fn decays_at_the_documented_rate_after_the_hold_time_elapses() {
        let mut meter = Meter::new(SR);
        meter.observe(&[loud_frame(1.0)]);
        let hold_samples_count = hold_samples(SR) as usize;
        // Run out the hold time, then decay for exactly one more second.
        meter.observe(&silence(hold_samples_count));
        meter.observe(&silence(SR as usize));

        // 20dB/s for 1s is a 20dB drop: linear ratio 10^(-20/20) = 0.1.
        let expected = 1.0 * 10f32.powf(-DECAY_DB_PER_S / 20.0);
        let tolerance = expected * 0.01; // accumulated per-sample rounding
        assert!(
            (meter.peak(0) - expected).abs() < tolerance,
            "expected ~{expected} (a 20dB drop) after 1s of decay, got {}",
            meter.peak(0)
        );
    }

    #[test]
    fn a_muted_or_silenced_channel_eventually_reads_exact_silence() {
        // The bug this whole ballistics change fixes: a plain running max
        // never comes back down, so a muted channel with an old peak
        // looks identical to one that's still loud. Decay must actually
        // reach exact 0.0, not just get smaller.
        let mut meter = Meter::new(SR);
        meter.observe(&[loud_frame(1.0)]);
        // Comfortably longer than hold time plus enough decay time to
        // cross the silence floor (a 120dB range decays in 6s at
        // 20dB/s; give it 10s of margin).
        meter.observe(&silence(SR as usize * 10));
        assert_eq!(
            meter.peak(0),
            0.0,
            "a long-silent channel must read exact silence, not a stale peak"
        );
    }

    #[test]
    fn a_fresh_higher_peak_during_decay_resets_the_hold_window() {
        let mut meter = Meter::new(SR);
        meter.observe(&[loud_frame(0.5)]);
        let hold_samples_count = hold_samples(SR) as usize;
        // Decay partway, then feed a new, higher peak.
        meter.observe(&silence(hold_samples_count + SR as usize / 4));
        assert!(meter.peak(0) < 0.5, "should have started decaying by now");
        meter.observe(&[loud_frame(0.9)]);
        assert_eq!(meter.peak(0), 0.9);
        // The new peak's own hold window must apply, not the old one's
        // already-expired countdown -- one sample short of the fresh
        // hold time, still exactly 0.9.
        meter.observe(&silence(hold_samples_count - 1));
        assert_eq!(
            meter.peak(0),
            0.9,
            "a fresh higher peak must restart the hold window, not inherit an expired one"
        );
    }

    #[test]
    fn reset_clears_immediately_bypassing_hold_and_decay() {
        let mut meter = Meter::new(SR);
        meter.observe(&[loud_frame(0.9)]);
        meter.reset();
        assert_eq!(meter.peak(0), 0.0);
        // And the reset channel doesn't jump back to holding an old
        // value -- a fresh, quieter peak right after reset should be
        // exactly what it reports.
        meter.observe(&[loud_frame(0.2)]);
        assert_eq!(meter.peak(0), 0.2);
    }

    /// Order proof, per this project's convention (see
    /// `docs/ARCHITECTURE.md`): decay must only ever apply *after* the
    /// hold window has actually elapsed. Checked the same way other order
    /// claims in this codebase are -- by constructing a scenario where
    /// "decay ran during the hold window" and "decay didn't" produce
    /// different, distinguishable answers, then confirming the real
    /// implementation matches only the correct one.
    #[test]
    fn decay_never_runs_before_the_hold_window_elapses_provably() {
        let mut meter = Meter::new(SR);
        meter.observe(&[loud_frame(1.0)]);
        let hold_samples_count = hold_samples(SR) as usize;
        let halfway = hold_samples_count / 2;
        meter.observe(&silence(halfway));
        let actual = meter.peak(0);

        // Hypothesis A (correct): still fully within the hold window,
        // so the peak hasn't moved at all.
        let hypothesis_hold = 1.0f32;
        // Hypothesis B (wrong: decay ran from sample zero regardless of
        // the hold window): independently computed via the same
        // per-sample factor this file's own `decay_factor_per_sample`
        // uses, applied naively from the start.
        let hypothesis_decay_from_zero = 1.0 * decay_factor_per_sample(SR).powi(halfway as i32);

        assert!(
            (hypothesis_hold - hypothesis_decay_from_zero).abs() > 0.01,
            "hypotheses aren't distinguishable"
        );
        assert_eq!(
            actual, hypothesis_hold,
            "actual should match the hold-respected hypothesis, not decay-from-zero"
        );
    }
}
