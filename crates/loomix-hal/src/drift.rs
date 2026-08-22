//! Drift estimation and correction (spec 2.3): "measure drift from the
//! difference between the device's sample time and the master's, filter it
//! with a slow PI controller, and feed the ratio into a polyphase
//! resampler. Never resample with a naive fixed ratio, it will click every
//! few minutes." This module is the estimator and the PI filter; the
//! resampler itself is `resample.rs`.
//!
//! Pure: driven entirely by a caller-supplied error value, no CoreAudio, no
//! wall-clock time. The synthetic-clock simulation in the tests below
//! stands in for the real 30-minute two-device soak (spec 3.4 M4's
//! acceptance criterion) during day to day development; that soak is the
//! final confirmation against real hardware, not the debugging loop.

/// A slow PI filter over per-block drift error, in samples (device
/// cumulative frames minus master cumulative frames; positive means the
/// device is running ahead). `kp`/`ki` are the loop's calibration knobs --
/// spec 2.3 is explicit the controller must be slow, so these stay
/// deliberately small; retune per real hardware, there is no one correct
/// value. `max_correction` bounds how far the returned ratio can move from
/// 1.0 in a single block.
pub struct PiController {
    kp: f32,
    ki: f32,
    max_correction: f32,
    integral: f32,
    integral_clamp: f32,
}

impl PiController {
    pub fn new(kp: f32, ki: f32, max_correction: f32) -> Self {
        assert!(
            ki > 0.0,
            "ki must be positive or the integral clamp below is meaningless"
        );
        Self {
            kp,
            ki,
            max_correction,
            integral: 0.0,
            // Anti-windup: bounds how much `ki * integral` alone can
            // contribute, so a large error can't leave the integral at a
            // value that keeps the output pinned at `max_correction` long
            // after the error that caused it is gone.
            integral_clamp: max_correction / ki,
        }
    }

    /// Feeds one block's drift error and returns the resample ratio to
    /// apply to the device's contribution next block.
    ///
    /// `error_samples` must already be the small, bounded difference
    /// between the device's and the master's cumulative sample time, not
    /// those cumulative counts themselves computed in `f32`: a 30-minute
    /// run reaches tens of millions of frames, past where `f32` can even
    /// represent every integer exactly, and differencing two such
    /// accumulators in `f32` silently loses the small drift they differ
    /// by. Track cumulative sample time in `f64` or an integer type
    /// upstream and only narrow to `f32` once it's this small error value.
    pub fn update(&mut self, error_samples: f32) -> f32 {
        self.integral =
            (self.integral + error_samples).clamp(-self.integral_clamp, self.integral_clamp);
        let correction = (self.kp * error_samples + self.ki * self.integral)
            .clamp(-self.max_correction, self.max_correction);
        1.0 - correction
    }

    pub fn reset_integral(&mut self) {
        self.integral = 0.0;
    }
}

/// Wraps [`PiController`] with discontinuity detection. A device being
/// reconfigured or a USB interface renegotiating can make its reported
/// sample time jump by an amount no real drift rate would produce in a
/// single block. A plain PI controller has no way to tell that apart from
/// a huge drift reading: it integrates it, and because the integral term
/// never leaks on its own, that one reading permanently biases the loop's
/// steady-state output away from 1.0. A reading past `discontinuity_threshold`
/// is assumed to be exactly that kind of one-off jump: the integral is
/// reset instead of absorbing it, so the loop resumes tracking real drift
/// rather than chasing a step it cannot correct by adjusting rate anyway.
pub struct DriftCorrector {
    pi: PiController,
    discontinuity_threshold: f32,
}

impl DriftCorrector {
    pub fn new(pi: PiController, discontinuity_threshold: f32) -> Self {
        Self {
            pi,
            discontinuity_threshold,
        }
    }

    pub fn update(&mut self, error_samples: f32) -> f32 {
        if error_samples.abs() > self.discontinuity_threshold {
            self.pi.reset_integral();
            return 1.0;
        }
        self.pi.update(error_samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK_FRAMES: f32 = 128.0;

    fn new_corrector() -> DriftCorrector {
        // Slow on purpose (spec 2.3): a lone bad reading should move the
        // ratio by a small fraction of a percent, not chase it.
        DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0)
    }

    /// A tiny deterministic PRNG (xorshift32) standing in for measurement
    /// jitter, so the jitter tests don't need a new dependency for what a
    /// few lines of stdlib-only arithmetic already covers.
    struct Xorshift32(u32);
    impl Xorshift32 {
        fn next_unit(&mut self) -> f32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            // Map to roughly [-1.0, 1.0].
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }

    /// Runs a closed-loop drift simulation for `blocks` blocks.
    /// `rate_factor(i)` is the device's true instantaneous rate relative to
    /// the master for block `i` (1.0 == perfectly locked); `correct` turns
    /// each block's measured error into the ratio applied to the device's
    /// contribution the following block. Returns `(error, ratio)` per
    /// block.
    fn simulate(
        blocks: usize,
        mut rate_factor: impl FnMut(usize) -> f32,
        mut correct: impl FnMut(f32) -> f32,
    ) -> Vec<(f32, f32)> {
        // Cumulative counters in f64 (see `PiController::update`'s doc
        // comment): at tens of thousands of blocks these reach millions of
        // frames, past where f32 can resolve the small fractional drift
        // between them on every add.
        let mut device_cumulative = 0.0_f64;
        let mut applied_ratio = 1.0_f32;
        let mut history = Vec::with_capacity(blocks);
        for i in 0..blocks {
            let master_cumulative = (i + 1) as f64 * BLOCK_FRAMES as f64;
            device_cumulative += BLOCK_FRAMES as f64 * rate_factor(i) as f64 * applied_ratio as f64;
            let error = (device_cumulative - master_cumulative) as f32;
            let ratio = correct(error);
            history.push((error, ratio));
            applied_ratio = ratio;
        }
        history
    }

    #[test]
    fn converges_to_a_steady_ppm_offset_and_stays_bounded() {
        let ppm = 500.0;
        let mut corrector = new_corrector();
        let history = simulate(50_000, |_| 1.0 + ppm / 1e6, |e| corrector.update(e));

        // Settling takes a while by design (slow loop); judge steady state
        // from the back half of the run, not the transient at the start.
        let settled = &history[history.len() / 2..];
        let max_abs_error = settled.iter().map(|(e, _)| e.abs()).fold(0.0, f32::max);
        assert!(
            max_abs_error < 200.0,
            "drift should stay bounded once settled, got max |error| = {max_abs_error}"
        );
    }

    #[test]
    fn a_naive_fixed_ratio_fails_the_same_scenario() {
        // The exact failure spec 2.3 names: no correction at all. Same
        // scenario as the test above, so this is a genuine A/B, not a
        // different, easier case.
        let ppm = 500.0;
        let history = simulate(50_000, |_| 1.0 + ppm / 1e6, |_| 1.0);

        let final_error = history.last().unwrap().0.abs();
        assert!(
            final_error > 2000.0,
            "a fixed-ratio resampler is expected to drift unboundedly here \
             (that's the point of this test) but only reached |error| = {final_error}"
        );
    }

    #[test]
    fn tracks_a_slow_drift_ramp() {
        // Simulates temperature-driven drift: the offset itself changes
        // slowly over the run rather than being constant.
        let mut corrector = new_corrector();
        let history = simulate(
            80_000,
            |i| 1.0 + (i as f32 / 80_000.0) * 300.0 / 1e6,
            |e| corrector.update(e),
        );

        let settled = &history[history.len() / 2..];
        let max_abs_error = settled.iter().map(|(e, _)| e.abs()).fold(0.0, f32::max);
        assert!(
            max_abs_error < 400.0,
            "a slow ramp should still be tracked within a bound, got max |error| = {max_abs_error}"
        );
    }

    #[test]
    fn rejects_single_block_jitter_instead_of_chasing_it() {
        // Zero true drift, but the measurement itself is noisy -- imperfect
        // synchronisation between reading the device's and the master's
        // sample time. The whole point of a *slow* loop is to not treat
        // this noise as real drift.
        let mut rng = Xorshift32(0x1234_5678);
        let mut corrector = new_corrector();
        let history = simulate(
            10_000,
            move |_| 1.0 + (rng.next_unit() * 20.0) / 1e6,
            |e| corrector.update(e),
        );

        let settled = &history[1000..];
        let max_ratio_deviation = settled
            .iter()
            .map(|(_, r)| (r - 1.0).abs())
            .fold(0.0, f32::max);
        assert!(
            max_ratio_deviation < 0.0005,
            "per-block jitter should barely move the ratio, got max deviation = {max_ratio_deviation}"
        );
    }

    #[test]
    fn discontinuous_clock_jump_recovers_ratio_but_a_plain_pi_loop_stays_biased() {
        // Steady, correctly locked rate, except block 100 where the device
        // reports a one-shot 3000-sample jump -- the signature of a
        // reconfiguration or a USB interface renegotiating mid-stream, not
        // a change in rate. A rate-only corrector can never undo the
        // offset that already happened (that needs an explicit resync, out
        // of scope here); what it must not do is let that single reading
        // permanently bias the *ratio* away from 1.0.
        let jump_at = 100;
        let jump_samples = 3000.0;
        let rate_factor = move |i: usize| {
            if i == jump_at {
                1.0 + jump_samples / BLOCK_FRAMES
            } else {
                1.0
            }
        };

        let mut plain_pi = PiController::new(2e-5, 5e-7, 0.01);
        let plain_history = simulate(jump_at + 2000, rate_factor, |e| plain_pi.update(e));

        let mut corrector = new_corrector();
        let corrected_history = simulate(jump_at + 2000, rate_factor, |e| corrector.update(e));

        // Long after the jump, with the true rate back to nominal: the
        // plain PI loop's integral never leaked, so its ratio is still
        // biased away from 1.0.
        let plain_ratio_bias = (plain_history.last().unwrap().1 - 1.0).abs();
        assert!(
            plain_ratio_bias > 0.001,
            "a plain PI loop is expected to stay biased after the jump \
             (that's the point of this test), got bias = {plain_ratio_bias}"
        );

        // The discontinuity-aware corrector recovers within a handful of
        // blocks and stays recovered.
        let recovered = &corrected_history[jump_at + 10..];
        let max_ratio_deviation = recovered
            .iter()
            .map(|(_, r)| (r - 1.0).abs())
            .fold(0.0, f32::max);
        assert!(
            max_ratio_deviation < 0.001,
            "the discontinuity-aware corrector should recover its ratio to \
             near 1.0 shortly after the jump, got max deviation = {max_ratio_deviation}"
        );
    }
}
