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

fn check(status: OSStatus) -> Result<(), CoreAudioError> {
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
        }
    }
}

/// Reads up to [`MAX_IO_CHANNELS`] channels from an `AudioBufferList` as
/// planar `&[f32]` slices. Returns the array and the real channel count
/// (`<= MAX_IO_CHANNELS`); channels beyond that are silently dropped
/// rather than read out of bounds. Shared by every trampoline in this
/// file that needs a device's captured audio.
///
/// # Safety
/// `list` must point to a valid `AudioBufferList` with `mNumberBuffers`
/// non-interleaved buffers, each `mData` valid for `mDataByteSize` bytes,
/// for the duration of the borrow -- exactly what CoreAudio guarantees for
/// the buffer list handed to an `AudioDeviceIOProc` for the callback's
/// duration.
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

/// The write side of [`read_input_channels`]: up to [`MAX_IO_CHANNELS`]
/// channels from an `AudioBufferList` as planar `&mut [f32]` slices, built
/// without `MaybeUninit` (see the render trampoline's own note, kept here
/// since this replaced its inline version) -- each of the fixed slots gets
/// a genuinely valid value, a disjoint sub-slice for `i < count` or an
/// always-valid empty slice otherwise, never an uninitialised read for a
/// device with fewer than `MAX_IO_CHANNELS` channels.
///
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
/// `outputs`/`scratch` were sized once at construction. No locks:
/// `rtrb::Producer::push` is lock-free. No syscalls beyond what CoreAudio
/// itself performs to invoke this callback.
unsafe extern "C" fn capture_ioproc_trampoline(
    _device: AudioObjectID,
    _now: *const AudioTimeStamp,
    input_data: *const AudioBufferList,
    _input_time: *const AudioTimeStamp,
    _output_data: *mut AudioBufferList,
    _output_time: *const AudioTimeStamp,
    client_data: *mut std::os::raw::c_void,
) -> OSStatus {
    let ctx = unsafe { &mut *(client_data as *mut CaptureIoProcContext) };
    let (channels, count) = unsafe { read_input_channels(input_data) };
    ctx.stage.on_capture(
        &channels[..count],
        &ctx.master,
        &mut ctx.outputs,
        &mut ctx.scratch,
    );
    0
}

/// A running capture IOProc registration. Stops and unregisters on drop.
pub struct CaptureIoProcHandle {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    _ctx: Box<CaptureIoProcContext>,
}

impl CaptureIoProcHandle {
    pub fn start(device: DeviceId, ctx: CaptureIoProcContext) -> Result<Self, CoreAudioError> {
        let mut ctx = Box::new(ctx);
        let mut proc_id: AudioDeviceIOProcID = None;
        check(unsafe {
            AudioDeviceCreateIOProcID(
                device,
                Some(capture_ioproc_trampoline),
                ctx.as_mut() as *mut CaptureIoProcContext as *mut _,
                &mut proc_id,
            )
        })?;
        if let Err(e) = check(unsafe { AudioDeviceStart(device, proc_id) }) {
            unsafe { AudioDeviceDestroyIOProcID(device, proc_id) };
            return Err(e);
        }
        Ok(Self {
            device,
            proc_id,
            _ctx: ctx,
        })
    }
}

impl Drop for CaptureIoProcHandle {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.device, self.proc_id);
            AudioDeviceDestroyIOProcID(self.device, self.proc_id);
        }
    }
}

/// Per-device state for a registered render IOProc, symmetric to
/// [`CaptureIoProcContext`]: the ring buffers are consumers (filled by
/// whoever assembles bus output) instead of producers.
pub struct RenderIoProcContext {
    stage: DriftCorrectedIoStage,
    master: Arc<MasterClock>,
    inputs: Vec<rtrb::Consumer<f32>>,
    scratch: Vec<f32>,
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
        }
    }
}

/// # Real-time safety
/// Same reasoning as [`capture_ioproc_trampoline`]: `channels` is a fixed
/// stack array, `ctx`'s state is pre-allocated, `rtrb::Consumer::pop` is
/// lock-free.
unsafe extern "C" fn render_ioproc_trampoline(
    _device: AudioObjectID,
    _now: *const AudioTimeStamp,
    _input_data: *const AudioBufferList,
    _input_time: *const AudioTimeStamp,
    output_data: *mut AudioBufferList,
    _output_time: *const AudioTimeStamp,
    client_data: *mut std::os::raw::c_void,
) -> OSStatus {
    let ctx = unsafe { &mut *(client_data as *mut RenderIoProcContext) };
    let (mut channels, count) = unsafe { write_output_channels(output_data) };
    ctx.stage.on_render(
        &ctx.master,
        &mut ctx.inputs,
        &mut channels[..count],
        &mut ctx.scratch,
    );
    0
}

/// A running render IOProc registration. Stops and unregisters on drop.
pub struct RenderIoProcHandle {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    _ctx: Box<RenderIoProcContext>,
}

impl RenderIoProcHandle {
    pub fn start(device: DeviceId, ctx: RenderIoProcContext) -> Result<Self, CoreAudioError> {
        let mut ctx = Box::new(ctx);
        let mut proc_id: AudioDeviceIOProcID = None;
        check(unsafe {
            AudioDeviceCreateIOProcID(
                device,
                Some(render_ioproc_trampoline),
                ctx.as_mut() as *mut RenderIoProcContext as *mut _,
                &mut proc_id,
            )
        })?;
        if let Err(e) = check(unsafe { AudioDeviceStart(device, proc_id) }) {
            unsafe { AudioDeviceDestroyIOProcID(device, proc_id) };
            return Err(e);
        }
        Ok(Self {
            device,
            proc_id,
            _ctx: ctx,
        })
    }
}

impl Drop for RenderIoProcHandle {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.device, self.proc_id);
            AudioDeviceDestroyIOProcID(self.device, self.proc_id);
        }
    }
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

struct MasterIoProcContext {
    callback: MasterTickCallback,
}

unsafe extern "C" fn master_ioproc_trampoline(
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

/// A running master-device IOProc registration. Stops and unregisters on
/// drop.
pub struct MasterIoProcHandle {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    _ctx: Box<MasterIoProcContext>,
}

impl MasterIoProcHandle {
    pub fn start(device: DeviceId, callback: MasterTickCallback) -> Result<Self, CoreAudioError> {
        let mut ctx = Box::new(MasterIoProcContext { callback });
        let mut proc_id: AudioDeviceIOProcID = None;
        check(unsafe {
            AudioDeviceCreateIOProcID(
                device,
                Some(master_ioproc_trampoline),
                ctx.as_mut() as *mut MasterIoProcContext as *mut _,
                &mut proc_id,
            )
        })?;
        if let Err(e) = check(unsafe { AudioDeviceStart(device, proc_id) }) {
            unsafe { AudioDeviceDestroyIOProcID(device, proc_id) };
            return Err(e);
        }
        Ok(Self {
            device,
            proc_id,
            _ctx: ctx,
        })
    }
}

impl Drop for MasterIoProcHandle {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.device, self.proc_id);
            AudioDeviceDestroyIOProcID(self.device, self.proc_id);
        }
    }
}

/// A CFString built from a dynamic, caller-supplied string. Not real-time
/// code (device creation is a one-off control-plane call, never per
/// block), so allocating here is fine.
fn cfstring_from_str(s: &str) -> CFStringRef {
    let c = std::ffi::CString::new(s).expect("CoreAudio strings must not contain a NUL byte");
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), kCFStringEncodingUTF8) }
}

/// A CFString built from one of `coreaudio-sys`'s dictionary-key
/// constants (`&[u8; N]`, already NUL-terminated C strings, e.g.
/// `kAudioAggregateDeviceUIDKey`) rather than a dynamic string -- no
/// `CString` round trip needed, the bytes are already in the right shape.
fn cfstring_from_key(key: &[u8]) -> CFStringRef {
    unsafe {
        CFStringCreateWithCString(
            std::ptr::null(),
            key.as_ptr() as *const std::os::raw::c_char,
            kCFStringEncodingUTF8,
        )
    }
}

/// Creates a CoreAudio aggregate device spanning `sub_device_uids` under
/// one clock (spec 2.3's alternative to `drift.rs`'s own correction --
/// "let the user choose" -- and spec 2.2's ASIO-multichannel-routing
/// mapping). `master_uid` picks which sub-device's clock the aggregate
/// follows and must be one of `sub_device_uids`; that invariant is the
/// caller's to uphold; CoreAudio's own error is what surfaces if it isn't.
/// Private by default (spec's own aggregate devices are Loomix-internal
/// plumbing, not something every app's device picker needs to show),
/// unless `visible_in_ui` is set.
pub fn create_aggregate_device(
    name: &str,
    uid: &str,
    sub_device_uids: &[&str],
    master_uid: &str,
    visible_in_ui: bool,
) -> Result<DeviceId, CoreAudioError> {
    unsafe {
        let name_key = cfstring_from_key(kAudioAggregateDeviceNameKey);
        let name_value = cfstring_from_str(name);
        let uid_key = cfstring_from_key(kAudioAggregateDeviceUIDKey);
        let uid_value = cfstring_from_str(uid);
        let private_key = cfstring_from_key(kAudioAggregateDeviceIsPrivateKey);
        let private_value = if visible_in_ui {
            kCFBooleanFalse
        } else {
            kCFBooleanTrue
        };
        let master_key = cfstring_from_key(kAudioAggregateDeviceMasterSubDeviceKey);
        let master_value = cfstring_from_str(master_uid);
        let sub_device_list_key = cfstring_from_key(kAudioAggregateDeviceSubDeviceListKey);
        let sub_device_uid_key = cfstring_from_key(kAudioSubDeviceUIDKey);

        let sub_device_dicts: Vec<CFDictionaryRef> = sub_device_uids
            .iter()
            .map(|sub_uid| {
                let value = cfstring_from_str(sub_uid);
                let mut keys: [*const std::os::raw::c_void; 1] = [sub_device_uid_key as *const _];
                let mut values: [*const std::os::raw::c_void; 1] = [value as *const _];
                let dict = CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_mut_ptr(),
                    values.as_mut_ptr(),
                    1,
                    &kCFTypeDictionaryKeyCallBacks,
                    &kCFTypeDictionaryValueCallBacks,
                );
                CFRelease(value as CFTypeRef);
                dict
            })
            .collect();
        let mut sub_device_dict_ptrs: Vec<*const std::os::raw::c_void> = sub_device_dicts
            .iter()
            .map(|&d| d as *const std::os::raw::c_void)
            .collect();
        let sub_device_array = CFArrayCreate(
            std::ptr::null(),
            sub_device_dict_ptrs.as_mut_ptr(),
            sub_device_dict_ptrs.len() as CFIndex,
            &kCFTypeArrayCallBacks,
        );
        for &d in &sub_device_dicts {
            CFRelease(d as CFTypeRef);
        }

        let mut keys: [*const std::os::raw::c_void; 5] = [
            name_key as *const _,
            uid_key as *const _,
            private_key as *const _,
            master_key as *const _,
            sub_device_list_key as *const _,
        ];
        let mut values: [*const std::os::raw::c_void; 5] = [
            name_value as *const _,
            uid_value as *const _,
            private_value as *const _,
            master_value as *const _,
            sub_device_array as *const _,
        ];
        let description = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_mut_ptr(),
            values.as_mut_ptr(),
            5,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );

        let mut device_id: AudioObjectID = 0;
        let status = AudioHardwareCreateAggregateDevice(description, &mut device_id);

        for key in [
            name_key,
            uid_key,
            private_key,
            master_key,
            sub_device_list_key,
            sub_device_uid_key,
        ] {
            CFRelease(key as CFTypeRef);
        }
        for value in [name_value, uid_value, master_value] {
            CFRelease(value as CFTypeRef);
        }
        CFRelease(sub_device_array as CFTypeRef);
        CFRelease(description as CFTypeRef);

        check(status)?;
        Ok(device_id)
    }
}

/// Destroys an aggregate device created by [`create_aggregate_device`].
pub fn destroy_aggregate_device(id: DeviceId) -> Result<(), CoreAudioError> {
    check(unsafe { AudioHardwareDestroyAggregateDevice(id) })
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
    #[ignore = "creates a real system aggregate device; run manually, not in CI"]
    fn create_and_destroy_an_aggregate_device_round_trip() {
        let id = default_output_device().expect("a default output device should exist");
        let uid = device_uid(id).expect("uid query should succeed");
        let aggregate_id = create_aggregate_device(
            "Loomix test aggregate",
            "com.loomix.test-aggregate",
            &[&uid],
            &uid,
            false,
        )
        .expect("aggregate device creation should succeed");
        destroy_aggregate_device(aggregate_id).expect("aggregate device teardown should succeed");
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
