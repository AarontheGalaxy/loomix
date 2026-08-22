//! A windowed-sinc polyphase resampler (spec 2.3): "feed the ratio into a
//! polyphase resampler. Never resample with a naive fixed ratio, it will
//! click every few minutes." Consumes the ratio `drift::DriftCorrector`
//! produces, which stays close to 1.0 by construction (real device clocks
//! drift by tens to hundreds of parts per million, not percent) -- this is
//! not a general arbitrary-ratio sample-rate converter.
//!
//! Mono; run one instance per channel. Real-time safe: the filter bank is
//! built once at construction (spec 3.3's "buffers are pre-allocated at
//! engine start"), and `process` only ever indexes into pre-allocated
//! fixed-size state, no allocation.

/// Filter support, in input samples either side of the current position.
/// `ponytail: 32 taps / 256 phases is a modest quality target that's
/// enough to pass this module's frequency-accuracy and click tests; if a
/// real two-device soak shows audible artefacts, raise TAPS/NUM_PHASES
/// first before changing the design.`
const TAPS: usize = 32;
const NUM_PHASES: usize = 256;

pub struct Resampler {
    /// `kernel[phase]`: `NUM_PHASES` windowed-sinc fractional-delay filters,
    /// one prototype lowpass evaluated at each of `NUM_PHASES` fractional
    /// offsets. Built once here, never in the hot path.
    kernel: Vec<[f32; TAPS]>,
    /// The `TAPS` most recent input samples, oldest first.
    history: [f32; TAPS],
    filled: usize,
    /// Fractional position, in input samples, of the next output sample
    /// relative to `history[TAPS / 2 - 1]`. Always in `[0.0, 1.0)` once
    /// primed.
    read_offset: f64,
}

impl Default for Resampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Resampler {
    pub fn new() -> Self {
        let half = TAPS as f64 / 2.0;
        let kernel = (0..NUM_PHASES)
            .map(|p| {
                let d = p as f64 / NUM_PHASES as f64;
                let mut taps = [0.0_f32; TAPS];
                let mut sum = 0.0_f64;
                for (k, tap) in taps.iter_mut().enumerate() {
                    let off = k as f64 - (half - 1.0);
                    let x = off - d;
                    let sinc = if x.abs() < 1e-9 {
                        1.0
                    } else {
                        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                    };
                    let window = if x.abs() >= half {
                        0.0
                    } else {
                        0.5 + 0.5 * (std::f64::consts::PI * x / half).cos()
                    };
                    let value = sinc * window;
                    *tap = value as f32;
                    sum += value;
                }
                // Normalise to exact unity DC gain regardless of the
                // window/truncation approximation above.
                for tap in taps.iter_mut() {
                    *tap = (*tap as f64 / sum) as f32;
                }
                taps
            })
            .collect();
        Self {
            kernel,
            history: [0.0; TAPS],
            filled: 0,
            read_offset: 0.0,
        }
    }

    /// Consumes as much of `input` as needed and writes resampled samples
    /// into `output`, at approximately `ratio` output samples per input
    /// sample. Returns `(output_written, input_consumed)`; `output_written
    /// < output.len()` means `input` ran out first -- the caller resumes
    /// with the remaining output slice once more input arrives, state
    /// carries across calls.
    pub fn process(&mut self, ratio: f32, input: &[f32], output: &mut [f32]) -> (usize, usize) {
        debug_assert!(ratio > 0.0, "resample ratio must be positive");
        let step = 1.0_f64 / ratio as f64;
        let mut in_pos = 0;
        let mut out_pos = 0;

        while out_pos < output.len() {
            if self.filled < TAPS {
                let Some(&sample) = input.get(in_pos) else {
                    break;
                };
                self.push(sample);
                in_pos += 1;
                continue;
            }
            if self.read_offset >= 1.0 {
                let Some(&sample) = input.get(in_pos) else {
                    break;
                };
                self.push(sample);
                in_pos += 1;
                self.read_offset -= 1.0;
                continue;
            }
            output[out_pos] = self.interpolate(self.read_offset);
            self.read_offset += step;
            out_pos += 1;
        }
        (out_pos, in_pos)
    }

    fn push(&mut self, sample: f32) {
        self.history.copy_within(1.., 0);
        self.history[TAPS - 1] = sample;
        self.filled = (self.filled + 1).min(TAPS);
    }

    fn interpolate(&self, frac: f64) -> f32 {
        let phase = ((frac * NUM_PHASES as f64).round() as usize).min(NUM_PHASES - 1);
        self.history
            .iter()
            .zip(self.kernel[phase].iter())
            .map(|(h, c)| h * c)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomix_core::render::{goertzel_magnitude, sine_tone};
    use loomix_core::rt_assert::assert_realtime;
    use loomix_core::{Frame, CHANNELS};

    /// Wraps a mono `f32` buffer as single-channel `Frame`s so the
    /// existing `render` helpers (built for the 8-channel engine) can be
    /// reused instead of duplicating a Goertzel/tone implementation here.
    fn as_frames(mono: &[f32]) -> Vec<Frame> {
        mono.iter()
            .map(|&s| {
                let mut f = [0.0; CHANNELS];
                f[0] = s;
                f
            })
            .collect()
    }

    fn magnitude_at(mono: &[f32], freq_hz: f32, sample_rate: f32) -> f32 {
        goertzel_magnitude(&as_frames(mono), 0, freq_hz, sample_rate)
    }

    fn run_all_at_once(resampler: &mut Resampler, ratio: f32, input: &[f32]) -> Vec<f32> {
        let mut output = vec![0.0; (input.len() as f32 * ratio).ceil() as usize + TAPS];
        let (written, _consumed) = resampler.process(ratio, input, &mut output);
        output.truncate(written);
        output
    }

    #[test]
    fn ratio_one_passes_a_sub_nyquist_tone_at_near_unity_gain() {
        let sample_rate = 48_000.0;
        let input_frames = sine_tone(4096, sample_rate, 1_000.0, 0);
        let input: Vec<f32> = input_frames.iter().map(|f| f[0]).collect();

        let mut resampler = Resampler::new();
        let output = run_all_at_once(&mut resampler, 1.0, &input);

        let in_mag = magnitude_at(&input, 1_000.0, sample_rate);
        let out_mag = magnitude_at(&output, 1_000.0, sample_rate);
        let gain = out_mag / in_mag;
        assert!(
            (0.95..=1.05).contains(&gain),
            "a sub-Nyquist tone should pass near unity gain at ratio 1.0, got gain = {gain}"
        );
    }

    #[test]
    fn resamples_a_tone_to_the_predicted_output_frequency() {
        let sample_rate = 48_000.0;
        let ratio = 1.02_f32; // a device running fast, corrected as drift.rs would.
        let f_in = 1_000.0_f32;
        let input_frames = sine_tone(8192, sample_rate, f_in, 0);
        let input: Vec<f32> = input_frames.iter().map(|f| f[0]).collect();

        let mut resampler = Resampler::new();
        let output = run_all_at_once(&mut resampler, ratio, &input);

        let f_out_predicted = f_in / ratio;
        let at_predicted = magnitude_at(&output, f_out_predicted, sample_rate);
        let at_original = magnitude_at(&output, f_in, sample_rate);
        assert!(
            at_predicted > at_original * 5.0,
            "expected the resampled tone's energy at the predicted frequency \
             {f_out_predicted} Hz to dominate over the original {f_in} Hz, \
             got {at_predicted} vs {at_original}"
        );
    }

    #[test]
    fn no_large_jump_between_consecutive_output_samples_under_a_drifting_ratio() {
        let sample_rate = 48_000.0;
        let input_frames = sine_tone(20_000, sample_rate, 440.0, 0);
        let input: Vec<f32> = input_frames.iter().map(|f| f[0]).collect();

        let mut resampler = Resampler::new();
        let mut output = Vec::new();
        let mut in_pos = 0;
        let mut ratio = 1.0_f32;
        while in_pos < input.len() {
            let chunk_end = (in_pos + 64).min(input.len());
            let chunk = &input[in_pos..chunk_end];
            let mut out_chunk = [0.0_f32; 96];
            let (written, consumed) = resampler.process(ratio, chunk, &mut out_chunk);
            output.extend_from_slice(&out_chunk[..written]);
            in_pos += consumed;
            // A slow ratio walk, well within what drift correction would
            // ever produce (spec 2.3's PI controller output is clamped
            // tighter than this in practice).
            ratio += 0.0002;
        }

        let max_step = output
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0_f32, f32::max);
        // The input itself (a 440 Hz sine) never steps by more than a
        // couple of percent of full scale between consecutive samples;
        // a click would show up as a step far larger than that.
        assert!(
            max_step < 0.2,
            "a click-free resampler should not produce large sample-to-sample \
             jumps under a slowly drifting ratio, got max step = {max_step}"
        );
    }

    #[test]
    fn realtime_process_does_not_allocate() {
        let mut resampler = Resampler::new();
        // Prime the filter outside the guarded scope -- only the steady
        // state hot path needs to be allocation-free.
        let warm = vec![0.0_f32; TAPS * 2];
        let mut scratch = vec![0.0_f32; TAPS * 2];
        resampler.process(1.0, &warm, &mut scratch);

        let input = [0.1_f32; 128];
        let mut output = [0.0_f32; 128];
        assert_realtime(|| {
            resampler.process(1.0, &input, &mut output);
        });
    }

    #[cfg(test)]
    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn output_stays_finite_and_bounded_under_random_ratio_automation(
                ratios in prop::collection::vec(0.98f32..1.02, 1..50),
                samples in prop::collection::vec(-1.0f32..1.0, 1..200),
            ) {
                let mut resampler = Resampler::new();
                let mut output = vec![0.0_f32; samples.len() * 2 + TAPS];
                for &ratio in &ratios {
                    let (written, _) = resampler.process(ratio, &samples, &mut output);
                    for &sample in &output[..written] {
                        prop_assert!(sample.is_finite());
                        // Not a tight gain bound: with `input` in [-1, 1]
                        // adversarially chosen by proptest, a windowed-sinc
                        // kernel's passband ripple/overshoot can exceed
                        // unity gain on some taps; the property under test
                        // is boundedness (no blow-up), not exact gain.
                        prop_assert!(sample.abs() <= 4.0);
                    }
                }
            }
        }
    }
}
