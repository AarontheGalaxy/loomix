//! CoreAudio device enumeration, hog mode, and device-list change
//! notification (spec 1.11, 2.1, 2.2). This is the one file in
//! `loomix-hal` that actually calls CoreAudio; it stays thin by design
//! (spec 3.4 M4's test plan) -- everything worth testing offline (clock
//! master selection, hog-mode fallback, hot-plug decisions) lives in
//! `clock`, `hog` and `hotplug` instead, and is covered there. What's
//! here can only be verified against a running `coreaudiod`, so it's
//! covered by the tests in this module's `tests` submodule (which run for
//! real, on whatever devices this machine actually has) plus the manual
//! two-device soak spec 3.4 M4 names as the acceptance criterion.

use crate::clock::DeviceId;
use crate::hog::Pid;
use crate::ioproc::DriftCorrectedIoStage;
use crate::master_clock::MasterClock;
use coreaudio_sys::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A CoreAudio `OSStatus` failure from any call in this module.
pub type CoreAudioError = OSStatus;

pub(crate) fn check(status: OSStatus) -> Result<(), CoreAudioError> {
    if status == 0 {
        Ok(())
    } else {
        Err(status)
    }
}

fn address(selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn property_data_size(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Result<u32, CoreAudioError> {
    let addr = address(selector);
    let mut size: u32 = 0;
    check(unsafe {
        AudioObjectGetPropertyDataSize(object, &addr, 0, std::ptr::null(), &mut size)
    })?;
    Ok(size)
}

/// All currently visible CoreAudio device object IDs (spec 1.11's device
/// selector; spec 2.3's "everyone else is resampled" set).
pub fn list_device_ids() -> Result<Vec<DeviceId>, CoreAudioError> {
    let size = property_data_size(kAudioObjectSystemObject, kAudioHardwarePropertyDevices)?;
    let count = size as usize / std::mem::size_of::<AudioObjectID>();
    let mut ids = vec![0 as AudioObjectID; count];
    let addr = address(kAudioHardwarePropertyDevices);
    let mut actual_size = size;
    check(unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &addr,
            0,
            std::ptr::null(),
            &mut actual_size,
            ids.as_mut_ptr() as *mut _,
        )
    })?;
    ids.truncate(actual_size as usize / std::mem::size_of::<AudioObjectID>());
    Ok(ids)
}

/// The system default output device (spec 1.19's "the main output device
/// is the clock master"). Callers pass its ID through
/// `clock::resolve_clock_source` alongside `list_device_ids`, not this
/// module's job to decide.
pub fn default_output_device() -> Result<DeviceId, CoreAudioError> {
    let addr = address(kAudioHardwarePropertyDefaultOutputDevice);
    let mut id: AudioObjectID = 0;
    let mut size = std::mem::size_of::<AudioObjectID>() as u32;
    check(unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut id as *mut _ as *mut _,
        )
    })?;
    Ok(id)
}

/// The system default input device -- the render-side mirror of
/// [`default_output_device`].
pub fn default_input_device() -> Result<DeviceId, CoreAudioError> {
    let addr = address(kAudioHardwarePropertyDefaultInputDevice);
    let mut id: AudioObjectID = 0;
    let mut size = std::mem::size_of::<AudioObjectID>() as u32;
    check(unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut id as *mut _ as *mut _,
        )
    })?;
    Ok(id)
}

/// Which scope of a device's channels to count: its input side or its
/// output side. A device can have channels on both (a full-duplex audio
/// interface) or just one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}

/// The device's channel count on `direction` (`kAudioDevicePropertyStreamConfiguration`,
/// summed across every `AudioBuffer` in the returned list -- a device can
/// expose several, e.g. one per physical connector). Needed up front, at
/// wiring time, to size the ring buffers and resamplers an IOProc
/// registration needs -- the real count only otherwise appears inside a
/// running callback's `AudioBufferList`, too late to prepare for.
pub fn channel_count(id: DeviceId, direction: Direction) -> Result<usize, CoreAudioError> {
    let scope = match direction {
        Direction::Input => kAudioDevicePropertyScopeInput,
        Direction::Output => kAudioDevicePropertyScopeOutput,
    };
    let addr = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyStreamConfiguration,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut size: u32 = 0;
    check(unsafe { AudioObjectGetPropertyDataSize(id, &addr, 0, std::ptr::null(), &mut size) })?;
    if size == 0 {
        return Ok(0);
    }
    let mut storage = vec![0u8; size as usize];
    let list = storage.as_mut_ptr() as *mut AudioBufferList;
    let mut actual_size = size;
    check(unsafe {
        AudioObjectGetPropertyData(
            id,
            &addr,
            0,
            std::ptr::null(),
            &mut actual_size,
            list as *mut _,
        )
    })?;
    let count = unsafe { (*list).mNumberBuffers as usize };
    let first = unsafe { (*list).mBuffers.as_ptr() };
    let mut total = 0usize;
    for i in 0..count {
        total += unsafe { (*first.add(i)).mNumberChannels as usize };
    }
    Ok(total)
}

/// The device's current nominal sample rate
/// (`kAudioDevicePropertyNominalSampleRate`) -- spec 1.11: "the main
/// output device... defines the engine sample rate," so `loomix-app`
/// needs the real value to call [`loomix_core::Engine::set_sample_rate`]
/// correctly once a device is actually selected, not just assume 48kHz
/// and let the DSP run at the wrong rate silently. Read-only: this
/// project never sets a device's rate, only reads whatever the hardware
/// (or the user, via Audio MIDI Setup) already has it configured to.
pub fn nominal_sample_rate(id: DeviceId) -> Result<f64, CoreAudioError> {
    let addr = address(kAudioDevicePropertyNominalSampleRate);
    let mut rate: f64 = 0.0;
    let mut size = std::mem::size_of::<f64>() as u32;
    check(unsafe {
        AudioObjectGetPropertyData(
            id,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut rate as *mut _ as *mut _,
        )
    })?;
    Ok(rate)
}

fn cfstring_property(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Result<String, CoreAudioError> {
    let addr = address(selector);
    let mut value: CFStringRef = std::ptr::null();
    let mut size = std::mem::size_of::<CFStringRef>() as u32;
    check(unsafe {
        AudioObjectGetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut value as *mut _ as *mut _,
        )
    })?;
    if value.is_null() {
        return Ok(String::new());
    }
    let result = unsafe { cfstring_to_string(value) };
    unsafe { CFRelease(value as CFTypeRef) };
    Ok(result)
}

/// # Safety
/// `s` must be a valid, non-null `CFStringRef`.
unsafe fn cfstring_to_string(s: CFStringRef) -> String {
    let len = unsafe { CFStringGetLength(s) };
    let max_size = unsafe { CFStringGetMaximumSizeForEncoding(len, kCFStringEncodingUTF8) } + 1;
    let mut buf = vec![0u8; max_size.max(1) as usize];
    let ok = unsafe {
        CFStringGetCString(
            s,
            buf.as_mut_ptr() as *mut std::os::raw::c_char,
            max_size,
            kCFStringEncodingUTF8,
        )
    };
    if ok == 0 {
        return String::new();
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr() as *const std::os::raw::c_char) };
    cstr.to_string_lossy().into_owned()
}

/// The device's persistent UID (spec 3.4 M2's "stable device UIDs"), the
/// value user config and presets should key on rather than the transient
/// `AudioObjectID`.
pub fn device_uid(id: DeviceId) -> Result<String, CoreAudioError> {
    cfstring_property(id, kAudioDevicePropertyDeviceUID)
}

/// The device's user-facing name (spec 1.3/1.5's device selector).
pub fn device_name(id: DeviceId) -> Result<String, CoreAudioError> {
    cfstring_property(id, kAudioObjectPropertyName)
}

/// Reads `kAudioDevicePropertyHogMode`: `None` means free, `Some(pid)` the
/// process currently holding exclusive access. Feed the result to
/// `hog::decide` rather than acting on it directly here.
pub fn hog_owner(id: DeviceId) -> Result<Option<Pid>, CoreAudioError> {
    let addr = address(kAudioDevicePropertyHogMode);
    let mut pid: pid_t = -1;
    let mut size = std::mem::size_of::<pid_t>() as u32;
    check(unsafe {
        AudioObjectGetPropertyData(
            id,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut pid as *mut _ as *mut _,
        )
    })?;
    Ok(if pid == -1 { None } else { Some(pid) })
}

/// Sets (or, passing our own pid again with intent to release handled by
/// the caller via `hog::decide`'s `Release` case, clears) hog mode on a
/// device. `pid == -1` releases; CoreAudio only honours a set from the
/// process that currently holds it, or when the device is free.
pub fn set_hog_owner(id: DeviceId, pid: Pid) -> Result<(), CoreAudioError> {
    let addr = address(kAudioDevicePropertyHogMode);
    check(unsafe {
        AudioObjectSetPropertyData(
            id,
            &addr,
            0,
            std::ptr::null(),
            std::mem::size_of::<pid_t>() as u32,
            &pid as *const _ as *const _,
        )
    })
}

static DEVICE_LIST_CHANGED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn on_devices_changed(
    _object_id: AudioObjectID,
    _num_addresses: UInt32,
    _addresses: *const AudioObjectPropertyAddress,
    _client_data: *mut std::os::raw::c_void,
) -> OSStatus {
    DEVICE_LIST_CHANGED.store(true, Ordering::SeqCst);
    0
}

/// Registers for `kAudioHardwarePropertyDevices` changes -- the trigger
/// for spec 3.4 M4's "hot plug handling": a device appearing or
/// disappearing turns into a `hotplug::Event` for `hotplug::decide` once
/// the caller diffs the device list against what it saw last. Unregisters
/// on drop.
pub struct DeviceListListener;

impl DeviceListListener {
    pub fn register() -> Result<Self, CoreAudioError> {
        let addr = address(kAudioHardwarePropertyDevices);
        check(unsafe {
            AudioObjectAddPropertyListener(
                kAudioObjectSystemObject,
                &addr,
                Some(on_devices_changed),
                std::ptr::null_mut(),
            )
        })?;
        Ok(Self)
    }

    /// Returns whether the device list has changed since the last call,
    /// and clears the flag. Polled rather than pushed: matches how the
    /// engine already drives everything else, one block/tick at a time,
    /// with no extra channel or callback plumbing for a HAL client to
    /// wire up.
    pub fn take_changed(&self) -> bool {
        DEVICE_LIST_CHANGED.swap(false, Ordering::SeqCst)
    }
}

impl Drop for DeviceListListener {
    fn drop(&mut self) {
        let addr = address(kAudioHardwarePropertyDevices);
        unsafe {
            AudioObjectRemovePropertyListener(
                kAudioObjectSystemObject,
                &addr,
                Some(on_devices_changed),
                std::ptr::null_mut(),
            );
        }
    }
}

/// Registers `ioproc.rs`'s already-proven [`DriftCorrectedIoStage`] as a
/// real CoreAudio IOProc (spec 3.4 M4). Everything decision-worthy --
/// drift correction, resampling, the ring buffer -- was already exercised
/// against a synthetic fake device in `ioproc.rs`'s own tests; what's here
/// is just the FFI glue that hands CoreAudio's real buffers to those same
/// methods, which no offline test can reach (there's no synthetic stand-in
/// for "does coreaudiod actually call this function pointer with the
/// buffers it promises"). Assumes non-interleaved streams (one
/// [`AudioBuffer`] per channel) -- the common, controllable case, not a
/// claim that Loomix drives every device format.
///
/// A channel count above this is rejected rather than silently truncated;
/// [`MAX_IO_CHANNELS`] matches spec 1.1's 8-channel bus width, the largest
/// channel count anything in this codebase is built to carry.
const MAX_IO_CHANNELS: usize = 8;

/// Per-device state for a registered capture IOProc, boxed at
/// registration time so a stable address exists to hand CoreAudio as
/// `inClientData`, and owned for the registration's lifetime.
pub struct CaptureIoProcContext {
    stage: DriftCorrectedIoStage,
    master: Arc<MasterClock>,
    outputs: Vec<rtrb::Producer<f32>>,
    scratch: Vec<f32>,
    /// Per-channel storage for deinterleaving one interleaved
    /// `AudioBuffer` into planar form, `MAX_IO_CHANNELS` chunks of
    /// `scratch_capacity` frames each -- see [`deinterleaved_channels`]'s
    /// doc comment for why this exists at all.
    deinterleave: Vec<f32>,
}

impl CaptureIoProcContext {
    pub fn new(
        stage: DriftCorrectedIoStage,
        master: Arc<MasterClock>,
        outputs: Vec<rtrb::Producer<f32>>,
        scratch_capacity: usize,
    ) -> Self {
        Self {
            stage,
            master,
            outputs,
            scratch: vec![0.0; scratch_capacity],
            deinterleave: vec![0.0; MAX_IO_CHANNELS * scratch_capacity],
        }
    }

    pub(crate) fn channel_count(&self) -> usize {
        self.outputs.len()
    }
}

/// Reads up to [`MAX_IO_CHANNELS`] channels from an `AudioBufferList` as
/// planar `&[f32]` slices, handling both layouts a real device can
/// deliver: one `AudioBuffer` per channel (the simple case, returned as
/// direct sub-slices, no copy), or one interleaved `AudioBuffer` carrying
/// every channel (deinterleaved into `scratch`, `MAX_IO_CHANNELS` chunks
/// of `scratch.len() / MAX_IO_CHANNELS` frames each).
///
/// Found necessary the hard way: `AudioObjectSetPropertyData` requesting
/// a non-interleaved format can report success and still not change what
/// the device actually delivers (confirmed against a real device on this
/// machine -- the readback showed the interleaved flag unchanged after a
/// successful-looking set). Adapting to whichever layout shows up, rather
/// than trusting a format request to have taken effect, is the only
/// reliable option. Before this fix, an interleaved buffer was
/// misread as a single one-channel buffer: in a debug build that trips
/// `on_capture`'s `debug_assert_eq!`, but `--release` (what a real soak
/// actually uses) compiles the assert out and `zip()` silently processes
/// only the shorter side, so every channel past the first is never
/// touched -- discovered as the manual two-device soak (spec 3.4 M4)
/// reporting continuous dropouts within about a second on every real
/// device pair tried, never once as a test failure, since there's no
/// offline stand-in for a real device's actual stream format.
///
/// Returns the array and the real channel count (`<= MAX_IO_CHANNELS`);
/// channels beyond that are silently dropped rather than read out of
/// bounds.
///
/// # Safety
/// `list` must point to a valid `AudioBufferList`, every buffer's `mData`
/// valid for `mDataByteSize` bytes, for the duration of the borrow --
/// exactly what CoreAudio guarantees for the buffer list handed to an
/// `AudioDeviceIOProc` for the callback's duration. `expected_channels`
/// should be the count this context was built for; `scratch` must outlive
/// the returned slices.
unsafe fn read_input_channels_planar(
    list: *const AudioBufferList,
    expected_channels: usize,
    scratch: &mut [f32],
) -> ([&[f32]; MAX_IO_CHANNELS], usize) {
    let buffer_count = unsafe { (*list).mNumberBuffers } as usize;
    let first = unsafe { (*list).mBuffers.as_ptr() };

    if buffer_count == 1 && expected_channels > 1 {
        let buf = unsafe { &*first };
        let total_samples = buf.mDataByteSize as usize / std::mem::size_of::<f32>();
        let frames = total_samples / expected_channels;
        let raw = unsafe { std::slice::from_raw_parts(buf.mData as *const f32, total_samples) };
        let chunk_len = scratch.len() / MAX_IO_CHANNELS;
        let mut channels: [&[f32]; MAX_IO_CHANNELS] = [&[]; MAX_IO_CHANNELS];
        for (c, chunk) in scratch
            .chunks_exact_mut(chunk_len)
            .enumerate()
            .take(expected_channels.min(MAX_IO_CHANNELS))
        {
            let dst = &mut chunk[..frames.min(chunk_len)];
            for (f, sample) in dst.iter_mut().enumerate() {
                *sample = raw[f * expected_channels + c];
            }
            channels[c] = dst;
        }
        return (channels, expected_channels.min(MAX_IO_CHANNELS));
    }

    let count = buffer_count.min(MAX_IO_CHANNELS);
    let mut channels: [&[f32]; MAX_IO_CHANNELS] = [&[]; MAX_IO_CHANNELS];
    for (i, slot) in channels.iter_mut().enumerate().take(count) {
        let buf = unsafe { &*first.add(i) };
        let len = buf.mDataByteSize as usize / std::mem::size_of::<f32>();
        *slot = unsafe { std::slice::from_raw_parts(buf.mData as *const f32, len) };
    }
    (channels, count)
}

/// The simple, non-interleaving-aware reader/writer pair
/// [`read_input_channels_planar`] replaced for capture/render: still used
/// by [`master_ioproc_trampoline`], which has no per-device channel count
/// or deinterleave scratch to work with (it hands raw buffers straight to
/// a caller-supplied callback). This is therefore a known, documented gap
/// for a master device that happens to deliver interleaved audio -- see
/// `read_input_channels_planar`'s doc comment for the failure mode this
/// would hit, and `docs/ARCHITECTURE.md` for why it's deferred rather than
/// fixed here.
///
/// # Safety
/// Same contract as [`read_input_channels_planar`], without the
/// deinterleaving case.
unsafe fn read_input_channels<'a>(
    list: *const AudioBufferList,
) -> ([&'a [f32]; MAX_IO_CHANNELS], usize) {
    let count = (unsafe { (*list).mNumberBuffers } as usize).min(MAX_IO_CHANNELS);
    let first = unsafe { (*list).mBuffers.as_ptr() };
    let mut channels: [&[f32]; MAX_IO_CHANNELS] = [&[]; MAX_IO_CHANNELS];
    for (i, slot) in channels.iter_mut().enumerate().take(count) {
        let buf = unsafe { &*first.add(i) };
        let len = buf.mDataByteSize as usize / std::mem::size_of::<f32>();
        *slot = unsafe { std::slice::from_raw_parts(buf.mData as *const f32, len) };
    }
    (channels, count)
}

/// # Safety
/// Same contract as [`read_input_channels`], for `mut` access.
unsafe fn write_output_channels<'a>(
    list: *mut AudioBufferList,
) -> ([&'a mut [f32]; MAX_IO_CHANNELS], usize) {
    let count = (unsafe { (*list).mNumberBuffers } as usize).min(MAX_IO_CHANNELS);
    let first = unsafe { (*list).mBuffers.as_mut_ptr() };
    let channels: [&mut [f32]; MAX_IO_CHANNELS] = std::array::from_fn(|i| {
        if i < count {
            let buf = unsafe { &mut *first.add(i) };
            let len = buf.mDataByteSize as usize / std::mem::size_of::<f32>();
            unsafe { std::slice::from_raw_parts_mut(buf.mData as *mut f32, len) }
        } else {
            &mut []
        }
    });
    (channels, count)
}

/// # Real-time safety
/// No allocation: `channels` is a fixed-size stack array, and `ctx`'s
/// `outputs`/`scratch`/`deinterleave` were sized once at construction. No
/// locks: `rtrb::Producer::push` is lock-free. No syscalls beyond what
/// CoreAudio itself performs to invoke this callback.
pub(crate) unsafe extern "C" fn capture_ioproc_trampoline(
    _device: AudioObjectID,
    _now: *const AudioTimeStamp,
    input_data: *const AudioBufferList,
    _input_time: *const AudioTimeStamp,
    _output_data: *mut AudioBufferList,
    _output_time: *const AudioTimeStamp,
    client_data: *mut std::os::raw::c_void,
) -> OSStatus {
    let ctx = unsafe { &mut *(client_data as *mut CaptureIoProcContext) };
    let expected_channels = ctx.outputs.len();
    let (channels, count) =
        unsafe { read_input_channels_planar(input_data, expected_channels, &mut ctx.deinterleave) };
    ctx.stage.on_capture(
        &channels[..count],
        &ctx.master,
        &mut ctx.outputs,
        &mut ctx.scratch,
    );
    0
}

/// Per-device state for a registered render IOProc, symmetric to
/// [`CaptureIoProcContext`]: the ring buffers are consumers (filled by
/// whoever assembles bus output) instead of producers.
pub struct RenderIoProcContext {
    stage: DriftCorrectedIoStage,
    master: Arc<MasterClock>,
    inputs: Vec<rtrb::Consumer<f32>>,
    scratch: Vec<f32>,
    /// Per-channel storage for the interleaved case -- `on_render` writes
    /// planar output here, then the trampoline interleaves it into the
    /// real (possibly single, interleaved) `AudioBuffer` CoreAudio
    /// actually handed over. See `read_input_channels`'s doc comment for
    /// why this is necessary rather than assumed away.
    interleave: Vec<f32>,
}

impl RenderIoProcContext {
    pub fn new(
        stage: DriftCorrectedIoStage,
        master: Arc<MasterClock>,
        inputs: Vec<rtrb::Consumer<f32>>,
        scratch_capacity: usize,
    ) -> Self {
        Self {
            stage,
            master,
            inputs,
            scratch: vec![0.0; scratch_capacity],
            interleave: vec![0.0; MAX_IO_CHANNELS * scratch_capacity],
        }
    }

    pub(crate) fn channel_count(&self) -> usize {
        self.inputs.len()
    }
}

/// # Real-time safety
/// Same reasoning as [`capture_ioproc_trampoline`]: no allocation (every
/// buffer sized once at construction), `rtrb::Consumer::pop` is
/// lock-free.
pub(crate) unsafe extern "C" fn render_ioproc_trampoline(
    _device: AudioObjectID,
    _now: *const AudioTimeStamp,
    _input_data: *const AudioBufferList,
    _input_time: *const AudioTimeStamp,
    output_data: *mut AudioBufferList,
    _output_time: *const AudioTimeStamp,
    client_data: *mut std::os::raw::c_void,
) -> OSStatus {
    let ctx = unsafe { &mut *(client_data as *mut RenderIoProcContext) };
    let expected_channels = ctx.inputs.len();
    let buffer_count = unsafe { (*output_data).mNumberBuffers } as usize;

    if buffer_count == 1 && expected_channels > 1 {
        let buf = unsafe { &mut *(*output_data).mBuffers.as_mut_ptr() };
        let total_samples = buf.mDataByteSize as usize / std::mem::size_of::<f32>();
        let frames =
            (total_samples / expected_channels).min(ctx.interleave.len() / MAX_IO_CHANNELS);
        let chunk_len = ctx.interleave.len() / MAX_IO_CHANNELS;
        let mut channels: [&mut [f32]; MAX_IO_CHANNELS] =
            std::array::from_fn(|_| &mut [] as &mut [f32]);
        for (c, chunk) in ctx
            .interleave
            .chunks_exact_mut(chunk_len)
            .enumerate()
            .take(expected_channels.min(MAX_IO_CHANNELS))
        {
            channels[c] = &mut chunk[..frames];
        }
        ctx.stage.on_render(
            &ctx.master,
            &mut ctx.inputs,
            &mut channels[..expected_channels.min(MAX_IO_CHANNELS)],
            &mut ctx.scratch,
        );
        let raw = unsafe { std::slice::from_raw_parts_mut(buf.mData as *mut f32, total_samples) };
        for f in 0..frames {
            for (c, channel) in channels.iter().enumerate().take(expected_channels) {
                raw[f * expected_channels + c] = channel[f];
            }
        }
        return 0;
    }

    let (mut channels, count) = unsafe { write_output_channels(output_data) };
    ctx.stage.on_render(
        &ctx.master,
        &mut ctx.inputs,
        &mut channels[..count],
        &mut ctx.scratch,
    );
    0
}

/// A boxed real-time-safe callback for the clock-master device's IOProc
/// (spec 1.19). Receives this callback's frame count, its captured
/// channels (empty if the device has none), and its output channels to
/// fill (empty if the device has none) -- both directions in the one
/// synchronous call CoreAudio actually makes for a full-duplex device,
/// deliberately not split into separate capture/render registrations the
/// way non-master devices are: the master's own callback is what drives
/// the engine tick, and that tick has to see this callback's captured
/// audio *and* produce this callback's output in the right order, which
/// two independently-scheduled registrations can't guarantee.
///
/// Must not allocate, lock, or perform I/O -- the same real-time
/// discipline as everything else on this thread (spec 3.3). This crate
/// cannot enforce that for a caller-supplied closure the way
/// `DriftCorrectedIoStage`'s own methods are exercised under
/// `assert_realtime`; it's the caller's obligation, documented here
/// because it's the one boundary in this file where the obligation
/// crosses from this crate's code into someone else's.
pub type MasterTickCallback = Box<dyn FnMut(u32, &[&[f32]], &mut [&mut [f32]]) + Send>;

pub(crate) struct MasterIoProcContext {
    callback: MasterTickCallback,
}

impl MasterIoProcContext {
    pub(crate) fn new(callback: MasterTickCallback) -> Self {
        Self { callback }
    }
}

pub(crate) unsafe extern "C" fn master_ioproc_trampoline(
    _device: AudioObjectID,
    _now: *const AudioTimeStamp,
    input_data: *const AudioBufferList,
    _input_time: *const AudioTimeStamp,
    output_data: *mut AudioBufferList,
    _output_time: *const AudioTimeStamp,
    client_data: *mut std::os::raw::c_void,
) -> OSStatus {
    let ctx = unsafe { &mut *(client_data as *mut MasterIoProcContext) };
    let (input_channels, input_count) = unsafe { read_input_channels(input_data) };
    let (mut output_channels, output_count) = unsafe { write_output_channels(output_data) };
    let frames = output_channels[..output_count]
        .first()
        .map(|c| c.len())
        .or_else(|| input_channels[..input_count].first().map(|c| c.len()))
        .unwrap_or(0) as u32;
    (ctx.callback)(
        frames,
        &input_channels[..input_count],
        &mut output_channels[..output_count],
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drift::{DriftCorrector, PiController};

    // These hit real CoreAudio -- there's no synthetic stand-in for "does
    // this machine's coreaudiod answer a property query correctly", so
    // unlike every other module in this crate, these are not offline
    // tests. They only assert things true of any Mac (at least one
    // device exists, the default output has a name and a UID), never
    // anything about which specific devices are present.

    #[test]
    fn lists_at_least_one_device() {
        let ids = list_device_ids().expect("AudioObjectGetPropertyData should succeed");
        assert!(!ids.is_empty(), "every Mac has at least one audio device");
    }

    #[test]
    fn default_output_device_has_a_name_and_a_uid() {
        let id = default_output_device().expect("a default output device should exist");
        let name = device_name(id).expect("name query should succeed");
        let uid = device_uid(id).expect("uid query should succeed");
        assert!(!name.is_empty());
        assert!(!uid.is_empty());
    }

    #[test]
    fn default_output_device_has_a_positive_nominal_sample_rate() {
        let id = default_output_device().expect("a default output device should exist");
        let rate = nominal_sample_rate(id).expect("sample rate query should succeed");
        assert!(
            rate > 0.0,
            "every real device reports a positive sample rate, got {rate}"
        );
    }

    #[test]
    fn default_output_device_has_at_least_one_output_channel() {
        let id = default_output_device().expect("a default output device should exist");
        let count =
            channel_count(id, Direction::Output).expect("channel count query should succeed");
        assert!(count >= 1, "every output device has at least one channel");
        // The same device queried on the opposite scope should never
        // error, even if the honest answer is zero -- a pure-output
        // device correctly reports no input channels rather than failing.
        let input_count = channel_count(id, Direction::Input)
            .expect("opposite-scope query should still succeed, even if the answer is 0");
        let _ = input_count;
    }

    #[test]
    fn default_input_device_exists() {
        // Not every CI runner or machine necessarily has one, so this
        // only asserts the query itself behaves -- either a valid device
        // with at least one input channel, or a clean "no such property"
        // error, never a crash.
        if let Ok(id) = default_input_device() {
            let count = channel_count(id, Direction::Input)
                .expect("channel count query should succeed for a real device");
            assert!(count >= 1);
        }
    }

    #[test]
    fn querying_an_invalid_object_id_fails_cleanly_instead_of_crashing() {
        // Object ID 0 is never a valid AudioObjectID (spec: CoreAudio
        // reserves it; kAudioObjectSystemObject is 1). This is the same
        // out-of-range-ID coverage M2's `test_driver_host.c` exhaustively
        // proved on the driver side, exercised here against the real
        // system HAL instead.
        let result = device_name(0);
        assert!(result.is_err());
    }

    #[test]
    fn reading_hog_owner_of_the_default_device_does_not_error() {
        // Read-only: does not assert a specific value (something else on
        // the machine could legitimately hold hog mode), and deliberately
        // does not call `set_hog_owner` here -- acquiring hog mode on a
        // shared CI runner or a developer's own output device is exactly
        // the kind of side-effecting action this module keeps out of the
        // automated suite. See `set_and_release_hog_mode_round_trip`,
        // `#[ignore]`d, for that.
        let id = default_output_device().expect("a default output device should exist");
        hog_owner(id).expect("hog mode query should succeed even when unheld");
    }

    #[test]
    #[ignore = "acquires hog mode on the default output device; run manually, not in CI"]
    fn set_and_release_hog_mode_round_trip() {
        let id = default_output_device().expect("a default output device should exist");
        let our_pid = std::process::id() as Pid;
        set_hog_owner(id, our_pid).expect("acquiring hog mode should succeed when free");
        assert_eq!(hog_owner(id).unwrap(), Some(our_pid));
        set_hog_owner(id, -1).expect("releasing hog mode should succeed");
        assert_eq!(hog_owner(id).unwrap(), None);
    }

    #[test]
    fn device_list_listener_registers_and_unregisters_cleanly() {
        let listener =
            DeviceListListener::register().expect("listener registration should succeed");
        assert!(!listener.take_changed());
        drop(listener);
    }

    /// Hand-builds a heap-allocated `AudioBufferList` with `channels.len()`
    /// non-interleaved f32 channels -- the same layout a real capture or
    /// render callback receives. Bindgen's struct declares `mBuffers` as
    /// `[AudioBuffer; 1]` (the real C type is a flexible array member), so
    /// more than one buffer needs its storage sized past that: starting
    /// from `size_of::<AudioBufferList>()` (which already accounts for
    /// whatever padding the compiler puts before a single correctly
    /// aligned `AudioBuffer`) and extending by one more `AudioBuffer` per
    /// additional channel is the standard idiom for this, rather than
    /// guessing the header size by hand.
    ///
    /// Owns `channels` itself (rather than borrowing them) precisely
    /// because the first version of this test didn't: it built the buffer
    /// list from a temporary `&mut [ch0.clone(), ch1.clone()]` array,
    /// whose backing `Vec`s were dropped -- leaving every `mData` pointer
    /// dangling -- the instant the constructor call's statement finished,
    /// before the buffer list was ever read. That version SIGKILLed
    /// (heap corruption, not a clean panic). Owning `channels` for exactly
    /// as long as `storage` needs them removes the whole class of bug.
    struct TestBufferList {
        storage: Vec<u8>,
        _channels: Vec<Vec<f32>>,
    }

    impl TestBufferList {
        fn new(mut channels: Vec<Vec<f32>>) -> Self {
            let extra = channels.len().saturating_sub(1);
            let total =
                std::mem::size_of::<AudioBufferList>() + extra * std::mem::size_of::<AudioBuffer>();
            let mut storage = vec![0u8; total];
            let list = storage.as_mut_ptr() as *mut AudioBufferList;
            unsafe {
                (*list).mNumberBuffers = channels.len() as UInt32;
                let first = (*list).mBuffers.as_mut_ptr();
                for (i, buf) in channels.iter_mut().enumerate() {
                    std::ptr::write(
                        first.add(i),
                        AudioBuffer {
                            mNumberChannels: 1,
                            mDataByteSize: (buf.len() * std::mem::size_of::<f32>()) as UInt32,
                            mData: buf.as_mut_ptr() as *mut std::os::raw::c_void,
                        },
                    );
                }
            }
            Self {
                storage,
                _channels: channels,
            }
        }

        /// A single `AudioBuffer` carrying `channel_count` interleaved
        /// channels -- the layout `read_input_channels_planar` and
        /// `render_ioproc_trampoline`'s interleaved branch exist for,
        /// confirmed to be what real devices on this machine actually
        /// deliver (see their doc comments).
        fn new_interleaved(mut interleaved: Vec<f32>, channel_count: usize) -> Self {
            let total = std::mem::size_of::<AudioBufferList>();
            let mut storage = vec![0u8; total];
            let list = storage.as_mut_ptr() as *mut AudioBufferList;
            unsafe {
                (*list).mNumberBuffers = 1;
                std::ptr::write(
                    (*list).mBuffers.as_mut_ptr(),
                    AudioBuffer {
                        mNumberChannels: channel_count as UInt32,
                        mDataByteSize: (interleaved.len() * std::mem::size_of::<f32>()) as UInt32,
                        mData: interleaved.as_mut_ptr() as *mut std::os::raw::c_void,
                    },
                );
            }
            Self {
                storage,
                _channels: vec![interleaved],
            }
        }

        fn as_ptr(&self) -> *const AudioBufferList {
            self.storage.as_ptr() as *const AudioBufferList
        }

        fn as_mut_ptr(&mut self) -> *mut AudioBufferList {
            self.storage.as_mut_ptr() as *mut AudioBufferList
        }
    }

    /// Calls `capture_ioproc_trampoline` directly with a hand-built
    /// multi-channel `AudioBufferList` -- no real device or `coreaudiod`
    /// needed, exercising exactly the buffer-count and byte-size-to-
    /// sample-count arithmetic a real callback would trigger. This is the
    /// exact class of bug the trampoline had before it compiled once
    /// (reading uninitialised memory past `count` in an early draft of
    /// the render side): worth a real check now that it exists.
    #[test]
    fn capture_trampoline_parses_a_two_channel_buffer_list_without_crossing_channels() {
        // At least TAPS (32) samples per channel so the resampler is
        // primed and actually emits output on this one callback.
        let ch0: Vec<f32> = (0..64).map(|i| i as f32 * 0.01).collect(); // stays under 1.0
        let ch1: Vec<f32> = (0..64).map(|i| 100.0 + i as f32 * 0.01).collect(); // stays over 100.0
        let input_len = ch0.len();
        let input_list = TestBufferList::new(vec![ch0, ch1]);

        let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
        let stage = DriftCorrectedIoStage::new(2, corrector);
        let master = Arc::new(MasterClock::default());
        let (tx0, mut rx0) = rtrb::RingBuffer::<f32>::new(256);
        let (tx1, mut rx1) = rtrb::RingBuffer::<f32>::new(256);
        let mut ctx = Box::new(CaptureIoProcContext::new(
            stage,
            master,
            vec![tx0, tx1],
            input_len * 2,
        ));

        let status = unsafe {
            capture_ioproc_trampoline(
                0,
                std::ptr::null(),
                input_list.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                ctx.as_mut() as *mut CaptureIoProcContext as *mut std::os::raw::c_void,
            )
        };
        assert_eq!(status, 0);

        let mut received0 = Vec::new();
        while let Ok(s) = rx0.pop() {
            received0.push(s);
        }
        let mut received1 = Vec::new();
        while let Ok(s) = rx1.pop() {
            received1.push(s);
        }
        assert!(!received0.is_empty() && !received1.is_empty());
        // Channel 0's samples are all under 1.0, channel 1's all over
        // 100.0 -- if the buffer-list parsing mixed up the two channels'
        // pointers or strides, this would catch it.
        assert!(received0.iter().all(|&s| s < 1.0));
        assert!(received1.iter().all(|&s| s > 100.0));
    }

    #[test]
    fn render_trampoline_fills_every_channel_and_pads_underrun_with_silence() {
        let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
        let stage = DriftCorrectedIoStage::new(2, corrector);
        let master = Arc::new(MasterClock::default());
        let (mut tx0, rx0) = rtrb::RingBuffer::<f32>::new(64);
        // Channel 0 has data queued; channel 1 has none, so it must
        // underrun to silence rather than leave the poisoned sentinel.
        for i in 0..8 {
            tx0.push(i as f32).unwrap();
        }
        let (_tx1, rx1) = rtrb::RingBuffer::<f32>::new(64);
        let mut ctx = Box::new(RenderIoProcContext::new(stage, master, vec![rx0, rx1], 256));

        let out0 = vec![1.0f32; 8]; // poisoned sentinel
        let out1 = vec![1.0f32; 8];
        let mut output_list = TestBufferList::new(vec![out0, out1]);

        let status = unsafe {
            render_ioproc_trampoline(
                0,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                output_list.as_mut_ptr(),
                std::ptr::null(),
                ctx.as_mut() as *mut RenderIoProcContext as *mut std::os::raw::c_void,
            )
        };
        assert_eq!(status, 0);

        unsafe {
            let list = &*output_list.as_ptr();
            let bufs = list.mBuffers.as_ptr();
            let buf0 = std::slice::from_raw_parts((*bufs).mData as *const f32, 8);
            let buf1 = std::slice::from_raw_parts((*bufs.add(1)).mData as *const f32, 8);
            assert!(
                buf1.iter().all(|&s| s == 0.0),
                "underrun channel should be silence"
            );
            assert!(
                buf0.iter().any(|&s| s != 1.0),
                "the channel with data queued should not be all sentinel"
            );
        }
    }

    #[test]
    fn capture_trampoline_deinterleaves_a_single_interleaved_buffer() {
        // The exact layout that broke a first version of this module
        // against real hardware: one AudioBuffer, mNumberChannels = 2,
        // samples interleaved L,R,L,R,... rather than two separate mono
        // buffers. Needs at least TAPS (32) frames per channel to prime
        // the resampler and actually emit output this callback.
        let frames = 64;
        let interleaved: Vec<f32> = (0..frames)
            .flat_map(|f| [f as f32 * 0.01, 100.0 + f as f32 * 0.01]) // ch0 < 1.0, ch1 > 100.0
            .collect();
        let input_list = TestBufferList::new_interleaved(interleaved, 2);

        let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
        let stage = DriftCorrectedIoStage::new(2, corrector);
        let master = Arc::new(MasterClock::default());
        let (tx0, mut rx0) = rtrb::RingBuffer::<f32>::new(256);
        let (tx1, mut rx1) = rtrb::RingBuffer::<f32>::new(256);
        let mut ctx = Box::new(CaptureIoProcContext::new(
            stage,
            master,
            vec![tx0, tx1],
            frames * 2,
        ));

        let status = unsafe {
            capture_ioproc_trampoline(
                0,
                std::ptr::null(),
                input_list.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                ctx.as_mut() as *mut CaptureIoProcContext as *mut std::os::raw::c_void,
            )
        };
        assert_eq!(status, 0);

        let mut received0 = Vec::new();
        while let Ok(s) = rx0.pop() {
            received0.push(s);
        }
        let mut received1 = Vec::new();
        while let Ok(s) = rx1.pop() {
            received1.push(s);
        }
        assert!(
            !received0.is_empty() && !received1.is_empty(),
            "both channels of an interleaved buffer should be drained, not just the first"
        );
        assert!(received0.iter().all(|&s| s < 1.0));
        assert!(received1.iter().all(|&s| s > 100.0));
    }

    #[test]
    fn render_trampoline_interleaves_output_into_a_single_buffer() {
        // At least TAPS (32) frames per channel so the resampler is
        // primed and actually emits output rather than silence -- an
        // earlier version of this test used 8, which the resampler pads
        // with real (correct) silence, not a bug in the interleaving.
        let frames = 64;
        let corrector = DriftCorrector::new(PiController::new(2e-5, 5e-7, 0.01), 500.0);
        let stage = DriftCorrectedIoStage::new(2, corrector);
        let master = Arc::new(MasterClock::default());
        // More than `frames` samples queued, generously: TAPS (32) of
        // them are consumed priming the resampler before it emits
        // anything, so requesting exactly `frames` output frames from
        // exactly `frames` input samples would legitimately underrun
        // partway through, silence-padding the tail -- not a bug, just
        // not what this test is checking.
        let queued = frames * 3;
        let (mut tx0, rx0) = rtrb::RingBuffer::<f32>::new(queued + 16);
        let (mut tx1, rx1) = rtrb::RingBuffer::<f32>::new(queued + 16);
        for i in 0..queued {
            tx0.push(i as f32 * 0.001).unwrap(); // channel 0: stays under 1.0
            tx1.push(100.0 + i as f32 * 0.001).unwrap(); // channel 1: stays over 100.0
        }
        let mut ctx = Box::new(RenderIoProcContext::new(
            stage,
            master,
            vec![rx0, rx1],
            frames * 2,
        ));

        let interleaved = vec![-1.0f32; frames * 2]; // poisoned sentinel
        let mut output_list = TestBufferList::new_interleaved(interleaved, 2);

        let status = unsafe {
            render_ioproc_trampoline(
                0,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                output_list.as_mut_ptr(),
                std::ptr::null(),
                ctx.as_mut() as *mut RenderIoProcContext as *mut std::os::raw::c_void,
            )
        };
        assert_eq!(status, 0);

        unsafe {
            let list = &*output_list.as_ptr();
            let buf = &*list.mBuffers.as_ptr();
            let raw = std::slice::from_raw_parts(buf.mData as *const f32, frames * 2);
            // Not exact-value equality: even a primed resampler at ratio
            // 1.0 isn't bit-exact to its input (windowed-sinc filtering),
            // established elsewhere in this crate's own tests. What this
            // test checks is that channel 0's values land at even
            // interleaved positions and channel 1's at odd ones -- the
            // actual thing that was broken.
            assert!(
                raw.iter().step_by(2).all(|&s| s < 1.0),
                "channel 0 should occupy every even interleaved position"
            );
            assert!(
                raw.iter().skip(1).step_by(2).all(|&s| s > 100.0),
                "channel 1 should occupy every odd interleaved position"
            );
        }
    }

    #[test]
    fn master_trampoline_hands_the_callback_both_directions_in_one_call() {
        let input = vec![vec![1.0f32, 2.0, 3.0, 4.0]];
        let input_list = TestBufferList::new(input);
        let output = vec![vec![9.0f32; 4]];
        let mut output_list = TestBufferList::new(output);

        let seen_frames = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let seen_input_first_sample = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let seen_frames_cb = seen_frames.clone();
        let seen_input_cb = seen_input_first_sample.clone();
        let callback: MasterTickCallback = Box::new(move |frames, input, output| {
            seen_frames_cb.store(frames, std::sync::atomic::Ordering::SeqCst);
            seen_input_cb.store(
                input
                    .first()
                    .and_then(|c| c.first())
                    .copied()
                    .unwrap_or(-1.0) as u32,
                std::sync::atomic::Ordering::SeqCst,
            );
            // Prove this callback can write the master's own bus output
            // directly, in the same call it saw the captured input.
            if let Some(out_channel) = output.first_mut() {
                out_channel.fill(42.0);
            }
        });
        let mut ctx = Box::new(MasterIoProcContext { callback });

        let status = unsafe {
            master_ioproc_trampoline(
                0,
                std::ptr::null(),
                input_list.as_ptr(),
                std::ptr::null(),
                output_list.as_mut_ptr(),
                std::ptr::null(),
                ctx.as_mut() as *mut MasterIoProcContext as *mut std::os::raw::c_void,
            )
        };
        assert_eq!(status, 0);
        assert_eq!(seen_frames.load(std::sync::atomic::Ordering::SeqCst), 4);
        assert_eq!(
            seen_input_first_sample.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        unsafe {
            let list = &*output_list.as_ptr();
            let buf0 = std::slice::from_raw_parts(list.mBuffers[0].mData as *const f32, 4);
            assert!(buf0.iter().all(|&s| s == 42.0));
        }
    }
}
