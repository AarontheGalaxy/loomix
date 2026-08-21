#ifndef LOOMIX_AUDIO_DRIVER_H
#define LOOMIX_AUDIO_DRIVER_H

#include <CoreAudio/AudioServerPlugIn.h>
#include <pthread.h>
#include <stdatomic.h>

#include "RingBuffer.h"

/*
 * M1 object model: one plug-in, one device ("Loomix In 1"), one input
 * stream (what a capture client reads) and one output stream (what a
 * playback client writes). kAudioObjectPlugInObject must be 1 per
 * <CoreAudio/AudioServerPlugIn.h>; the rest are this driver's own choice.
 */
enum
{
    kObjectID_PlugIn = kAudioObjectPlugInObject,
    kObjectID_Device = 2,
    kObjectID_Stream_Input = 3,
    kObjectID_Stream_Output = 4
};

/* Spec section 1.11: the engine's supported nominal sample rates. */
static const Float64 kLoomixSupportedSampleRates[] = {44100.0, 48000.0, 88200.0, 96000.0, 176400.0, 192000.0};
#define kLoomixSupportedSampleRateCount (sizeof(kLoomixSupportedSampleRates) / sizeof(kLoomixSupportedSampleRates[0]))

#define kLoomixDefaultSampleRate 48000.0
#define kLoomixDefaultChannels 2

/* Frames of head start WriteMix is guaranteed over ReadInput each cycle;
 * see the kAudioDevicePropertySafetyOffset case in LoomixAudioDriver.c. */
#define kLoomixInputSafetyOffsetFrames ((UInt32)512)

typedef struct
{
    AudioServerPlugInDriverRef mDriverRef;
    AudioServerPlugInHostRef mHost;
    pthread_mutex_t mStateMutex;

    Float64 mSampleRate;
    UInt32 mChannelCount;

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
     * as marking the beginning and ending of "the IO thread" (singular) for
     * a device, via BeginIOOperation/EndIOOperation -- meaning every
     * DoIOOperation call for this device's streams, across every attached
     * client, is serialized on one real-time thread for the device's
     * entire StartIO..StopIO lifetime. That would make the ring buffer's
     * cross-call fields safe without a lock even though only one thread
     * ever touches them. It isn't independently verifiable from outside
     * coreaudiod (no thread sanitizer was attached to confirm it under
     * this driver's two-client -- one playback, one capture -- scenario),
     * so the fields shared between Write and Read are atomic regardless;
     * see RingBuffer.h. */
    float *mRingStorage;
    LoomixRingBuffer mRing;
} LoomixDriverState;

#endif /* LOOMIX_AUDIO_DRIVER_H */
