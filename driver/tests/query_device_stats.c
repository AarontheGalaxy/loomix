/*
 * Prints two space-separated values read from the installed Loomix In 1
 * device, as a CoreAudio client would see them: input-scope SafetyOffset,
 * then the current nominal sample rate.
 *
 * SafetyOffset catches the M1 bug where it was 0: with no scheduling
 * margin between WriteMix and ReadInput, the host could read a ring-buffer
 * slot before that cycle's write reached it. The sample rate lets a test
 * confirm a live SetPropertyData -> PerformDeviceConfigurationChange
 * reconfiguration actually took effect.
 *
 * This tool intentionally does not expose the ring buffer's internal
 * write/read discontinuity counters -- an earlier version tried to via
 * custom AudioObject properties, but CoreAudio's host only forwards a
 * custom property to the plugin if it's been declared through
 * kAudioObjectPropertyCustomPropertyInfoList first, which wasn't
 * implemented, so every read of them failed with
 * kAudioHardwareUnknownPropertyError. Rather than build out that
 * machinery for a value nothing outside the driver needs, the
 * authoritative check on those counters is
 * driver/tests/test_ring_buffer.c, which reads them directly with no
 * CoreAudio involved at all. See docs/ARCHITECTURE.md.
 */
#include "timeout_guard.h"

#include <CoreAudio/CoreAudio.h>
#include <stdio.h>

int main(void)
{
    ArmTimeout();

    CFStringRef uid = CFSTR("com.loomix.audiodriver.in1");
    AudioObjectPropertyAddress translateAddress = {
        kAudioHardwarePropertyTranslateUIDToDevice,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    AudioObjectID deviceID = kAudioObjectUnknown;
    UInt32 size = sizeof(deviceID);
    OSStatus status = AudioObjectGetPropertyData(kAudioObjectSystemObject, &translateAddress, sizeof(uid), &uid, &size, &deviceID);
    if (status != noErr || deviceID == kAudioObjectUnknown)
    {
        fprintf(stderr, "Loomix In 1 not found (status %d) -- is the driver installed?\n", (int)status);
        return 1;
    }

    AudioObjectPropertyAddress safetyAddress = {
        kAudioDevicePropertySafetyOffset,
        kAudioObjectPropertyScopeInput,
        kAudioObjectPropertyElementMain,
    };
    UInt32 safetyOffset = 0;
    size = sizeof(safetyOffset);
    if (AudioObjectGetPropertyData(deviceID, &safetyAddress, 0, NULL, &size, &safetyOffset) != noErr)
    {
        fprintf(stderr, "failed to read SafetyOffset\n");
        return 1;
    }

    AudioObjectPropertyAddress rateAddress = {
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    Float64 sampleRate = 0;
    size = sizeof(sampleRate);
    if (AudioObjectGetPropertyData(deviceID, &rateAddress, 0, NULL, &size, &sampleRate) != noErr)
    {
        fprintf(stderr, "failed to read nominal sample rate\n");
        return 1;
    }

    printf("%u %.0f\n", (unsigned int)safetyOffset, sampleRate);
    return 0;
}
