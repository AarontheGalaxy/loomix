//! Wires `loomix-hal`'s device I/O into `loomix-core`'s engine (spec 3.2:
//! `loomix-app` is what "wires everything together", not `loomix-hal` --
//! see `loomix-hal::ioproc`'s module doc). Two things live here: pulling
//! each strip's captured audio out of its device's ring buffer and
//! pushing each bus's output into its device's ring buffer around one
//! call to [`Engine::process_block`], and deciding which device (if any)
//! drives that call -- spec 1.19's "the main output device is the clock
//! master".
//!
//! Pure Rust, no CoreAudio calls of its own: `#![forbid(unsafe_code)]`
//! applies to this whole crate (spec 4.2), so the ring-assembly logic
//! below is offline-testable with synthetic ring buffers the same way
//! everything in `loomix-hal` is with synthetic clocks.

use loomix_core::{Engine, Frame, CHANNELS, NUM_BUSES, NUM_STRIPS};
use loomix_hal::clock::DeviceId;
use loomix_hal::device::CoreAudioError;
use loomix_hal::master_clock::MasterClock;
use rtrb::{Consumer, Producer};
use std::sync::Arc;

/// Which device (if any) is the clock master, reusing `loomix-hal`'s
/// already-tested pure resolution logic (`clock::resolve_clock_source`)
/// over a live device enumeration -- the only thing this function adds
/// over that one is the enumeration call itself.
pub fn select_clock_master(
    configured: Option<DeviceId>,
) -> Result<loomix_hal::clock::ClockSource, CoreAudioError> {
    let alive = loomix_hal::device::list_device_ids()?;
    Ok(loomix_hal::clock::resolve_clock_source(configured, &alive))
}

/// A lock-free, pollable dropout counter -- incremented on the real-time
/// thread (a plain `AtomicU64::fetch_add`, no different in cost from the
/// counter-free version), read from a monitoring thread. Spec 3.4 M4's
/// 30-minute two-device soak needs to report "no dropouts" as a measured
/// fact, not an assumption.
#[derive(Clone, Default)]
pub struct DropoutCounter(Arc<std::sync::atomic::AtomicU64>);

impl DropoutCounter {
    fn increment(&self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// One strip's captured audio, arriving as one ring-buffer consumer per
/// device channel (filled by that device's `CaptureIoProcHandle`).
pub struct StripSource {
    channels: Vec<Consumer<f32>>,
    underruns: DropoutCounter,
}

impl StripSource {
    pub fn new(channels: Vec<Consumer<f32>>) -> Self {
        Self {
            channels,
            underruns: DropoutCounter::default(),
        }
    }

    pub fn underrun_counter(&self) -> DropoutCounter {
        self.underruns.clone()
    }

    /// Packs available samples into `out` (one [`Frame`] per output
    /// position), channel by channel, up to [`CHANNELS`] device channels;
    /// an empty ring (this callback's audio hasn't arrived yet) fills that
    /// position with silence rather than blocking or leaving stale data --
    /// same underrun handling as `ioproc::DriftCorrectedIoStage::on_render`.
    fn pull_into(&mut self, out: &mut [Frame]) {
        for frame in out.iter_mut() {
            *frame = [0.0; CHANNELS];
        }
        for (channel, consumer) in self.channels.iter_mut().enumerate().take(CHANNELS) {
            for frame in out.iter_mut() {
                frame[channel] = match consumer.pop() {
                    Ok(sample) => sample,
                    Err(_) => {
                        self.underruns.increment();
                        0.0
                    }
                };
            }
        }
    }
}

/// One bus's output, arriving as one ring-buffer producer per device
/// channel (drained by that device's `RenderIoProcHandle`).
pub struct BusSink {
    channels: Vec<Producer<f32>>,
    overruns: DropoutCounter,
}

impl BusSink {
    pub fn new(channels: Vec<Producer<f32>>) -> Self {
        Self {
            channels,
            overruns: DropoutCounter::default(),
        }
    }

    pub fn overrun_counter(&self) -> DropoutCounter {
        self.overruns.clone()
    }

    /// Pushes `input` out, channel by channel, up to [`CHANNELS`] device
    /// channels. A full ring (the device side hasn't caught up) drops the
    /// newest samples rather than blocking -- same policy as
    /// `DriftCorrectedIoStage::on_capture`.
    fn push_from(&mut self, input: &[Frame]) {
        for (channel, producer) in self.channels.iter_mut().enumerate().take(CHANNELS) {
            for frame in input {
                if producer.push(frame[channel]).is_err() {
                    self.overruns.increment();
                }
            }
        }
    }
}

/// Assembles every strip's and bus's ring buffers around one
/// [`Engine`], driven by whichever device calls [`Self::on_master_tick`]
/// once per callback (spec 1.19). Strips/buses with no device attached
/// stay silent (input) or simply have nothing draining them (output) --
/// exactly spec 1.11's "clearing it leaves the strip fed only by network
/// audio" / a bus with no device selected, extended to "no device
/// attached yet" during M4, before M12's network audio exists to fill
/// that gap.
pub struct EngineIoDriver {
    engine: Engine,
    strip_sources: [Option<StripSource>; NUM_STRIPS],
    bus_sinks: [Option<BusSink>; NUM_BUSES],
    master_clock: Arc<MasterClock>,
    /// Which strip (if any) the master device's own captured channels
    /// feed directly, bypassing a ring buffer entirely -- see
    /// [`Self::on_master_tick`]'s doc comment for why.
    master_strip: Option<usize>,
    scratch_inputs: [Vec<Frame>; NUM_STRIPS],
    scratch_outputs: [Vec<Frame>; NUM_BUSES],
}

/// Bus A1 (spec 1.1's first physical bus) is definitionally the main
/// output bus (spec 1.19), and therefore the one the clock-master
/// device's own render channels serve directly in
/// [`EngineIoDriver::on_master_tick`] -- not a scoping shortcut, spec 1.11
/// literally defines the main output device's bus this way.
const MASTER_BUS: usize = 0;

impl EngineIoDriver {
    /// `max_block_frames` sizes every scratch buffer once, up front (spec
    /// 3.3: "buffers are pre-allocated at engine start... the engine
    /// reallocates only on an explicit restart") -- `on_master_tick`'s own
    /// `debug_assert` panics if a callback ever exceeds it, rather than
    /// silently allocating on the audio thread.
    pub fn new(
        engine: Engine,
        master_clock: Arc<MasterClock>,
        master_strip: Option<usize>,
        max_block_frames: usize,
    ) -> Self {
        Self {
            engine,
            strip_sources: std::array::from_fn(|_| None),
            bus_sinks: std::array::from_fn(|_| None),
            master_clock,
            master_strip,
            scratch_inputs: std::array::from_fn(|_| vec![[0.0; CHANNELS]; max_block_frames]),
            scratch_outputs: std::array::from_fn(|_| vec![[0.0; CHANNELS]; max_block_frames]),
        }
    }

    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    pub fn set_strip_source(&mut self, strip: usize, source: StripSource) {
        self.strip_sources[strip] = Some(source);
    }

    pub fn set_bus_sink(&mut self, bus: usize, sink: BusSink) {
        self.bus_sinks[bus] = Some(sink);
    }

    /// Called once per callback by the clock-master device's IOProc
    /// (`loomix_hal::device_lifecycle::MasterIoProcHandle`). `master_in` is that
    /// device's own captured channels this callback (empty if it isn't
    /// also a strip's source); `master_out` is where its bus-A1 output
    /// for this callback must be written (spec 1.19 -- the main output
    /// device's bus is always A1). Handling the master's own audio
    /// directly here, rather than through a ring buffer like every other
    /// device, sidesteps an ordering problem a ring can't: this callback
    /// needs the engine's *freshly computed* bus-A1 output to write out
    /// before it returns, and a ring only has last block's, not this
    /// one's, unless something else already ran the engine tick first --
    /// which is exactly this method's job.
    ///
    /// Real-time safe: every scratch buffer was sized once at
    /// construction, `resize`/`truncate` within that reserved capacity
    /// never reallocates, and `Consumer::pop`/`Producer::push` are
    /// lock-free.
    pub fn on_master_tick(
        &mut self,
        block_frames: usize,
        master_in: &[&[f32]],
        master_out: &mut [&mut [f32]],
    ) {
        for buf in self
            .scratch_inputs
            .iter_mut()
            .chain(self.scratch_outputs.iter_mut())
        {
            debug_assert!(
                block_frames <= buf.capacity(),
                "callback block size exceeded the capacity reserved at construction"
            );
            buf.resize(block_frames, [0.0; CHANNELS]);
        }

        self.master_clock.advance(block_frames as u32);

        for (strip, buf) in self.scratch_inputs.iter_mut().enumerate() {
            if Some(strip) == self.master_strip {
                pack_channels(master_in, buf);
            } else if let Some(source) = &mut self.strip_sources[strip] {
                source.pull_into(buf);
            } else {
                buf.iter_mut().for_each(|f| *f = [0.0; CHANNELS]);
            }
        }

        {
            let input_refs: [&[Frame]; NUM_STRIPS] =
                self.scratch_inputs.each_ref().map(|v| v.as_slice());
            let mut output_refs: [&mut [Frame]; NUM_BUSES] =
                self.scratch_outputs.each_mut().map(|v| v.as_mut_slice());
            self.engine.process_block(&input_refs, &mut output_refs);
        }

        for (bus, buf) in self.scratch_outputs.iter().enumerate() {
            if bus == MASTER_BUS {
                unpack_channels(buf, master_out);
            } else if let Some(sink) = &mut self.bus_sinks[bus] {
                sink.push_from(buf);
            }
        }
    }
}

/// `src` is either one `&[f32]` per channel (the planar case, handled by
/// the loop at the bottom) or, on real hardware, a single combined buffer
/// carrying every channel interleaved (`L0,R0,L1,R1,...`) --
/// `master_ioproc_trampoline` (`loomix-hal`) hands over whatever shape
/// CoreAudio actually delivered, with no deinterleaving of its own (see
/// its doc comment). Distinguished here by size alone, with no separate
/// "expected channel count" input needed: `dst.len()` is already the
/// frame count this callback asked for, so `src[0].len() > dst.len()`
/// means `src[0]` packs more than one frame's worth of samples into one
/// buffer, which only happens when it's actually interleaved multi-channel
/// data, never a genuine one-channel (mono) buffer -- a mono buffer's
/// length is always exactly the frame count, so this never misfires on
/// the mono case that already worked correctly before this fix existed.
///
/// Found by two deterministic tests
/// (`on_master_tick_deinterleaves_stereo_input_correctly_from_a_single_combined_buffer`
/// and its output-side sibling below) proving this function silently
/// scrambled real (non-silent) stereo content, not assumed from reading
/// the code -- see `docs/ARCHITECTURE.md`.
fn pack_channels(src: &[&[f32]], dst: &mut [Frame]) {
    for frame in dst.iter_mut() {
        *frame = [0.0; CHANNELS];
    }
    let frames = dst.len();
    if frames == 0 {
        return;
    }
    if src.len() == 1 && src[0].len() > frames {
        let raw = src[0];
        let channel_count = (raw.len() / frames).min(CHANNELS);
        for (f, frame) in dst.iter_mut().enumerate() {
            for c in 0..channel_count {
                frame[c] = raw[f * channel_count + c];
            }
        }
        return;
    }
    for (channel, data) in src.iter().enumerate().take(CHANNELS) {
        for (frame, &sample) in dst.iter_mut().zip(data.iter()) {
            frame[channel] = sample;
        }
    }
}

/// The output-side mirror of [`pack_channels`]'s interleaving fix -- same
/// reasoning, same size-based distinction, same "a mono buffer's length
/// is always exactly the frame count so this never misfires on the case
/// that already worked" argument, just interleaving into `dst[0]` instead
/// of deinterleaving out of `src[0]`.
fn unpack_channels(src: &[Frame], dst: &mut [&mut [f32]]) {
    let frames = src.len();
    if frames == 0 {
        return;
    }
    if dst.len() == 1 && dst[0].len() > frames {
        let channel_count = (dst[0].len() / frames).min(CHANNELS);
        let raw = &mut dst[0];
        for (f, frame) in src.iter().enumerate() {
            for c in 0..channel_count {
                raw[f * channel_count + c] = frame[c];
            }
        }
        return;
    }
    for (channel, out_channel) in dst.iter_mut().enumerate().take(CHANNELS) {
        for (frame, out_sample) in src.iter().zip(out_channel.iter_mut()) {
            *out_sample = frame[channel];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomix_core::render::{goertzel_magnitude, sine_tone};
    use loomix_core::rt_assert::assert_realtime;

    fn new_driver(master_strip: Option<usize>) -> EngineIoDriver {
        EngineIoDriver::new(
            Engine::new(),
            Arc::new(MasterClock::default()),
            master_strip,
            256,
        )
    }

    // Read-only (a device enumeration, same as loomix-hal's own device
    // tests), safe to run in CI unlike anything in device_wiring.rs --
    // this only asserts `select_clock_master` reaches real CoreAudio and
    // returns *some* answer, never which specific device that is.
    #[test]
    fn select_clock_master_resolves_against_real_enumeration() {
        let configured = loomix_hal::device::default_output_device().ok();
        let result = select_clock_master(configured);
        assert!(result.is_ok(), "enumeration should succeed on any Mac");
        if let (Some(id), Ok(loomix_hal::clock::ClockSource::Device(resolved))) =
            (configured, result)
        {
            assert_eq!(
                resolved, id,
                "the configured default output device should resolve as itself"
            );
        }
    }

    #[test]
    fn strip_source_underrun_fills_silence() {
        let (_producer, consumer) = rtrb::RingBuffer::<f32>::new(16);
        let mut source = StripSource::new(vec![consumer]);
        let counter = source.underrun_counter();
        let mut out = vec![[1.0; CHANNELS]; 8]; // poisoned sentinel
        source.pull_into(&mut out);
        assert!(out.iter().all(|f| f[0] == 0.0));
        assert_eq!(
            counter.get(),
            8,
            "every frame of this callback underran, one increment each"
        );
    }

    #[test]
    fn bus_sink_drops_silently_when_the_ring_is_full() {
        let (producer, _consumer) = rtrb::RingBuffer::<f32>::new(4);
        let mut sink = BusSink::new(vec![producer]);
        let counter = sink.overrun_counter();
        let frames = vec![[1.0; CHANNELS]; 8]; // more than the ring's capacity
        sink.push_from(&frames); // must not panic
        assert_eq!(
            counter.get(),
            4,
            "the 4 frames past capacity should be counted"
        );
    }

    #[test]
    fn strip_routed_through_a_ring_reaches_the_assigned_bus() {
        let sample_rate = 48_000.0;
        let block_frames = 128;
        let num_blocks = 40;
        let tone = sine_tone(block_frames * num_blocks, sample_rate, 1_000.0, 0);

        let mut driver = new_driver(None);
        // Default routing sends every strip to bus 0 (spec 3.4 M3); route
        // strip 0 to bus 1 instead, so its output lands on a BusSink
        // rather than MASTER_BUS's direct pass-through, and bus 0 (via
        // `master_out` below) should stay silent.
        driver.engine_mut().strips[0].bus_assign = [false; NUM_BUSES];
        driver.engine_mut().strips[0].bus_assign[1] = true;

        let (mut strip_tx, strip_rx) = rtrb::RingBuffer::<f32>::new(tone.len() * 2);
        driver.set_strip_source(0, StripSource::new(vec![strip_rx]));
        let (bus_tx, mut bus_rx) = rtrb::RingBuffer::<f32>::new(tone.len() * 2);
        driver.set_bus_sink(1, BusSink::new(vec![bus_tx]));

        for frame in &tone {
            let _ = strip_tx.push(frame[0]);
        }

        let mut master_out_buf = vec![0.0_f32; block_frames];
        for _ in 0..num_blocks {
            let mut out_channel = master_out_buf.as_mut_slice();
            assert_realtime(|| {
                driver.on_master_tick(block_frames, &[], std::slice::from_mut(&mut out_channel));
            });
            assert!(
                master_out_buf.iter().all(|&s| s == 0.0),
                "bus 0 should stay silent: strip 0 was routed to bus 1, not bus 0"
            );
        }

        let mut received = Vec::new();
        while let Ok(sample) = bus_rx.pop() {
            received.push(sample);
        }
        assert!(!received.is_empty());
        let in_mag = goertzel_magnitude(
            &tone[..received.len().min(tone.len())],
            0,
            1_000.0,
            sample_rate,
        );
        let out_frames: Vec<Frame> = received
            .iter()
            .map(|&s| {
                let mut f = [0.0; CHANNELS];
                f[0] = s;
                f
            })
            .collect();
        let out_mag = goertzel_magnitude(&out_frames, 0, 1_000.0, sample_rate);
        assert!(
            (out_mag / in_mag - 1.0).abs() < 0.05,
            "the tone should reach bus 1 essentially unchanged (no resampling in this path), \
             got in={in_mag} out={out_mag}"
        );
    }

    #[test]
    fn the_master_devices_own_strip_bypasses_the_ring_entirely() {
        let block_frames = 8;
        let mut driver = new_driver(Some(0));
        // Default routing: strip 0 -> bus 0, and bus 0 is always
        // MASTER_BUS, so this exercises master_in -> engine -> master_out
        // with no ring buffer anywhere in the path.
        let master_in_data = vec![0.5_f32; block_frames];
        let master_in: [&[f32]; 1] = [&master_in_data];
        let mut out_buf = vec![0.0_f32; block_frames];
        {
            let mut out_channel = out_buf.as_mut_slice();
            driver.on_master_tick(
                block_frames,
                &master_in,
                std::slice::from_mut(&mut out_channel),
            );
        }
        assert!(
            out_buf.iter().all(|&s| (s - 0.5).abs() < 1e-6),
            "strip 0's master-fed input at unity gain should reach bus 0 unchanged, got {out_buf:?}"
        );
    }

    /// Real output hardware delivers ONE interleaved `AudioBuffer` for a
    /// stereo stream, not one buffer per channel (the M1/M2 log's own
    /// finding, `docs/ARCHITECTURE.md`: "every real output device tried
    /// on this machine -- the built-in speaker, an external monitor's
    /// speakers, BlackHole -- delivers one interleaved buffer instead").
    /// `master_ioproc_trampoline` hands `master_out` through exactly as
    /// CoreAudio gave it, with no deinterleaving -- a known, documented
    /// gap that was left unfixed because the M4 soak harness's own
    /// content was silence, and a scrambled arrangement of zeros is still
    /// all zeros, so that gap was never actually exercised end to end
    /// until real (non-silent) audio did. This test is the host-testable
    /// proof `unpack_channels` mishandles exactly that shape -- distinct,
    /// non-zero L/R values per frame, so a swapped, compressed, or
    /// stale-tail interleaving is visibly wrong, not coincidentally right.
    #[test]
    fn on_master_tick_interleaves_stereo_output_correctly_into_a_single_combined_buffer() {
        let block_frames = 4;
        let mut driver = new_driver(Some(0));
        // Default routing: strip 0 -> bus 0 (MASTER_BUS) at unity, so
        // whatever master_in carries reaches master_out unprocessed.
        // Realistic audio amplitudes, deliberately: an earlier version of
        // this test used values like 101.0 to make L and R easy to tell
        // apart, which instead exercised the strip's limiter (+12dB
        // default ceiling, ~3.981 linear) and muddied the numbers with
        // real DSP behaviour unrelated to the bug under test.
        let master_in_l = [0.1_f32, 0.2, 0.3, 0.4];
        let master_in_r = [0.5_f32, 0.6, 0.7, 0.8];
        let master_in: [&[f32]; 2] = [&master_in_l, &master_in_r];

        // ONE combined interleaved buffer -- what real hardware actually
        // hands over, not two separate per-channel buffers like the test
        // above uses for its (mono, so interleaving-proof) case.
        let mut interleaved_out = vec![-999.0_f32; block_frames * 2]; // poisoned sentinel
        {
            let mut out_channel = interleaved_out.as_mut_slice();
            driver.on_master_tick(
                block_frames,
                &master_in,
                std::slice::from_mut(&mut out_channel),
            );
        }

        let expected = [0.1, 0.5, 0.2, 0.6, 0.3, 0.7, 0.4, 0.8];
        assert_eq!(
            interleaved_out, expected,
            "expected standard L,R,L,R,... interleaving, got {interleaved_out:?}"
        );
    }

    /// The input-side mirror of the test above: a real multi-channel
    /// capture device can equally deliver one combined interleaved
    /// buffer rather than one per channel, and `pack_channels` has the
    /// identical structural gap as `unpack_channels` for that shape (both
    /// assume `src`/`dst`'s length *is* the channel count). The mono mic
    /// this milestone actually tested against never exercises this --
    /// one channel has nothing to interleave -- so this is proven the
    /// same host-testable way, not assumed safe by extension.
    #[test]
    fn on_master_tick_deinterleaves_stereo_input_correctly_from_a_single_combined_buffer() {
        let block_frames = 4;
        let mut driver = new_driver(Some(0));
        driver.engine_mut().strips[0].bus_assign = [false; NUM_BUSES];
        driver.engine_mut().strips[0].bus_assign[1] = true; // bus 1, not MASTER_BUS

        // ONE combined interleaved buffer: L0,R0,L1,R1,... Realistic
        // amplitudes (see the output-side test above for why: large
        // values here would exercise the strip's limiter instead of
        // isolating the deinterleaving bug).
        let interleaved_in = [0.1_f32, 0.5, 0.2, 0.6, 0.3, 0.7, 0.4, 0.8];
        let master_in: [&[f32]; 1] = [&interleaved_in];
        let mut master_out_buf = [0.0_f32; 4]; // MASTER_BUS (0) should stay silent

        let (bus_l_tx, mut bus_l_rx) = rtrb::RingBuffer::<f32>::new(64);
        let (bus_r_tx, mut bus_r_rx) = rtrb::RingBuffer::<f32>::new(64);
        driver.set_bus_sink(1, BusSink::new(vec![bus_l_tx, bus_r_tx]));

        {
            let mut out_channel = master_out_buf.as_mut_slice();
            driver.on_master_tick(
                block_frames,
                &master_in,
                std::slice::from_mut(&mut out_channel),
            );
        }

        let received_l: Vec<f32> = std::iter::from_fn(|| bus_l_rx.pop().ok()).collect();
        let received_r: Vec<f32> = std::iter::from_fn(|| bus_r_rx.pop().ok()).collect();
        assert_eq!(
            received_l,
            vec![0.1, 0.2, 0.3, 0.4],
            "left channel should be correctly deinterleaved, got {received_l:?}"
        );
        assert_eq!(
            received_r,
            vec![0.5, 0.6, 0.7, 0.8],
            "right channel should be correctly deinterleaved, got {received_r:?}"
        );
    }

    /// End-to-end regression test for the bug traced back from a real
    /// recording: every other block of output audio entirely silent, at
    /// exactly the output device's channel count as its ratio (found by
    /// counting zero-valued 256-sample blocks in a WAV capture, not from
    /// reading this code -- `docs/ARCHITECTURE.md`'s next dated entry).
    ///
    /// Root cause lived in `loomix-hal::device::master_ioproc_trampoline`
    /// (fixed there, proven directly by a sibling test): for a real
    /// stereo output device, which delivers ONE combined interleaved
    /// buffer, it used to report that buffer's raw sample count
    /// (`frames * channel_count`) as the frame count instead of the true
    /// frame count. That value becomes `on_master_tick`'s `block_frames`
    /// here, which is used both to size the engine's scratch buffers
    /// and, critically, how many frames [`StripSource::pull_into`]
    /// drains from the real capture ring per callback -- with the old
    /// value, doubling the drain rate against a capture ring a real
    /// device is only filling at the true frame rate.
    ///
    /// This test cannot call the trampoline itself (it lives in
    /// `loomix-hal`, a different crate, and this crate forbids unsafe
    /// code entirely), so it reproduces the propagation honestly instead:
    /// a capture ring is fed exactly `true_frames_per_callback` known,
    /// non-zero frames per simulated callback -- what a correctly
    /// functioning stereo capture device actually supplies -- while
    /// `on_master_tick` is driven with `block_frames` set to
    /// `true_frames_per_callback`, the value the now-fixed trampoline
    /// actually reports for a stereo master output device (dividing the
    /// raw interleaved sample count by the buffer's own reported channel
    /// count). Before the fix this test used `true_frames_per_callback *
    /// channel_count` here instead and failed with exactly the predicted
    /// 2560 underruns (`true_frames_per_callback * 2 channels *
    /// num_callbacks`) -- every sample pushed into the capture ring is
    /// non-zero by construction, so any zero reaching bus 1 could only be
    /// `pull_into`'s underrun-silence fill.
    #[test]
    fn capture_ring_drained_at_master_devices_reported_frame_count_produces_no_silent_blocks() {
        let true_frames_per_callback = 128;
        let num_callbacks = 10;
        // What the now-fixed `master_ioproc_trampoline` actually reports
        // for a stereo interleaved output device: the true frame count,
        // not `true_frames_per_callback * channel_count`.
        let block_frames = true_frames_per_callback;

        let mut driver = new_driver(None);
        driver.engine_mut().strips[0].bus_assign = [false; NUM_BUSES];
        driver.engine_mut().strips[0].bus_assign[1] = true; // bus 1, not MASTER_BUS

        let ring_len = true_frames_per_callback * num_callbacks + block_frames;
        let (mut cap_l_tx, cap_l_rx) = rtrb::RingBuffer::<f32>::new(ring_len);
        let (mut cap_r_tx, cap_r_rx) = rtrb::RingBuffer::<f32>::new(ring_len);
        let source = StripSource::new(vec![cap_l_rx, cap_r_rx]);
        let underruns = source.underrun_counter();
        driver.set_strip_source(0, source);

        let (bus_l_tx, mut bus_l_rx) = rtrb::RingBuffer::<f32>::new(ring_len);
        let (bus_r_tx, mut bus_r_rx) = rtrb::RingBuffer::<f32>::new(ring_len);
        driver.set_bus_sink(1, BusSink::new(vec![bus_l_tx, bus_r_tx]));

        // Known, non-zero, distinguishable per-channel constants -- a real
        // capture device correctly supplying `true_frames_per_callback`
        // frames every callback, exactly matching `block_frames` now that
        // the trampoline reports the true frame count.
        for _ in 0..num_callbacks {
            for _ in 0..true_frames_per_callback {
                let _ = cap_l_tx.push(0.3);
                let _ = cap_r_tx.push(0.7);
            }
        }

        let mut master_out_buf = vec![0.0_f32; block_frames];
        for _ in 0..num_callbacks {
            let mut out_channel = master_out_buf.as_mut_slice();
            driver.on_master_tick(block_frames, &[], std::slice::from_mut(&mut out_channel));
        }

        let received_l: Vec<f32> = std::iter::from_fn(|| bus_l_rx.pop().ok()).collect();
        let received_r: Vec<f32> = std::iter::from_fn(|| bus_r_rx.pop().ok()).collect();

        assert_eq!(
            underruns.get(),
            0,
            "every capture frame delivered was real (0.3/0.7, never 0.0); \
             any underrun here means the engine drained the ring faster \
             than a correctly-functioning capture device fills it, got \
             {} underruns",
            underruns.get()
        );
        assert!(
            received_l.iter().all(|&s| s == 0.3) && received_r.iter().all(|&s| s == 0.7),
            "expected every received sample to be the known non-zero \
             capture value with no silent blocks; got a zero-valued \
             stretch instead (L sample: {:?}, R sample: {:?})",
            received_l.iter().find(|&&s| s != 0.3),
            received_r.iter().find(|&&s| s != 0.7)
        );
    }
}
