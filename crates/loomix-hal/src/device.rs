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
use coreaudio_sys::*;
use std::sync::atomic::{AtomicBool, Ordering};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
