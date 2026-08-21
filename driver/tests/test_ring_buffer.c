/*
 * Deterministic unit test for RingBuffer.c -- no CoreAudio, no installed
 * driver, runs on every push. This is the load-bearing M1 correctness
 * test; driver/tests/loopback_test.sh is a manual smoke test on top of
 * it, since that one needs the driver actually installed and can only
 * observe whatever the OS's client-delivery path lets through (see the
 * comment at the top of that script).
 *
 * Build and run:
 *   clang -Wall -Wextra -Werror -std=gnu17 \
 *       -o test_ring_buffer test_ring_buffer.c ../LoomixAudioDriver/RingBuffer.c
 *   ./test_ring_buffer
 */
#include "../LoomixAudioDriver/RingBuffer.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

#define kTestChannels 2
#define kStorageFloats ((size_t)kLoomixRingBufferFrameCapacity * kLoomixMaxChannels)

static float gStorage[kStorageFloats];

static void FillFrames(float *frames, uint32_t frameCount, uint32_t channels, float startValue)
{
    for (uint32_t frame = 0; frame < frameCount; frame++)
    {
        for (uint32_t ch = 0; ch < channels; ch++)
        {
            frames[frame * channels + ch] = startValue + (float)frame;
        }
    }
}

static void PoisonStorage(void)
{
    for (size_t i = 0; i < kStorageFloats; i++)
    {
        gStorage[i] = -999.0f;
    }
}

/* Read-what-you-wrote, well within the ring: the basic contract. */
static void test_write_then_read_matches(void)
{
    LoomixRingBuffer rb;
    PoisonStorage();
    LoomixRingBuffer_Init(&rb, gStorage, kTestChannels);

    float written[100 * kTestChannels];
    FillFrames(written, 100, kTestChannels, 0.0f);
    LoomixRingBuffer_Write(&rb, 0, 100, written);

    float readBack[100 * kTestChannels];
    LoomixRingBuffer_Read(&rb, 0, 100, readBack);

    assert(memcmp(written, readBack, sizeof(written)) == 0);
    assert(rb.writeDiscontinuityCount == 0);
    assert(rb.readDiscontinuityCount == 0);
}

/* A read for a sample range nothing has ever written to must come back
 * silent -- not whatever garbage happens to sit in that ring slot. This
 * is the exact shape of the original M1 bug: with no write-cursor check,
 * an early or racing read returned poisoned/stale memory verbatim. */
static void test_unwritten_region_is_silent_not_poison(void)
{
    LoomixRingBuffer rb;
    PoisonStorage();
    LoomixRingBuffer_Init(&rb, gStorage, kTestChannels);

    float readBack[50 * kTestChannels];
    memset(readBack, 0x7f, sizeof(readBack)); /* not already zero */
    LoomixRingBuffer_Read(&rb, 0, 50, readBack);

    for (size_t i = 0; i < 50 * kTestChannels; i++)
    {
        assert(readBack[i] == 0.0f);
    }
}

/* A read that partially overlaps written and unwritten regions must
 * return exact data for the written part and silence for the rest, not
 * poison for the rest. */
static void test_partial_block_written_part_exact_rest_silent(void)
{
    LoomixRingBuffer rb;
    PoisonStorage();
    LoomixRingBuffer_Init(&rb, gStorage, kTestChannels);

    float written[100 * kTestChannels];
    FillFrames(written, 100, kTestChannels, 10.0f);
    LoomixRingBuffer_Write(&rb, 0, 100, written);

    /* Ask for frames 50..199: only 50..99 were written. */
    float readBack[150 * kTestChannels];
    memset(readBack, 0x7f, sizeof(readBack));
    LoomixRingBuffer_Read(&rb, 50, 150, readBack);

    assert(memcmp(readBack, written + (size_t)50 * kTestChannels, (size_t)50 * kTestChannels * sizeof(float)) == 0);
    for (size_t i = (size_t)50 * kTestChannels; i < 150 * kTestChannels; i++)
    {
        assert(readBack[i] == 0.0f);
    }
}

/* Write across the capacity boundary so it wraps to the front of the
 * ring, then read the same wrapped range back. */
static void test_write_read_across_wrap_boundary(void)
{
    LoomixRingBuffer rb;
    PoisonStorage();
    LoomixRingBuffer_Init(&rb, gStorage, kTestChannels);

    uint32_t frameCount = 40;
    double startTime = (double)kLoomixRingBufferFrameCapacity - 20; /* spans the wrap */

    float written[40 * kTestChannels];
    FillFrames(written, frameCount, kTestChannels, 500.0f);
    LoomixRingBuffer_Write(&rb, startTime, frameCount, written);

    float readBack[40 * kTestChannels];
    LoomixRingBuffer_Read(&rb, startTime, frameCount, readBack);

    assert(memcmp(written, readBack, sizeof(written)) == 0);
}

/* A write for an earlier sample time than one already written (e.g. a
 * late-arriving or reordered cycle) must not pull the high-water mark
 * backward -- otherwise a subsequent read of the already-advanced region
 * would wrongly see silence. */
static void test_out_of_order_write_does_not_regress_cursor(void)
{
    LoomixRingBuffer rb;
    PoisonStorage();
    LoomixRingBuffer_Init(&rb, gStorage, kTestChannels);

    float first[100 * kTestChannels];
    FillFrames(first, 100, kTestChannels, 0.0f);
    LoomixRingBuffer_Write(&rb, 1000, 100, first); /* writes through sample 1100 */
    assert(rb.writeCursorSampleTime == 1100);

    float earlier[50 * kTestChannels];
    FillFrames(earlier, 50, kTestChannels, 900.0f);
    LoomixRingBuffer_Write(&rb, 500, 50, earlier); /* writes through sample 550, well behind */
    assert(rb.writeCursorSampleTime == 1100);       /* must not regress */

    /* The region the first write covered must still read back exactly,
     * unaffected by the earlier, out-of-order second write. */
    float readBack[100 * kTestChannels];
    LoomixRingBuffer_Read(&rb, 1000, 100, readBack);
    assert(memcmp(first, readBack, sizeof(first)) == 0);
}

/* A gap between calls (read or write skips ahead) must be counted, not
 * silently absorbed -- this is what a test client checks after a session
 * via the kLoomixCustomProperty_*Discontinuities properties. */
static void test_gap_between_calls_is_counted(void)
{
    LoomixRingBuffer rb;
    PoisonStorage();
    LoomixRingBuffer_Init(&rb, gStorage, kTestChannels);

    float frames[10 * kTestChannels];
    FillFrames(frames, 10, kTestChannels, 0.0f);

    LoomixRingBuffer_Write(&rb, 0, 10, frames);
    assert(rb.writeDiscontinuityCount == 0);
    LoomixRingBuffer_Write(&rb, 10, 10, frames); /* contiguous: 0..10 then 10..20 */
    assert(rb.writeDiscontinuityCount == 0);
    LoomixRingBuffer_Write(&rb, 100, 10, frames); /* gap: expected 20, got 100 */
    assert(rb.writeDiscontinuityCount == 1);

    float readBack[10 * kTestChannels];
    LoomixRingBuffer_Read(&rb, 0, 10, readBack);
    assert(rb.readDiscontinuityCount == 0);
    LoomixRingBuffer_Read(&rb, 500, 10, readBack); /* first read ever at a nonzero start counts */
    assert(rb.readDiscontinuityCount == 1);
}

/* Reset must bring the cursor, bookkeeping and counters back to a fresh
 * session's starting state without touching storage or channel count. */
static void test_reset_clears_session_state(void)
{
    LoomixRingBuffer rb;
    PoisonStorage();
    LoomixRingBuffer_Init(&rb, gStorage, kTestChannels);

    float frames[10 * kTestChannels];
    FillFrames(frames, 10, kTestChannels, 0.0f);
    LoomixRingBuffer_Write(&rb, 0, 10, frames);
    LoomixRingBuffer_Write(&rb, 200, 10, frames); /* forces a counted gap */
    assert(rb.writeDiscontinuityCount == 1);

    LoomixRingBuffer_Reset(&rb);
    assert(rb.writeCursorSampleTime == 0);
    assert(rb.writeDiscontinuityCount == 0);
    assert(rb.readDiscontinuityCount == 0);
    assert(rb.channelCount == kTestChannels); /* untouched by reset */

    /* A read at sample 0 after reset must be silent: reset doesn't erase
     * storage, but it does erase the write cursor, so nothing reads back
     * as "written" even though the earlier write physically left data
     * there. */
    float readBack[10 * kTestChannels];
    memset(readBack, 0x7f, sizeof(readBack));
    LoomixRingBuffer_Read(&rb, 0, 10, readBack);
    for (size_t i = 0; i < 10 * kTestChannels; i++)
    {
        assert(readBack[i] == 0.0f);
    }
}

int main(void)
{
    test_write_then_read_matches();
    test_unwritten_region_is_silent_not_poison();
    test_partial_block_written_part_exact_rest_silent();
    test_write_read_across_wrap_boundary();
    test_out_of_order_write_does_not_regress_cursor();
    test_gap_between_calls_is_counted();
    test_reset_clears_session_state();

    printf("PASS: 7 ring buffer tests\n");
    return 0;
}
