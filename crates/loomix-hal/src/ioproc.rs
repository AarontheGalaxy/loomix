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

/// One non-master device's drift-corrected I/O, one instance per device
/// (not per channel -- it owns one [`Resampler`] per channel internally,
/// since drift is a property of the device's clock, shared by every
/// channel it carries).
pub struct DriftCorrectedIoStage {
    resamplers: Vec<Resampler>,
    corrector: DriftCorrector,
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
    pub fn new(channel_count: usize, corrector: DriftCorrector) -> Self {
        Self {
            resamplers: (0..channel_count).map(|_| Resampler::new()).collect(),
            corrector,
            progress_frames: 0,
        }
    }

    /// The ratio to use for the callback about to run, from progress
    /// measured as of the *previous* callback (this callback's own
    /// contribution isn't known until after it resamples).
    fn ratio_for_next_callback(&mut self, master: &MasterClock) -> f32 {
        let error = self.progress_frames as f64 - master.frames() as f64;
        self.corrector.update(error as f32)
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
        let mut stage = DriftCorrectedIoStage::new(1, corrector);

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

    #[test]
    fn render_underrun_fills_silence_instead_of_blocking_or_stale_data() {
        let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
        let mut stage = DriftCorrectedIoStage::new(1, corrector);
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
}
