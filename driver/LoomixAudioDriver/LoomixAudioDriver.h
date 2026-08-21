#ifndef LOOMIX_AUDIO_DRIVER_H
#define LOOMIX_AUDIO_DRIVER_H

#include <CoreAudio/AudioServerPlugIn.h>
#include <pthread.h>
#include <stdatomic.h>

#include "RingBuffer.h"

/*
 * M2 object model: one plug-in, kLoomixDeviceCount devices, each a
 * self-contained loopback pair (spec section 1.1: 8 "Loomix In" playback
 * devices, plus 8 "Loomix Out" capture devices -- A1-A5 and B1-B3 -- that
 * stay loopback pairs like the "In" devices until M3's engine exists to
 * feed them from an actual bus mix instead). kAudioObjectPlugInObject
 * must be 1 per <CoreAudio/AudioServerPlugIn.h>; every other object ID is
 * this driver's own choice, computed from a device's index so the whole
 * table doesn't need to be hand-enumerated.
 */
#define kObjectID_PlugIn kAudioObjectPlugInObject
#define kLoomixDeviceCount 16
#define kLoomixObjectsPerDevice 3 /* device, input stream, output stream */
#define kLoomixFirstDeviceObjectID 2

#define kLoomixDeviceObjectID(deviceIndex) (kLoomixFirstDeviceObjectID + (AudioObjectID)(deviceIndex) * kLoomixObjectsPerDevice)
#define kLoomixStreamInputObjectID(deviceIndex) (kLoomixDeviceObjectID(deviceIndex) + 1)
#define kLoomixStreamOutputObjectID(deviceIndex) (kLoomixDeviceObjectID(deviceIndex) + 2)

/* Spec section 1.11: the engine's supported nominal sample rates. */
static const Float64 kLoomixSupportedSampleRates[] = {44100.0, 48000.0, 88200.0, 96000.0, 176400.0, 192000.0};
#define kLoomixSupportedSampleRateCount (sizeof(kLoomixSupportedSampleRates) / sizeof(kLoomixSupportedSampleRates[0]))

#define kLoomixDefaultSampleRate 48000.0
#define kLoomixDefaultChannels 2

/* Frames of head start WriteMix is guaranteed over ReadInput each cycle;
 * see the kAudioDevicePropertySafetyOffset case in LoomixAudioDriver.c. */
#define kLoomixInputSafetyOffsetFrames ((UInt32)512)

/* M2's "control channel so the app can query buffer statistics" (spec
 * section 3.4): a custom AudioObject property on each device, returning a
 * CFPropertyList of the ring buffer's own counters. Registered through
 * kAudioObjectPropertyCustomPropertyInfoList, which an M1 attempt at this
 * skipped -- CoreAudio's host only forwards a *custom* selector (as
 * opposed to one of its own) to the plugin if it's been declared there
 * first, and per <CoreAudio/AudioServerPlugIn.h> its data has to be
 * marshaled as a CFString or CFPropertyList, not the raw UInt32 the
 * earlier attempt returned; both gaps are why every read of it failed
 * with kAudioHardwareUnknownPropertyError (see docs/ARCHITECTURE.md). */
#define kLoomixCustomProperty_BufferStatistics 'stat'

/* Spec section 1.1: "Loomix In 1".."Loomix In 8" feed the mixer's hardware
 * strips; "Loomix Out A1".."Out B3" carry the bus outputs. Names and UIDs
 * (stable UIDs are an explicit M2 requirement) are static CFStringRef
 * tables in LoomixAudioDriver.c, indexed the same way. */

typedef struct
{
    Float64 mSampleRate;
    UInt32 mChannelCount;

    /* Configurable driver-side latency (spec 3.4 M2), reported as this
     * device's input-scope SafetyOffset: extra frames of margin ReadInput
     * is guaranteed over WriteMix, on top of kLoomixInputSafetyOffsetFrames.
     * Spec 1.19 wants larger values to trade latency for safety under
     * heavier system load; see kAudioDevicePropertySafetyOffset. */
    UInt32 mExtraLatencyFrames;

    /* Synthesized clock: derived from mach_absolute_time() elapsed since
     * StartIO, since this device has no hardware clock of its own. */
    UInt64 mStartHostTime;
    Boolean mIsRunning;
    UInt32 mIORunningClients;

    /* The actual read/write logic, and its correctness, lives in
     * RingBuffer.c/.h -- plain C, no CoreAudio, unit tested directly by
     * driver/tests/test_ring_buffer.c. This struct only owns the storage
     * and hands sample times through to it.
     *
     * <CoreAudio/AudioServerPlugIn.h> documents kAudioServerPlugInIOOperationThread
     * as marking the beginning and ending of "the IO thread" (singular)
     * *per device* -- so two different devices' IO can genuinely run on
     * different threads at once, but every DoIOOperation call for one
     * device's streams, across every attached client, is serialized on
     * that one device's thread for its entire StartIO..StopIO lifetime.
     * That would make a plain read/write of a ring buffer's cross-call
     * fields safe without a lock. It isn't independently verifiable from
     * outside coreaudiod, so those fields are atomic regardless; see
     * RingBuffer.h. Devices never share a ring buffer with each other,
     * so cross-device concurrency needs no synchronization here either. */
    float *mRingStorage;
    LoomixRingBuffer mRing;
} LoomixDevice;

typedef struct
{
    AudioServerPlugInDriverRef mDriverRef;
    AudioServerPlugInHostRef mHost;
    pthread_mutex_t mStateMutex;
    LoomixDevice mDevices[kLoomixDeviceCount];
} LoomixDriverState;

#endif /* LOOMIX_AUDIO_DRIVER_H */
