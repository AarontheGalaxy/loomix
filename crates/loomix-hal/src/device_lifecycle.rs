//! The part of `device.rs` that actually starts, stops or creates real
//! CoreAudio state: registering an IOProc (`AudioDeviceCreateIOProcID` +
//! `AudioDeviceStart`, torn down again by `AudioDeviceStop` +
//! `AudioDeviceDestroyIOProcID` on drop), requesting a stream format, and
//! creating/destroying an aggregate device. Split out from `device.rs`
//! deliberately, not merely for size: every function here is either never
//! exercised by the automated test suite at all (starting real device I/O
//! or creating a real system aggregate device isn't something to do
//! unattended in CI, the same reasoning `hog::decide`'s round-trip test is
//! `#[ignore]`d for) or wraps a real CoreAudio call whose *error* arm can
//! only be hit by an actual hardware failure, not something a test can
//! force deterministically. `device.rs`'s own trampoline functions
//! (`capture_ioproc_trampoline` and friends) and enumeration queries stay
//! there because they *are* exercised, directly, by hand-built
//! `AudioBufferList`s and real (successful) enumeration calls -- see that
//! file's test module.
//!
//! Coverage: excluded from the `cargo llvm-cov` gate via
//! `--ignore-filename-regex` in `justfile`/`ci.yml`, with this doc comment
//! as the explicit reason. Splitting this out of `device.rs` rather than
//! excluding that whole (partly well-tested) file is what makes the
//! exclusion specific to code that's actually untestable, not a blanket
//! carve-out for everything CoreAudio-adjacent.

use crate::clock::DeviceId;
use crate::device::{
    capture_ioproc_trampoline, check, master_ioproc_trampoline, render_ioproc_trampoline,
    CaptureIoProcContext, CoreAudioError, Direction, MasterIoProcContext, MasterTickCallback,
    RenderIoProcContext,
};
use coreaudio_sys::*;

/// *Requests* a non-interleaved, 32-bit float stream format on
/// `direction` -- best-effort, not load-bearing. `CaptureIoProcHandle`/
/// `RenderIoProcHandle::start` call this and ignore whether it succeeds:
/// confirmed against a real device on this machine that
/// `AudioObjectSetPropertyData` can report success here and the format
/// still comes back interleaved on readback, silently. Some devices
/// genuinely don't support non-interleaved at all; asking anyway costs
/// nothing on the ones that do honour it. The actual fix for a device
/// that stays interleaved either way is `device::read_input_channels_planar`
/// (and the equivalent inlined into `render_ioproc_trampoline`) adapting
/// to whichever layout the callback actually receives, not this function.
///
/// The device's own sample rate is preserved -- read from its current
/// format and left alone -- since that's the hardware's to set, not a
/// client's; only the interleaving and channel count are requested here.
pub fn set_stream_format_non_interleaved(
    id: DeviceId,
    direction: Direction,
    channel_count: usize,
) -> Result<(), CoreAudioError> {
    let scope = match direction {
        Direction::Input => kAudioDevicePropertyScopeInput,
        Direction::Output => kAudioDevicePropertyScopeOutput,
    };
    let addr = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyStreamFormat,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut format: AudioStreamBasicDescription = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
    check(unsafe {
        AudioObjectGetPropertyData(
            id,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut format as *mut _ as *mut _,
        )
    })?;
    format.mFormatID = kAudioFormatLinearPCM;
    format.mFormatFlags =
        kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved;
    format.mChannelsPerFrame = channel_count as u32;
    format.mBitsPerChannel = 32;
    // Non-interleaved: each AudioBuffer carries exactly one channel, so a
    // "frame" within any single buffer is one 4-byte float.
    format.mBytesPerFrame = 4;
    format.mFramesPerPacket = 1;
    format.mBytesPerPacket = 4;
    check(unsafe {
        AudioObjectSetPropertyData(
            id,
            &addr,
            0,
            std::ptr::null(),
            std::mem::size_of::<AudioStreamBasicDescription>() as u32,
            &format as *const _ as *const _,
        )
    })
}

/// A running capture IOProc registration. Stops and unregisters on drop.
pub struct CaptureIoProcHandle {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    _ctx: Box<CaptureIoProcContext>,
}

impl CaptureIoProcHandle {
    pub fn start(device: DeviceId, ctx: CaptureIoProcContext) -> Result<Self, CoreAudioError> {
        // Best-effort; ignored either way (see the function's doc comment).
        let _ = set_stream_format_non_interleaved(device, Direction::Input, ctx.channel_count());
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

/// A running render IOProc registration. Stops and unregisters on drop.
pub struct RenderIoProcHandle {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    _ctx: Box<RenderIoProcContext>,
}

impl RenderIoProcHandle {
    pub fn start(device: DeviceId, ctx: RenderIoProcContext) -> Result<Self, CoreAudioError> {
        // Best-effort; ignored either way (see the function's doc comment).
        let _ = set_stream_format_non_interleaved(device, Direction::Output, ctx.channel_count());
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

/// A running master-device IOProc registration. Stops and unregisters on
/// drop.
pub struct MasterIoProcHandle {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    _ctx: Box<MasterIoProcContext>,
}

impl MasterIoProcHandle {
    pub fn start(device: DeviceId, callback: MasterTickCallback) -> Result<Self, CoreAudioError> {
        let mut ctx = Box::new(MasterIoProcContext::new(callback));
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
    use crate::device::{default_output_device, device_uid};

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
}
