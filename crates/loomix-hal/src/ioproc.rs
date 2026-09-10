//! The real-time audio I/O stage for one non-master device (spec 3.4 M4):
//! resample against the master clock (spec 2.3) and move frames across a
//! lock-free ring buffer (`rtrb`, named by spec 3.3) toward whoever
//! assembles the engine's strip inputs and bus outputs -- `loomix-app`'s
//! job per spec 3.2's crate layout, not this crate's; `loomix-hal` stops
//! at handing over correctly-resampled frames.
//!
//! This is the first place drift correction, the resampler and a ring
//! buffer all run together on a real-time thread, so it's proven under
//! `assert_realtime` from the start (see the tests below), and proven
//! against a synthetic, hardware-free fake device (`tests::FakeDevice`)
//! before a single unsafe CoreAudio call exists to drive it for real --
//! that real registration is `device.rs`'s job, calling into exactly the
//! methods exercised here.

use crate::drift::DriftCorrector;
use crate::master_clock::MasterClock;
use crate::resample::Resampler;
use rtrb::{Consumer, Producer};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// A live, lock-free view of a [`DriftCorrectedIoStage`]'s current
/// resample ratio -- safe to poll from any thread, e.g. a soak harness's
/// monitoring loop (spec 3.4 M4's 30-minute two-device acceptance test),
/// since it's just a read of what the real-time thread last wrote. `1.0`
/// means no correction currently applied; a value that stays near `1.0`
/// and bounded, rather than drifting toward the corrector's clamp, is
/// what "bounded drift" (spec 3.4 M4) looks like numerically.
#[derive(Clone)]
pub struct RatioHandle(Arc<AtomicU32>);

impl RatioHandle {
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// One non-master device's drift-corrected I/O, one instance per device
/// (not per channel -- it owns one [`Resampler`] per channel internally,
/// since drift is a property of the device's clock, shared by every
/// channel it carries).
pub struct DriftCorrectedIoStage {
    resamplers: Vec<Resampler>,
    corrector: DriftCorrector,
    /// The known, fixed ratio a genuine nominal-rate difference between
    /// this device and the master needs -- `master_rate / device_rate`
    /// for a capture stage, computed once at connect time from each
    /// device's own reported nominal rate, never adjusted by the control
    /// loop itself (`docs/ARCHITECTURE.md`'s 2026-09-10 entry). `1.0` for
    /// the ordinary same-nominal-rate case, which is the only case this
    /// field used to implicitly assume. `corrector`'s own output --
    /// always `1.0 ± max_correction` -- multiplies this rather than
    /// replacing it, so the PI loop keeps doing exactly the small-
    /// residual-drift job spec 2.3 built it for, now measured against the
    /// true baseline instead of an assumed one.
    base_ratio: f32,
    ratio_bits: Arc<AtomicU32>,
    /// Cumulative *master-equivalent* progress made so far -- for capture,
    /// frames written to the ring (what the resampler actually produced);
    /// for render, frames consumed from the ring (what the resampler
    /// actually pulled). Deliberately not the raw device frame count:
    /// a device's raw clock offset grows without bound for as long as it
    /// keeps running at a constant ppm error, so feeding that straight
    /// into the PI controller gives it an error signal that never
    /// converges no matter how well the correction works, and the
    /// integral saturates almost immediately (found by this exact
    /// scenario: `ratio` pinned at 1.0 - max_correction for nearly the
    /// whole run, a 0.7% mistune when the injected offset was 0.05%).
    /// This quantity, by contrast, is exactly what the correction affects
    /// and is supposed to converge to track the master.
    ///
    /// Never needs `f64`: every value summed into it is an exact integer
    /// frame count CoreAudio (or a resampler call) hands over, so there is
    /// no fractional accumulation to lose precision on -- the
    /// `f32`-cumulative-counter bug the synthetic drift simulation caught
    /// doesn't apply here by construction.
    progress_frames: u64,
}

impl DriftCorrectedIoStage {
    /// `base_ratio` must already be sane (finite, positive, within
    /// whatever bound the caller enforces) -- this constructor trusts it,
    /// the same "validate at the boundary" convention every other
    /// caller-supplied value in this codebase follows; `main.rs::
    /// connect_audio` is that boundary for a real device pairing.
    pub fn new(channel_count: usize, corrector: DriftCorrector, base_ratio: f32) -> Self {
        Self {
            resamplers: (0..channel_count).map(|_| Resampler::new()).collect(),
            corrector,
            base_ratio,
            ratio_bits: Arc::new(AtomicU32::new(base_ratio.to_bits())),
            progress_frames: 0,
        }
    }

    /// A cloneable, thread-safe handle to this stage's current resample
    /// ratio -- the *effective* ratio (`base_ratio` times the corrector's
    /// own small correction), the number the resampler is actually being
    /// driven at, not just the residual correction on its own. Not
    /// real-time code itself -- the handle is meant to be cloned once, up
    /// front, and polled from a monitoring thread, not from inside
    /// another IOProc callback.
    pub fn ratio_handle(&self) -> RatioHandle {
        RatioHandle(self.ratio_bits.clone())
    }

    /// The ratio to use for the callback about to run, from progress
    /// measured as of the *previous* callback (this callback's own
    /// contribution isn't known until after it resamples). `corrector`
    /// still only ever returns `1.0 ± max_correction` -- exactly what it
    /// always returned, tracking only the small residual drift spec 2.3
    /// built it for -- multiplied by `base_ratio` here rather than used
    /// directly, so a genuine nominal-rate difference is already
    /// accounted for before the PI loop ever sees an error signal
    /// (`docs/ARCHITECTURE.md`'s 2026-09-10 entry).
    fn ratio_for_next_callback(&mut self, master: &MasterClock) -> f32 {
        let error = self.progress_frames as f64 - master.frames() as f64;
        let correction = self.corrector.update(error as f32);
        let ratio = self.base_ratio * correction;
        self.ratio_bits.store(ratio.to_bits(), Ordering::Relaxed);
        ratio
    }

    /// Called once per capture callback. `input` is planar, one slice per
    /// channel, all the same length -- what this callback just received
    /// from the device. `outputs` is one ring-buffer producer per channel;
    /// `scratch` is reused across channels, pre-allocated by the caller to
    /// at least `input`'s length (resampling near 1.0 never produces
    /// dramatically more output than input). A full ring drops the
    /// newest samples rather than blocking: spec 3.3 forbids blocking in
    /// the audio callback, and a full ring means the consumer side is
    /// already behind, which dropping doesn't make worse.
    pub fn on_capture(
        &mut self,
        input: &[&[f32]],
        master: &MasterClock,
        outputs: &mut [Producer<f32>],
        scratch: &mut [f32],
    ) {
        debug_assert_eq!(input.len(), self.resamplers.len());
        debug_assert_eq!(outputs.len(), self.resamplers.len());
        let ratio = self.ratio_for_next_callback(master);
        let mut produced = 0usize;
        for ((resampler, channel_in), producer) in self
            .resamplers
            .iter_mut()
            .zip(input.iter())
            .zip(outputs.iter_mut())
        {
            let (written, _consumed) = resampler.process(ratio, channel_in, scratch);
            produced = written; // every channel resamples the same ratio/length in lockstep
            for &sample in &scratch[..written] {
                let _ = producer.push(sample);
            }
        }
        self.progress_frames += produced as u64;
    }

    /// Called once per render callback. `inputs` is one ring-buffer
    /// consumer per channel (filled by whoever assembles bus output);
    /// `output` is planar, one slice per channel, at the length this
    /// callback must fill. An empty ring (the producer side hasn't caught
    /// up) fills the remainder of that channel with silence rather than
    /// stale data or blocking.
    pub fn on_render(
        &mut self,
        master: &MasterClock,
        inputs: &mut [Consumer<f32>],
        output: &mut [&mut [f32]],
        scratch: &mut [f32],
    ) {
        debug_assert_eq!(inputs.len(), self.resamplers.len());
        debug_assert_eq!(output.len(), self.resamplers.len());
        let ratio = self.ratio_for_next_callback(master);
        let mut consumed_total = 0usize;
        for ((resampler, consumer), out_channel) in self
            .resamplers
            .iter_mut()
            .zip(inputs.iter_mut())
            .zip(output.iter_mut())
        {
            let available = consumer.slots().min(scratch.len());
            for slot in scratch[..available].iter_mut() {
                *slot = consumer.pop().unwrap_or(0.0);
            }
            let (written, consumed) = resampler.process(ratio, &scratch[..available], out_channel);
            consumed_total = consumed; // same reasoning as on_capture's `produced`
            for sample in out_channel[written..].iter_mut() {
                *sample = 0.0;
            }
        }
        self.progress_frames += consumed_total as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drift::PiController;
    use loomix_core::render::{goertzel_magnitude, sine_tone};
    use loomix_core::rt_assert::assert_realtime;
    use loomix_core::{Frame, CHANNELS};

    fn magnitude_at(mono: &[f32], freq_hz: f32, sample_rate: f32) -> f32 {
        let frames: Vec<Frame> = mono
            .iter()
            .map(|&s| {
                let mut f = [0.0; CHANNELS];
                f[0] = s;
                f
            })
            .collect();
        goertzel_magnitude(&frames, 0, freq_hz, sample_rate)
    }

    /// Drives a [`DriftCorrectedIoStage`] on a synthetic clock -- no real
    /// thread, no real time, no CoreAudio. `ppm_offset` is this fake
    /// device's constant clock error relative to the master, the same
    /// scenario shape `drift.rs`'s tests already use, now carried all the
    /// way through resampling and a real ring buffer instead of stopping
    /// at the controller's output ratio.
    struct FakeDevice {
        block_frames: usize,
        ppm_offset: f64,
    }

    impl FakeDevice {
        /// Runs `num_callbacks` capture callbacks over `input` (which must
        /// be long enough to cover them at up to `ppm_offset`'s rate --
        /// the caller's job, not this harness's, so a short buffer fails
        /// loudly via the slice index rather than silently truncating the
        /// run). A device running `ppm_offset` fast or slow delivers
        /// slightly more or fewer frames per callback than the master
        /// advanced by -- exactly what a real drifting clock does.
        ///
        /// `master` is advanced *before* each callback, not after: in the
        /// real system the master's own IOProc runs concurrently on its
        /// own thread with no fixed ordering against this one, so what
        /// this callback reads is whatever the master last published --
        /// this harness picks the ordering that makes that read reflect
        /// the current callback's expected position, rather than
        /// (a test-harness-only artifact of strict sequential execution)
        /// always lagging it by exactly one block.
        fn run_capture(
            &self,
            stage: &mut DriftCorrectedIoStage,
            master: &MasterClock,
            input: &[f32],
            outputs: &mut [Producer<f32>],
            num_callbacks: usize,
        ) {
            let mut scratch = vec![0.0_f32; self.block_frames * 2];
            let mut pos = 0usize;
            let mut carried_frames = 0.0_f64;
            for _ in 0..num_callbacks {
                carried_frames += self.block_frames as f64 * (1.0 + self.ppm_offset / 1e6);
                let this_callback = (carried_frames as usize).max(1);
                carried_frames -= this_callback as f64;
                let end = pos + this_callback;
                let channel = [&input[pos..end]];
                master.advance(self.block_frames as u32);
                assert_realtime(|| {
                    stage.on_capture(&channel, master, outputs, &mut scratch);
                });
                pos = end;
            }
        }
    }

    #[test]
    fn a_drifting_fake_device_reconstructs_the_tone_within_bounded_drift() {
        let sample_rate = 48_000.0;
        let block_frames = 128;
        let num_callbacks = 2000;
        // Comfortably covers num_callbacks blocks even at the fastest ppm
        // offset used below, with margin.
        let input_frames = sine_tone(block_frames * num_callbacks * 2, sample_rate, 1_000.0, 0);
        let input: Vec<f32> = input_frames.iter().map(|f| f[0]).collect();

        let (mut producer, mut consumer) = rtrb::RingBuffer::<f32>::new(input.len());
        let master = MasterClock::default();
        let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
        let mut stage = DriftCorrectedIoStage::new(1, corrector, 1.0);

        // A device running 500 ppm fast -- a real, if generous, clock
        // error (spec 2.3's whole reason to exist).
        let device = FakeDevice {
            block_frames,
            ppm_offset: 500.0,
        };
        device.run_capture(
            &mut stage,
            &master,
            &input,
            std::slice::from_mut(&mut producer),
            num_callbacks,
        );

        let mut received = Vec::new();
        while let Ok(sample) = consumer.pop() {
            received.push(sample);
        }
        assert!(
            !received.is_empty(),
            "the drifting device should still deliver frames, just resampled"
        );

        // The frame-count check: this is what drift correction is actually
        // for. Matches the same bound `drift.rs`'s own proven-correct
        // scenario settles to at these kp/ki (same 500 ppm, same order of
        // magnitude of blocks) -- this test adds proof that bound survives
        // once the resampler and a real ring buffer are wired in, not just
        // the controller in isolation.
        let master_frames = master.frames() as usize;
        let frame_drift = received.len().abs_diff(master_frames);
        assert!(
            frame_drift < 300,
            "corrected capture should track the master's frame count \
             closely, got {frame_drift} frames of drift (master = \
             {master_frames}, received = {})",
            received.len()
        );

        // The tone survives recognisably: still dominant at ~1000 Hz
        // relative to a clearly different frequency, not "close to unity
        // absolute gain against a phase-clean reference" -- over a 5+
        // second single tone, a resample ratio that legitimately wobbles
        // by a few tenths of a percent block to block (bounded, matching
        // the frame-count check above) accumulates enough phase jitter to
        // fail a tight absolute-gain bound without anything actually being
        // wrong; `resample.rs`'s own non-unity-ratio test uses the same
        // "dominates" style of check for the same reason.
        let at_tone = magnitude_at(&received, 1_000.0, sample_rate);
        let at_distant = magnitude_at(&received, 4_000.0, sample_rate);
        assert!(
            at_tone > at_distant * 5.0,
            "the 1 kHz tone should still dominate a clearly different \
             frequency after drift-corrected capture, got {at_tone} vs {at_distant}"
        );
    }

    #[test]
    fn a_naive_uncorrected_capture_drifts_the_ring_out_of_sync() {
        // The A/B this whole module exists to avoid: a device delivering
        // frames straight into the ring with no resampling at all
        // accumulates exactly the raw frame-count surplus/deficit its ppm
        // offset produces, unbounded over the run -- the same failure
        // spec 2.3 names, now shown at the ring-buffer level rather than
        // only at the controller's ratio output (which `drift.rs` already
        // covers).
        let block_frames = 128usize;
        let num_callbacks = 2000usize;
        let ppm_offset = 500.0_f64;
        let input = vec![0.0_f32; block_frames * num_callbacks * 2];

        let (mut producer, mut consumer) = rtrb::RingBuffer::<f32>::new(input.len());
        let master = MasterClock::default();
        let mut carried = 0.0_f64;
        let mut pos = 0usize;
        for _ in 0..num_callbacks {
            carried += block_frames as f64 * (1.0 + ppm_offset / 1e6);
            let this_callback = (carried as usize).max(1);
            carried -= this_callback as f64;
            let end = pos + this_callback;
            master.advance(block_frames as u32);
            for &s in &input[pos..end] {
                let _ = producer.push(s);
            }
            pos = end;
        }

        let mut received_count = 0usize;
        while consumer.pop().is_ok() {
            received_count += 1;
        }
        let master_frames = master.frames() as usize;
        let drift = received_count.abs_diff(master_frames);
        assert!(
            drift > 20,
            "an uncorrected capture is expected to drift away from the \
             master's frame count over this run (that's the point of this \
             test), got drift = {drift} frames"
        );
    }

    /// Spec 2.3's PI controller is explicitly "slow" and, in
    /// `main.rs::connect_audio`'s actual constants (reused here
    /// verbatim), bounded to `max_correction = 0.01` with a
    /// `discontinuity_threshold` of 500 samples of *cumulative* error --
    /// tuned for the small, genuinely-drifting-clock scenario the test
    /// above covers (two devices at the *same* nominal rate, one running
    /// a few hundred ppm fast or slow). Nothing in `connect_audio` or
    /// `attach_capture_device` queries the capture device's own nominal
    /// sample rate (only the output device's, via `nominal_sample_rate`,
    /// feeds `engine.set_sample_rate`) or seeds the resampler's ratio
    /// with it -- every capture stage starts blind at ratio 1.0 and is
    /// only ever nudged a fraction of a percent per block from there.
    ///
    /// A *genuine* nominal-rate pairing most real setups will hit sooner
    /// or later -- a 44.1 kHz input device against a 48 kHz output, an
    /// 8.125% difference -- needs a steady-state ratio of about 0.919 to
    /// track the master, ~92x past `max_correction`'s reach. Worse: the
    /// cumulative error crosses `discontinuity_threshold` (500 samples)
    /// after only ~45 blocks at this mismatch, and every crossing is
    /// mistaken for the one-off device-reconfiguration jump the threshold
    /// exists to catch (`drift.rs`'s `DriftCorrector::update` doc
    /// comment): the integral resets and the ratio snaps back to exactly
    /// 1.0, over and over, so the loop never settles anywhere near the
    /// ratio it actually needs. This is a real, distinct gap from the
    /// interleaved-buffer bug fixed elsewhere in this milestone -- proven
    /// here, not assumed, by running the exact same harness and
    /// production constants as the passing 500 ppm test above and
    /// showing the frame-drift bound that test relies on does not hold.
    ///
    /// Kept exactly as originally written, `base_ratio` pinned to `1.0`
    /// rather than deleted, once `base_ratio` existed (M11): this is now
    /// the permanent regression test for *not* seeding it -- proof that
    /// leaving it at the old implicit default reproduces the exact bug
    /// `base_ratio` exists to fix, not just a claim about code that no
    /// longer exists.
    #[test]
    fn a_genuine_nominal_rate_mismatch_is_not_corrected_within_production_bounds() {
        let sample_rate = 48_000.0;
        let block_frames = 128;
        let num_callbacks = 2000;
        let input_frames = sine_tone(block_frames * num_callbacks * 2, sample_rate, 1_000.0, 0);
        let input: Vec<f32> = input_frames.iter().map(|f| f[0]).collect();

        let (mut producer, mut consumer) = rtrb::RingBuffer::<f32>::new(input.len());
        let master = MasterClock::default();
        // Same constants `main.rs::connect_audio` actually configures,
        // not a hypothetical worst case.
        let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
        let mut stage = DriftCorrectedIoStage::new(1, corrector, 1.0);
        let ratio_handle = stage.ratio_handle();

        // 44100 Hz capture against a 48000 Hz master: (44100 / 48000 - 1)
        // * 1e6 ppm -- a real device pairing, not a stress-test extreme.
        let device = FakeDevice {
            block_frames,
            ppm_offset: (44_100.0 / 48_000.0 - 1.0) * 1e6,
        };
        device.run_capture(
            &mut stage,
            &master,
            &input,
            std::slice::from_mut(&mut producer),
            num_callbacks,
        );

        let mut received = Vec::new();
        while let Ok(sample) = consumer.pop() {
            received.push(sample);
        }

        let master_frames = master.frames() as usize;
        let frame_drift = received.len().abs_diff(master_frames);
        assert!(
            frame_drift > 5_000,
            "expected a genuine nominal-rate mismatch to drift far past \
             the 500 ppm test's <300 frame bound (nothing here seeds or \
             widens the correction for it), got only {frame_drift} \
             frames of drift (master = {master_frames}, received = {})",
            received.len()
        );

        let final_ratio = ratio_handle.get();
        assert!(
            (final_ratio - 1.0).abs() < 0.02,
            "expected the discontinuity guard to keep snapping the ratio \
             back to ~1.0 rather than settling near the ~0.919 this \
             pairing actually needs, got {final_ratio}"
        );
    }

    /// The exact real-hardware scenario from `docs/ARCHITECTURE.md`'s
    /// 2026-09-10 entry, not a hypothetical: a Bluetooth headset's HFP
    /// capture profile (24000 Hz nominal, confirmed by direct CoreAudio
    /// query against real AirPods) against a real master output (44100
    /// Hz) -- an 84% mismatch, worse than the 44.1/48kHz test above, and
    /// exactly the shape `base_ratio` (M11) exists to fix. Live hardware
    /// measured this producing underruns at roughly 20,000/second once
    /// the discontinuity guard settles into permanently resetting; this
    /// test reproduces the identical shortfall mechanism
    /// `StripSource::pull_into` (`loomix-app::engine_io`) uses in
    /// production -- draining at the master's own rate and counting every
    /// sample the capture side hadn't produced yet -- entirely within
    /// this crate, so it needs no cross-crate access to prove the fix.
    #[test]
    fn airpods_class_mismatch_settles_to_a_bounded_underrun_count_once_base_ratio_seeds_it() {
        let master_sample_rate = 44_100.0_f32;
        let device_sample_rate = 24_000.0_f32;
        let block_frames = 128;
        // ~10 seconds of master time at 44100Hz/128-frame blocks -- long
        // enough that the discontinuity guard trips almost immediately at
        // this mismatch (the 44.1/48kHz test above already shows that
        // happening within ~45 blocks for a much smaller 8% mismatch) and
        // stays tripped for the whole run, the same as it did against
        // real hardware.
        let num_callbacks = 3445;
        let input_frames = sine_tone(
            block_frames * num_callbacks * 2,
            device_sample_rate,
            1_000.0,
            0,
        );
        let input: Vec<f32> = input_frames.iter().map(|f| f[0]).collect();

        let (mut producer, mut consumer) = rtrb::RingBuffer::<f32>::new(input.len());
        let master = MasterClock::default();
        let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
        // The fix under test: `main.rs::connect_audio`'s own formula,
        // `master_rate / device_rate`, computed once from each device's
        // real nominal rate the same way it would be for a genuine
        // AirPods connection.
        let base_ratio = master_sample_rate / device_sample_rate;
        let mut stage = DriftCorrectedIoStage::new(1, corrector, base_ratio);

        let device = FakeDevice {
            block_frames,
            ppm_offset: (device_sample_rate / master_sample_rate - 1.0) as f64 * 1e6,
        };
        device.run_capture(
            &mut stage,
            &master,
            &input,
            std::slice::from_mut(&mut producer),
            num_callbacks,
        );

        // Drain at the master's own rate and count every sample not yet
        // available -- exactly `StripSource::pull_into`'s own logic
        // (`loomix-app::engine_io`), reproduced here rather than imported
        // across the crate boundary, so this is a real underrun count
        // from a real drain, not a subtraction.
        let master_frames = master.frames() as usize;
        let mut underruns = 0u64;
        for _ in 0..master_frames {
            if consumer.pop().is_err() {
                underruns += 1;
            }
        }

        // Sanity check on the scenario itself, independent of whether the
        // fix works: with `base_ratio` inert (today's bug), the capture
        // ring only ever fills at the device's own raw rate while this
        // loop drains at the master's, so the shortfall should come out
        // within a few percent of `master_frames * (1 - device_rate /
        // master_rate)` -- for this run, ~200,987 frames, matching real
        // hardware's measured ~20,000/second over the ~10 seconds this
        // run simulates (`docs/ARCHITECTURE.md`) to within a few percent.
        // Recorded so the bound below reads as a real, derived quantity,
        // not an arbitrary threshold picked to make the test pass.
        let naive_shortfall_if_unfixed =
            master_frames as f32 * (1.0 - device_sample_rate / master_sample_rate);
        assert!(
            naive_shortfall_if_unfixed > 150_000.0,
            "sanity check on the scenario itself: an 84% mismatch over \
             ~10 simulated seconds should imply well over 150,000 frames \
             of shortfall if uncorrected, got {naive_shortfall_if_unfixed}"
        );

        assert!(
            underruns < 300,
            "a genuine AirPods-class rate mismatch (24000 Hz capture \
             against a 44100 Hz master) should settle to a small, bounded \
             underrun count once base_ratio seeds the resampler correctly \
             -- got {underruns} underruns over {master_frames} master \
             frames (naive unfixed shortfall would have been ~{naive_shortfall_if_unfixed})"
        );
    }

    #[test]
    fn render_underrun_fills_silence_instead_of_blocking_or_stale_data() {
        let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
        let mut stage = DriftCorrectedIoStage::new(1, corrector, 1.0);
        let master = MasterClock::default();
        let (_producer, consumer) = rtrb::RingBuffer::<f32>::new(16);
        // Nothing was ever pushed -- every callback underruns.
        let mut inputs = [consumer];
        let mut scratch = vec![0.0_f32; 256];
        let mut out_buf = [1.0_f32; 128]; // poisoned with a non-zero sentinel
        let mut output: [&mut [f32]; 1] = [&mut out_buf];

        assert_realtime(|| {
            stage.on_render(&master, &mut inputs, &mut output, &mut scratch);
        });

        assert!(
            out_buf.iter().all(|&s| s == 0.0),
            "an underrun should produce silence, not the poisoned sentinel \
             or leftover ring contents"
        );
    }

    #[test]
    fn ratio_handle_reflects_the_stage_that_produced_it() {
        let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
        let mut stage = DriftCorrectedIoStage::new(1, corrector, 1.0);
        let handle = stage.ratio_handle();
        assert_eq!(handle.get(), 1.0, "no callback has run yet");

        let master = MasterClock::default();
        master.advance(128);
        let mut scratch = vec![0.0_f32; 256];
        let input = vec![0.0_f32; 128];
        let (mut producer, _consumer) = rtrb::RingBuffer::<f32>::new(256);
        stage.on_capture(
            &[&input],
            &master,
            std::slice::from_mut(&mut producer),
            &mut scratch,
        );

        // Same value the stage itself would use next callback -- the
        // handle is a live view, not a snapshot taken at construction.
        assert!(
            (handle.get() - 1.0).abs() < 0.01,
            "one callback shouldn't move the ratio far from 1.0, got {}",
            handle.get()
        );
    }
}
