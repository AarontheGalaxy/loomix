/*
 * Prints how many currently-visible CoreAudio devices have a UID
 * starting with "com.loomix.audiodriver." (16 expected once the driver
 * is installed and coreaudiod has it loaded), then exits 0 if that count
 * is nonzero, 1 if it's zero. A driver crashing coreaudiod on load, or
 * failing to publish any device, otherwise shows up only as confusing
 * failures far downstream (an ffmpeg device-index lookup coming up
 * empty, a 30-second test timing out) -- this makes "zero devices" a
 * loud, immediate, unambiguous failure instead, meant to run first in
 * both driver/scripts/install.sh and driver/tests/loopback_test.sh.
 */
#include "timeout_guard.h"

#include <CoreAudio/CoreAudio.h>
#include <stdio.h>
#include <stdlib.h>

int main(void)
{
    ArmTimeout();

    AudioObjectPropertyAddress devicesAddress = {
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    UInt32 dataSize = 0;
    OSStatus status = AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &devicesAddress, 0, NULL, &dataSize);
    if (status != noErr)
    {
        fprintf(stderr, "failed to size the system device list (status %d)\n", (int)status);
        return 1;
    }

    UInt32 deviceCount = dataSize / sizeof(AudioObjectID);
    AudioObjectID *deviceIDs = (AudioObjectID *)malloc(dataSize);
    if (deviceIDs == NULL)
    {
        fprintf(stderr, "out of memory sizing a %u-device list\n", deviceCount);
        return 1;
    }

    status = AudioObjectGetPropertyData(kAudioObjectSystemObject, &devicesAddress, 0, NULL, &dataSize, deviceIDs);
    if (status != noErr)
    {
        fprintf(stderr, "failed to read the system device list (status %d)\n", (int)status);
        free(deviceIDs);
        return 1;
    }

    UInt32 loomixCount = 0;
    for (UInt32 i = 0; i < deviceCount; i++)
    {
        AudioObjectPropertyAddress uidAddress = {
            kAudioDevicePropertyDeviceUID,
            kAudioObjectPropertyScopeGlobal,
            kAudioObjectPropertyElementMain,
        };
        CFStringRef uid = NULL;
        UInt32 uidSize = sizeof(uid);
        if (AudioObjectGetPropertyData(deviceIDs[i], &uidAddress, 0, NULL, &uidSize, &uid) != noErr || uid == NULL)
        {
            continue;
        }
        CFStringRef prefix = CFSTR("com.loomix.audiodriver.");
        CFRange found;
        if (CFStringFindWithOptions(uid, prefix, CFRangeMake(0, CFStringGetLength(uid)), kCFCompareAnchored, &found))
        {
            loomixCount++;
        }
        CFRelease(uid);
    }

    free(deviceIDs);
    printf("%u\n", loomixCount);
    return loomixCount == 0 ? 1 : 0;
}
