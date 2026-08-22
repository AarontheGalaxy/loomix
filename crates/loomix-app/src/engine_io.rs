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

/// One strip's captured audio, arriving as one ring-buffer consumer per
/// device channel (filled by that device's `CaptureIoProcHandle`).
pub struct StripSource {
    channels: Vec<Consumer<f32>>,
}

impl StripSource {
    pub fn new(channels: Vec<Consumer<f32>>) -> Self {
        Self { channels }
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
                frame[channel] = consumer.pop().unwrap_or(0.0);
            }
        }
    }
}

/// One bus's output, arriving as one ring-buffer producer per device
/// channel (drained by that device's `RenderIoProcHandle`).
pub struct BusSink {
    channels: Vec<Producer<f32>>,
}

impl BusSink {
    pub fn new(channels: Vec<Producer<f32>>) -> Self {
        Self { channels }
    }

    /// Pushes `input` out, channel by channel, up to [`CHANNELS`] device
    /// channels. A full ring (the device side hasn't caught up) drops the
    /// newest samples rather than blocking -- same policy as
    /// `DriftCorrectedIoStage::on_capture`.
    fn push_from(&mut self, input: &[Frame]) {
        for (channel, producer) in self.channels.iter_mut().enumerate().take(CHANNELS) {
            for frame in input {
                let _ = producer.push(frame[channel]);
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
/// attached yet" during M4, before M11's network audio exists to fill
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
    /// (`loomix_hal::device::MasterIoProcHandle`). `master_in` is that
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

fn pack_channels(src: &[&[f32]], dst: &mut [Frame]) {
    for frame in dst.iter_mut() {
        *frame = [0.0; CHANNELS];
    }
    for (channel, data) in src.iter().enumerate().take(CHANNELS) {
        for (frame, &sample) in dst.iter_mut().zip(data.iter()) {
            frame[channel] = sample;
        }
    }
}

fn unpack_channels(src: &[Frame], dst: &mut [&mut [f32]]) {
    for (channel, out_channel) in dst.iter_mut().enumerate().take(CHANNELS) {
        for (frame, out_sample) in src.iter().zip(out_channel.iter_mut()) {
            *out_sample = frame[channel];
        }
    }
}

/// Creates `channel_count` ring buffers, registers `device` as a
/// drift-corrected capture IOProc (`loomix-hal`'s already-proven
/// `DriftCorrectedIoStage`), and points `driver`'s `strip` at the
/// consumer side. Returns the handle keeping the registration alive --
/// dropping it stops and unregisters the device.
pub fn attach_capture_device(
    driver: &mut EngineIoDriver,
    strip: usize,
    device: DeviceId,
    channel_count: usize,
    master_clock: Arc<MasterClock>,
    corrector: loomix_hal::drift::DriftCorrector,
    ring_capacity: usize,
) -> Result<loomix_hal::device::CaptureIoProcHandle, CoreAudioError> {
    let mut producers = Vec::with_capacity(channel_count);
    let mut consumers = Vec::with_capacity(channel_count);
    for _ in 0..channel_count {
        let (producer, consumer) = rtrb::RingBuffer::new(ring_capacity);
        producers.push(producer);
        consumers.push(consumer);
    }
    let stage = loomix_hal::ioproc::DriftCorrectedIoStage::new(channel_count, corrector);
    let ctx = loomix_hal::device::CaptureIoProcContext::new(
        stage,
        master_clock,
        producers,
        ring_capacity,
    );
    let handle = loomix_hal::device::CaptureIoProcHandle::start(device, ctx)?;
    driver.set_strip_source(strip, StripSource::new(consumers));
    Ok(handle)
}

/// The render-side mirror of [`attach_capture_device`]: registers `device`
/// as a drift-corrected render IOProc and points `driver`'s `bus` at the
/// producer side.
pub fn attach_render_device(
    driver: &mut EngineIoDriver,
    bus: usize,
    device: DeviceId,
    channel_count: usize,
    master_clock: Arc<MasterClock>,
    corrector: loomix_hal::drift::DriftCorrector,
    ring_capacity: usize,
) -> Result<loomix_hal::device::RenderIoProcHandle, CoreAudioError> {
    let mut producers = Vec::with_capacity(channel_count);
    let mut consumers = Vec::with_capacity(channel_count);
    for _ in 0..channel_count {
        let (producer, consumer) = rtrb::RingBuffer::new(ring_capacity);
        producers.push(producer);
        consumers.push(consumer);
    }
    let stage = loomix_hal::ioproc::DriftCorrectedIoStage::new(channel_count, corrector);
    let ctx =
        loomix_hal::device::RenderIoProcContext::new(stage, master_clock, consumers, ring_capacity);
    let handle = loomix_hal::device::RenderIoProcHandle::start(device, ctx)?;
    driver.set_bus_sink(bus, BusSink::new(producers));
    Ok(handle)
}

/// Registers `device` as the clock-master IOProc (spec 1.19), driving
/// `driver`'s engine tick from here on. Takes `driver` by value
/// deliberately: once the master starts, `driver` lives entirely on its
/// real-time thread inside the registered callback, not shared with
/// anything else -- call this last, after every other device has already
/// been attached with [`attach_capture_device`]/[`attach_render_device`].
pub fn attach_master_device(
    device: DeviceId,
    mut driver: EngineIoDriver,
) -> Result<loomix_hal::device::MasterIoProcHandle, CoreAudioError> {
    let callback: loomix_hal::device::MasterTickCallback =
        Box::new(move |frames, input, output| {
            driver.on_master_tick(frames as usize, input, output);
        });
    loomix_hal::device::MasterIoProcHandle::start(device, callback)
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

    #[test]
    fn strip_source_underrun_fills_silence() {
        let (_producer, consumer) = rtrb::RingBuffer::<f32>::new(16);
        let mut source = StripSource::new(vec![consumer]);
        let mut out = vec![[1.0; CHANNELS]; 8]; // poisoned sentinel
        source.pull_into(&mut out);
        assert!(out.iter().all(|f| f[0] == 0.0));
    }

    #[test]
    fn bus_sink_drops_silently_when_the_ring_is_full() {
        let (producer, _consumer) = rtrb::RingBuffer::<f32>::new(4);
        let mut sink = BusSink::new(vec![producer]);
        let frames = vec![[1.0; CHANNELS]; 8]; // more than the ring's capacity
        sink.push_from(&frames); // must not panic
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
}
