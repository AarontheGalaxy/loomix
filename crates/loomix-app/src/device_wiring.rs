//! The part of `engine_io.rs` that actually registers a device with
//! `loomix-hal` -- creating ring buffers and starting a real IOProc
//! (`loomix_hal::device_lifecycle::{Capture,Render,Master}IoProcHandle::start`).
//! Split out deliberately, same reasoning as
//! `loomix_hal::device_lifecycle`'s own doc comment: starting real device
//! I/O isn't something the automated test suite does, so nothing here is
//! exercised by it. `engine_io.rs`'s `StripSource`/`BusSink`/
//! `EngineIoDriver`/`select_clock_master` stay there because they *are`
//! -- pure ring-assembly logic and a read-only enumeration call, both
//! covered by tests in that file.
//!
//! Coverage: excluded from the `cargo llvm-cov` gate via
//! `--ignore-filename-regex` in `justfile`/`ci.yml`, with this doc comment
//! as the explicit reason.

use crate::engine_io::{BusSink, DropoutCounter, EngineIoDriver, StripSource};
use loomix_hal::clock::DeviceId;
use loomix_hal::device::CoreAudioError;
use loomix_hal::master_clock::MasterClock;
use std::sync::Arc;

/// What [`attach_capture_device`]/[`attach_render_device`] hand back: the
/// registration keeping the device active (dropping it stops and
/// unregisters), plus the live monitoring handles a soak harness (spec
/// 3.4 M4) polls -- the resample ratio and the dropout count -- without
/// needing to reach back into the `EngineIoDriver` at all.
pub struct AttachedDevice<H> {
    pub io: H,
    pub ratio: loomix_hal::ioproc::RatioHandle,
    pub dropouts: DropoutCounter,
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
) -> Result<AttachedDevice<loomix_hal::device_lifecycle::CaptureIoProcHandle>, CoreAudioError> {
    let mut producers = Vec::with_capacity(channel_count);
    let mut consumers = Vec::with_capacity(channel_count);
    for _ in 0..channel_count {
        let (producer, consumer) = rtrb::RingBuffer::new(ring_capacity);
        producers.push(producer);
        consumers.push(consumer);
    }
    let stage = loomix_hal::ioproc::DriftCorrectedIoStage::new(channel_count, corrector);
    let ratio = stage.ratio_handle();
    let ctx = loomix_hal::device::CaptureIoProcContext::new(
        stage,
        master_clock,
        producers,
        ring_capacity,
    );
    let io = loomix_hal::device_lifecycle::CaptureIoProcHandle::start(device, ctx)?;
    let source = StripSource::new(consumers);
    let dropouts = source.underrun_counter();
    driver.set_strip_source(strip, source);
    Ok(AttachedDevice {
        io,
        ratio,
        dropouts,
    })
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
) -> Result<AttachedDevice<loomix_hal::device_lifecycle::RenderIoProcHandle>, CoreAudioError> {
    let mut producers = Vec::with_capacity(channel_count);
    let mut consumers = Vec::with_capacity(channel_count);
    for _ in 0..channel_count {
        let (producer, consumer) = rtrb::RingBuffer::new(ring_capacity);
        producers.push(producer);
        consumers.push(consumer);
    }
    let stage = loomix_hal::ioproc::DriftCorrectedIoStage::new(channel_count, corrector);
    let ratio = stage.ratio_handle();
    let ctx =
        loomix_hal::device::RenderIoProcContext::new(stage, master_clock, consumers, ring_capacity);
    let io = loomix_hal::device_lifecycle::RenderIoProcHandle::start(device, ctx)?;
    let sink = BusSink::new(producers);
    let dropouts = sink.overrun_counter();
    driver.set_bus_sink(bus, sink);
    Ok(AttachedDevice {
        io,
        ratio,
        dropouts,
    })
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
) -> Result<loomix_hal::device_lifecycle::MasterIoProcHandle, CoreAudioError> {
    let callback: loomix_hal::device::MasterTickCallback =
        Box::new(move |frames, input, output| {
            driver.on_master_tick(frames as usize, input, output);
        });
    loomix_hal::device_lifecycle::MasterIoProcHandle::start(device, callback)
}
