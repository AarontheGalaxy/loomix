#ifndef LOOMIX_RING_BUFFER_H
#define LOOMIX_RING_BUFFER_H

/* No CoreAudio, no CoreFoundation -- this is the one piece of the driver
 * that's actually load-bearing for correctness, so it's kept plain,
 * self-contained C, testable with nothing but a compiler and libc
 * (driver/tests/test_ring_buffer.c). Everything CoreAudio-specific lives
 * in LoomixAudioDriver.c, which calls this module rather than duplicating
 * its logic. */

#include <stdatomic.h>
#include <stdint.h>

/* Spec section 1.1: hardware strips run 2 to 8 channels. */
#define kLoomixMinChannels 2
#define kLoomixMaxChannels 8

/* Sized generously above spec section 1.19's "at least 3x the engine
 * buffer size" (3 x 2048 = 6144 frames, the largest supported buffer) and
 * well above the 512-frame safety offset proven sufficient in the M1
 * loopback test, without being sized for the *seconds* a device might
 * run -- with 16 devices, most idle at once (M2), a per-device multiplier
 * that big is wasted memory on devices nothing is using. Allocated once,
 * lazily, by the caller on first use (not for every device at driver
 * load), and never resized on the audio thread. */
#define kLoomixRingBufferFrameCapacity ((uint32_t)65536u)

typedef struct
{
    /* Caller-owned storage, kLoomixRingBufferFrameCapacity *
     * kLoomixMaxChannels floats, set once by LoomixRingBuffer_Init. Every
     * frame slot reserves room for the maximum channel count regardless
     * of the current channel count, so the layout never changes shape
     * when the channel count does. */
    float *samples;
    uint32_t channelCount;

    /* Highest sample time Write has written through. Read compares
     * against this to tell a written frame from one it would otherwise
     * have to serve stale (a prior lap's, or never written at all).
     * Written by Write, read by Read; see the threading note on this
     * same field in LoomixAudioDriver.h for why it's atomic. */
    _Atomic uint64_t writeCursorSampleTime;

    /* Contiguity bookkeeping: each of Write/Read compares this call's
     * start against the previous call's end and bumps the matching
     * counter on a mismatch. The *End fields are private to their own
     * function; the counters are meant to be read from another thread
     * after a session, so they're atomic. */
    double lastWriteEndSampleTime; /* -1 = no write yet this session */
    double lastReadEndSampleTime;  /* -1 = no read yet this session */
    _Atomic uint64_t writeDiscontinuityCount;
    _Atomic uint64_t readDiscontinuityCount;
} LoomixRingBuffer;

/* `storage` must point to kLoomixRingBufferFrameCapacity * kLoomixMaxChannels
 * floats, valid for the ring buffer's lifetime; ownership stays with the
 * caller. Does not zero `storage` -- callers that rely on unwritten
 * regions reading as silence rely on LoomixRingBuffer_Read's write-cursor
 * check, not on the storage starting zeroed (see test_ring_buffer.c). */
void LoomixRingBuffer_Init(LoomixRingBuffer *rb, float *storage, uint32_t channelCount);

/* Resets the write cursor and contiguity counters to a fresh session's
 * starting state, e.g. when IO restarts after being fully stopped. Does
 * not touch `storage` or `channelCount`. */
void LoomixRingBuffer_Reset(LoomixRingBuffer *rb);

/* Changes the active channel count, e.g. after a configuration change.
 * Callers must ensure no concurrent Write/Read is in flight (the driver
 * only calls this while the host has IO stopped). */
void LoomixRingBuffer_SetChannelCount(LoomixRingBuffer *rb, uint32_t channelCount);

/* Copies `inFrameCount` frames of `rb->channelCount` channels from
 * `inFrames` into the ring at the slot `inSampleTime` maps to, wrapping
 * at capacity, and advances the write high-water mark. Never allocates,
 * locks or does I/O. */
void LoomixRingBuffer_Write(LoomixRingBuffer *rb, double inSampleTime, uint32_t inFrameCount, const float *inFrames);

/* Copies `inFrameCount` frames into `outFrames` from the ring at the slot
 * `inSampleTime` maps to. Any requested frame at or past the write
 * high-water mark is silence, not whatever the ring's storage happens to
 * hold there. Never allocates, locks or does I/O. */
void LoomixRingBuffer_Read(LoomixRingBuffer *rb, double inSampleTime, uint32_t inFrameCount, float *outFrames);

#endif /* LOOMIX_RING_BUFFER_H */
