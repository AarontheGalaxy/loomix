/*
 * Sets the installed Loomix In 1 device's nominal sample rate, exercising
 * the SetPropertyData -> RequestDeviceConfigurationChange ->
 * PerformDeviceConfigurationChange path in LoomixAudioDriver.c (spec
 * section 1.11: 44.1 through 192 kHz). Usage: set_sample_rate <hz>
 */
#include <CoreAudio/CoreAudio.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, const char *argv[])
{
    if (argc != 2)
    {
        fprintf(stderr, "usage: %s <sample-rate-hz>\n", argv[0]);
        return 1;
    }
    Float64 rate = atof(argv[1]);

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

    AudioObjectPropertyAddress rateAddress = {
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    status = AudioObjectSetPropertyData(deviceID, &rateAddress, 0, NULL, sizeof(rate), &rate);
    if (status != noErr)
    {
        fprintf(stderr, "failed to set sample rate to %.0f (status %d)\n", rate, (int)status);
        return 1;
    }

    return 0;
}
