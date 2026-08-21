#include "RingBuffer.h"

#include <string.h>

void LoomixRingBuffer_Init(LoomixRingBuffer *rb, float *storage, uint32_t channelCount)
{
    rb->samples = storage;
    rb->channelCount = channelCount;
    LoomixRingBuffer_Reset(rb);
}

void LoomixRingBuffer_Reset(LoomixRingBuffer *rb)
{
    rb->writeCursorSampleTime = 0;
    rb->lastWriteEndSampleTime = -1;
    rb->lastReadEndSampleTime = -1;
    rb->writeDiscontinuityCount = 0;
    rb->readDiscontinuityCount = 0;
}

void LoomixRingBuffer_SetChannelCount(LoomixRingBuffer *rb, uint32_t channelCount)
{
    rb->channelCount = channelCount;
}

void LoomixRingBuffer_Write(LoomixRingBuffer *rb, double inSampleTime, uint32_t inFrameCount, const float *inFrames)
{
    if (rb->lastWriteEndSampleTime >= 0 && inSampleTime != rb->lastWriteEndSampleTime)
    {
        rb->writeDiscontinuityCount++;
    }
    rb->lastWriteEndSampleTime = inSampleTime + inFrameCount;

    uint32_t channels = rb->channelCount;
    uint64_t startFrame = (uint64_t)inSampleTime % kLoomixRingBufferFrameCapacity;
    uint32_t firstRunFrames = inFrameCount;
    if (startFrame + inFrameCount > kLoomixRingBufferFrameCapacity)
    {
        firstRunFrames = (uint32_t)(kLoomixRingBufferFrameCapacity - startFrame);
    }
    uint32_t secondRunFrames = inFrameCount - firstRunFrames;

    float *ringStart = rb->samples + startFrame * kLoomixMaxChannels;

    for (uint32_t frame = 0; frame < firstRunFrames; frame++)
    {
        memcpy(ringStart + (size_t)frame * kLoomixMaxChannels, inFrames + (size_t)frame * channels, channels * sizeof(float));
    }
    for (uint32_t frame = 0; frame < secondRunFrames; frame++)
    {
        memcpy(rb->samples + (size_t)frame * kLoomixMaxChannels, inFrames + (size_t)(firstRunFrames + frame) * channels,
               channels * sizeof(float));
    }

    uint64_t writtenThrough = (uint64_t)inSampleTime + inFrameCount;
    if (writtenThrough > rb->writeCursorSampleTime)
    {
        rb->writeCursorSampleTime = writtenThrough;
    }
}

void LoomixRingBuffer_Read(LoomixRingBuffer *rb, double inSampleTime, uint32_t inFrameCount, float *outFrames)
{
    if (rb->lastReadEndSampleTime >= 0 && inSampleTime != rb->lastReadEndSampleTime)
    {
        rb->readDiscontinuityCount++;
    }
    rb->lastReadEndSampleTime = inSampleTime + inFrameCount;

    uint32_t channels = rb->channelCount;
    uint64_t requestStart = (uint64_t)inSampleTime;
    uint64_t startFrame = requestStart % kLoomixRingBufferFrameCapacity;

    for (uint32_t frame = 0; frame < inFrameCount; frame++)
    {
        float *outFrame = outFrames + (size_t)frame * channels;
        if (requestStart + frame < rb->writeCursorSampleTime)
        {
            uint64_t ringFrameIndex = (startFrame + frame) % kLoomixRingBufferFrameCapacity;
            memcpy(outFrame, rb->samples + ringFrameIndex * kLoomixMaxChannels, channels * sizeof(float));
        }
        else
        {
            memset(outFrame, 0, channels * sizeof(float));
        }
    }
}
