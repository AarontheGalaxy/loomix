/*
 * Loomix virtual audio driver, M1: one device pair ("Loomix In 1") that
 * appears as both a playback device (apps write to its output stream) and
 * a capture device (a client reads its input stream). The two are joined
 * by a ring buffer indexed by the device's own synthesized sample-time
 * clock, so a frame a client writes in one IO cycle lands exactly where a
 * capture client reads it back in a later cycle -- the standard technique
 * for a CoreAudio loopback device, implemented here directly against
 * Apple's public <CoreAudio/AudioServerPlugIn.h> contract (spec section
 * 2.1: read BlackHole for the shape, write an independent implementation).
 */

#include "LoomixAudioDriver.h"

#include <CoreFoundation/CoreFoundation.h>
#include <mach/mach_time.h>
#include <stdlib.h>
#include <string.h>

#pragma mark Forward declarations

static HRESULT LoomixDriver_QueryInterface(void *inDriver, REFIID inUUID, LPVOID *outInterface);
static ULONG LoomixDriver_AddRef(void *inDriver);
static ULONG LoomixDriver_Release(void *inDriver);
static OSStatus LoomixDriver_Initialize(AudioServerPlugInDriverRef inDriver, AudioServerPlugInHostRef inHost);
static OSStatus LoomixDriver_CreateDevice(AudioServerPlugInDriverRef inDriver, CFDictionaryRef inDescription,
                                           const AudioServerPlugInClientInfo *inClientInfo, AudioObjectID *outDeviceObjectID);
static OSStatus LoomixDriver_DestroyDevice(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID);
static OSStatus LoomixDriver_AddDeviceClient(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                              const AudioServerPlugInClientInfo *inClientInfo);
static OSStatus LoomixDriver_RemoveDeviceClient(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                                 const AudioServerPlugInClientInfo *inClientInfo);
static OSStatus LoomixDriver_PerformDeviceConfigurationChange(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                                                UInt64 inChangeAction, void *inChangeInfo);
static OSStatus LoomixDriver_AbortDeviceConfigurationChange(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                                               UInt64 inChangeAction, void *inChangeInfo);
static Boolean LoomixDriver_HasProperty(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID,
                                         const AudioObjectPropertyAddress *inAddress);
static OSStatus LoomixDriver_IsPropertySettable(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID,
                                                 const AudioObjectPropertyAddress *inAddress, Boolean *outIsSettable);
static OSStatus LoomixDriver_GetPropertyDataSize(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID,
                                                  const AudioObjectPropertyAddress *inAddress, UInt32 inQualifierDataSize,
                                                  const void *inQualifierData, UInt32 *outDataSize);
static OSStatus LoomixDriver_GetPropertyData(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID,
                                              const AudioObjectPropertyAddress *inAddress, UInt32 inQualifierDataSize,
                                              const void *inQualifierData, UInt32 inDataSize, UInt32 *outDataSize, void *outData);
static OSStatus LoomixDriver_SetPropertyData(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID,
                                              const AudioObjectPropertyAddress *inAddress, UInt32 inQualifierDataSize,
                                              const void *inQualifierData, UInt32 inDataSize, const void *inData);
static OSStatus LoomixDriver_StartIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID);
static OSStatus LoomixDriver_StopIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID);
static OSStatus LoomixDriver_GetZeroTimeStamp(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID,
                                               Float64 *outSampleTime, UInt64 *outHostTime, UInt64 *outSeed);
static OSStatus LoomixDriver_WillDoIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID,
                                                UInt32 inOperationID, Boolean *outWillDo, Boolean *outWillDoInPlace);
static OSStatus LoomixDriver_BeginIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID,
                                               UInt32 inOperationID, UInt32 inIOBufferFrameSize, const AudioServerPlugInIOCycleInfo *inIOCycleInfo);
static OSStatus LoomixDriver_DoIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, AudioObjectID inStreamObjectID,
                                            UInt32 inClientID, UInt32 inOperationID, UInt32 inIOBufferFrameSize,
                                            const AudioServerPlugInIOCycleInfo *inIOCycleInfo, void *ioMainBuffer, void *ioSecondaryBuffer);
static OSStatus LoomixDriver_EndIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID,
                                             UInt32 inOperationID, UInt32 inIOBufferFrameSize, const AudioServerPlugInIOCycleInfo *inIOCycleInfo);

#pragma mark Driver singleton

static AudioServerPlugInDriverInterface gDriverInterface = {
    NULL,
    LoomixDriver_QueryInterface,
    LoomixDriver_AddRef,
    LoomixDriver_Release,
    LoomixDriver_Initialize,
    LoomixDriver_CreateDevice,
    LoomixDriver_DestroyDevice,
    LoomixDriver_AddDeviceClient,
    LoomixDriver_RemoveDeviceClient,
    LoomixDriver_PerformDeviceConfigurationChange,
    LoomixDriver_AbortDeviceConfigurationChange,
    LoomixDriver_HasProperty,
    LoomixDriver_IsPropertySettable,
    LoomixDriver_GetPropertyDataSize,
    LoomixDriver_GetPropertyData,
    LoomixDriver_SetPropertyData,
    LoomixDriver_StartIO,
    LoomixDriver_StopIO,
    LoomixDriver_GetZeroTimeStamp,
    LoomixDriver_WillDoIOOperation,
    LoomixDriver_BeginIOOperation,
    LoomixDriver_DoIOOperation,
    LoomixDriver_EndIOOperation,
};
static AudioServerPlugInDriverInterface *gDriverInterfacePtr = &gDriverInterface;
static AudioServerPlugInDriverRef gDriverRef = &gDriverInterfacePtr;
static ULONG gRefCount = 1;

static LoomixDriverState gState = {
    .mDriverRef = NULL,
    .mHost = NULL,
    .mSampleRate = kLoomixDefaultSampleRate,
    .mChannelCount = kLoomixDefaultChannels,
    .mStartHostTime = 0,
    .mIsRunning = false,
    .mIORunningClients = 0,
    .mRingStorage = NULL,
};

#define kZeroTimeStampPeriod ((UInt32)16384)

typedef struct
{
    Float64 mNewSampleRate;
    UInt32 mNewChannelCount;
} LoomixConfigurationChange;

#pragma mark CFPlugIn factory

void *LoomixAudioDriver_Create(CFAllocatorRef inAllocator, CFUUIDRef inRequestedTypeUUID);
void *LoomixAudioDriver_Create(CFAllocatorRef inAllocator, CFUUIDRef inRequestedTypeUUID)
{
    (void)inAllocator;
    if (!CFEqual(inRequestedTypeUUID, kAudioServerPlugInTypeUUID))
    {
        return NULL;
    }
    if (gState.mRingStorage == NULL)
    {
        gState.mRingStorage = (float *)calloc((size_t)kLoomixRingBufferFrameCapacity * kLoomixMaxChannels, sizeof(float));
        LoomixRingBuffer_Init(&gState.mRing, gState.mRingStorage, gState.mChannelCount);
        pthread_mutex_init(&gState.mStateMutex, NULL);
    }
    LoomixDriver_AddRef((void *)gDriverRef);
    return (void *)gDriverRef;
}

#pragma mark IUnknown

static HRESULT LoomixDriver_QueryInterface(void *inDriver, REFIID inUUID, LPVOID *outInterface)
{
    if (inDriver == NULL || outInterface == NULL)
    {
        return E_INVALIDARG;
    }

    CFUUIDRef requested = CFUUIDCreateFromUUIDBytes(NULL, inUUID);
    if (requested == NULL)
    {
        return E_INVALIDARG;
    }

    HRESULT result = E_NOINTERFACE;
    if (CFEqual(requested, IUnknownUUID) || CFEqual(requested, kAudioServerPlugInDriverInterfaceUUID))
    {
        LoomixDriver_AddRef(inDriver);
        *outInterface = inDriver;
        result = S_OK;
    }
    CFRelease(requested);
    return result;
}

static ULONG LoomixDriver_AddRef(void *inDriver)
{
    (void)inDriver;
    return (ULONG)__sync_add_and_fetch(&gRefCount, 1);
}

static ULONG LoomixDriver_Release(void *inDriver)
{
    (void)inDriver;
    return (ULONG)__sync_sub_and_fetch(&gRefCount, 1);
}

#pragma mark Basic operations

static OSStatus LoomixDriver_Initialize(AudioServerPlugInDriverRef inDriver, AudioServerPlugInHostRef inHost)
{
    gState.mDriverRef = inDriver;
    gState.mHost = inHost;
    return kAudioHardwareNoError;
}

/* Loomix In 1 is a static device published at Initialize, not one the
 * host creates dynamically -- CreateDevice/DestroyDevice are for the
 * "create a custom device from Audio MIDI Setup" flow, which this driver
 * doesn't support. */
static OSStatus LoomixDriver_CreateDevice(AudioServerPlugInDriverRef inDriver, CFDictionaryRef inDescription,
                                           const AudioServerPlugInClientInfo *inClientInfo, AudioObjectID *outDeviceObjectID)
{
    (void)inDriver;
    (void)inDescription;
    (void)inClientInfo;
    (void)outDeviceObjectID;
    return kAudioHardwareUnsupportedOperationError;
}

static OSStatus LoomixDriver_DestroyDevice(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID)
{
    (void)inDriver;
    (void)inDeviceObjectID;
    return kAudioHardwareUnsupportedOperationError;
}

static OSStatus LoomixDriver_AddDeviceClient(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                              const AudioServerPlugInClientInfo *inClientInfo)
{
    (void)inDriver;
    (void)inDeviceObjectID;
    (void)inClientInfo;
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_RemoveDeviceClient(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                                 const AudioServerPlugInClientInfo *inClientInfo)
{
    (void)inDriver;
    (void)inDeviceObjectID;
    (void)inClientInfo;
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_PerformDeviceConfigurationChange(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                                                UInt64 inChangeAction, void *inChangeInfo)
{
    (void)inDriver;
    (void)inChangeAction;
    if (inDeviceObjectID != kObjectID_Device || inChangeInfo == NULL)
    {
        return kAudioHardwareBadObjectError;
    }

    LoomixConfigurationChange *change = (LoomixConfigurationChange *)inChangeInfo;
    pthread_mutex_lock(&gState.mStateMutex);
    gState.mSampleRate = change->mNewSampleRate;
    gState.mChannelCount = change->mNewChannelCount;
    LoomixRingBuffer_SetChannelCount(&gState.mRing, change->mNewChannelCount);
    pthread_mutex_unlock(&gState.mStateMutex);
    free(change);

    if (gState.mHost != NULL)
    {
        AudioObjectPropertyAddress changed[] = {
            {kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain},
            {kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain},
            {kAudioStreamPropertyPhysicalFormat, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain},
        };
        gState.mHost->PropertiesChanged(gState.mHost, kObjectID_Device, 3, changed);
    }
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_AbortDeviceConfigurationChange(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                                               UInt64 inChangeAction, void *inChangeInfo)
{
    (void)inDriver;
    (void)inDeviceObjectID;
    (void)inChangeAction;
    free(inChangeInfo);
    return kAudioHardwareNoError;
}

#pragma mark Property helpers

static Boolean FormatIsSupported(Float64 inSampleRate, UInt32 inChannelCount)
{
    if (inChannelCount < kLoomixMinChannels || inChannelCount > kLoomixMaxChannels)
    {
        return false;
    }
    for (UInt32 i = 0; i < kLoomixSupportedSampleRateCount; i++)
    {
        if (kLoomixSupportedSampleRates[i] == inSampleRate)
        {
            return true;
        }
    }
    return false;
}

static AudioStreamBasicDescription CurrentStreamFormat(void)
{
    AudioStreamBasicDescription format;
    memset(&format, 0, sizeof(format));
    format.mSampleRate = gState.mSampleRate;
    format.mFormatID = kAudioFormatLinearPCM;
    format.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
    format.mBytesPerPacket = gState.mChannelCount * sizeof(Float32);
    format.mFramesPerPacket = 1;
    format.mBytesPerFrame = gState.mChannelCount * sizeof(Float32);
    format.mChannelsPerFrame = gState.mChannelCount;
    format.mBitsPerChannel = 8 * sizeof(Float32);
    return format;
}

#define kLoomixAvailableFormatCount (kLoomixSupportedSampleRateCount * (kLoomixMaxChannels - kLoomixMinChannels + 1))

static UInt32 FillAvailableFormats(AudioStreamRangedDescription *outFormats)
{
    UInt32 count = 0;
    for (UInt32 rateIndex = 0; rateIndex < kLoomixSupportedSampleRateCount; rateIndex++)
    {
        for (UInt32 channels = kLoomixMinChannels; channels <= kLoomixMaxChannels; channels++)
        {
            AudioStreamBasicDescription format;
            memset(&format, 0, sizeof(format));
            format.mSampleRate = kLoomixSupportedSampleRates[rateIndex];
            format.mFormatID = kAudioFormatLinearPCM;
            format.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
            format.mBytesPerPacket = channels * sizeof(Float32);
            format.mFramesPerPacket = 1;
            format.mBytesPerFrame = channels * sizeof(Float32);
            format.mChannelsPerFrame = channels;
            format.mBitsPerChannel = 8 * sizeof(Float32);

            outFormats[count].mFormat = format;
            outFormats[count].mSampleRateRange.mMinimum = format.mSampleRate;
            outFormats[count].mSampleRateRange.mMaximum = format.mSampleRate;
            count++;
        }
    }
    return count;
}

static Boolean IsStreamObject(AudioObjectID inObjectID)
{
    return inObjectID == kObjectID_Stream_Input || inObjectID == kObjectID_Stream_Output;
}

#pragma mark Property operations

static Boolean LoomixDriver_HasProperty(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID,
                                         const AudioObjectPropertyAddress *inAddress)
{
    UInt32 dataSize = 0;
    OSStatus status = LoomixDriver_GetPropertyDataSize(inDriver, inObjectID, inClientProcessID, inAddress, 0, NULL, &dataSize);
    return status == kAudioHardwareNoError;
}

static OSStatus LoomixDriver_IsPropertySettable(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID,
                                                 const AudioObjectPropertyAddress *inAddress, Boolean *outIsSettable)
{
    (void)inDriver;
    (void)inClientProcessID;
    *outIsSettable = false;

    Boolean isDeviceSampleRate = inObjectID == kObjectID_Device && inAddress->mSelector == kAudioDevicePropertyNominalSampleRate;
    Boolean isStreamFormat = IsStreamObject(inObjectID) &&
                              (inAddress->mSelector == kAudioStreamPropertyVirtualFormat || inAddress->mSelector == kAudioStreamPropertyPhysicalFormat);
    *outIsSettable = isDeviceSampleRate || isStreamFormat;
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_GetPropertyDataSize(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID,
                                                  const AudioObjectPropertyAddress *inAddress, UInt32 inQualifierDataSize,
                                                  const void *inQualifierData, UInt32 *outDataSize)
{
    (void)inDriver;
    (void)inClientProcessID;
    (void)inQualifierDataSize;
    (void)inQualifierData;

    AudioObjectPropertySelector selector = inAddress->mSelector;

    if (inObjectID == kObjectID_PlugIn)
    {
        switch (selector)
        {
        case kAudioObjectPropertyBaseClass:
        case kAudioObjectPropertyClass:
        case kAudioObjectPropertyOwner:
            *outDataSize = sizeof(AudioClassID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyManufacturer:
        case kAudioPlugInPropertyBundleID:
            *outDataSize = sizeof(CFStringRef);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyOwnedObjects:
        case kAudioPlugInPropertyDeviceList:
            *outDataSize = sizeof(AudioObjectID);
            return kAudioHardwareNoError;
        case kAudioPlugInPropertyBoxList:
        case kAudioPlugInPropertyClockDeviceList:
            *outDataSize = 0;
            return kAudioHardwareNoError;
        case kAudioPlugInPropertyTranslateUIDToDevice:
            *outDataSize = sizeof(AudioObjectID);
            return kAudioHardwareNoError;
        default:
            return kAudioHardwareUnknownPropertyError;
        }
    }
    else if (inObjectID == kObjectID_Device)
    {
        switch (selector)
        {
        case kAudioObjectPropertyBaseClass:
        case kAudioObjectPropertyClass:
        case kAudioObjectPropertyOwner:
            *outDataSize = sizeof(AudioClassID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyName:
        case kAudioObjectPropertyManufacturer:
        case kAudioDevicePropertyDeviceUID:
        case kAudioDevicePropertyModelUID:
            *outDataSize = sizeof(CFStringRef);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyOwnedObjects:
        case kAudioDevicePropertyStreams:
        case kAudioDevicePropertyRelatedDevices:
            *outDataSize = (inAddress->mScope == kAudioObjectPropertyScopeGlobal ? 2 : 1) * sizeof(AudioObjectID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyControlList:
            *outDataSize = 0;
            return kAudioHardwareNoError;
        case kAudioDevicePropertyTransportType:
        case kAudioDevicePropertyClockDomain:
        case kAudioDevicePropertyDeviceIsAlive:
        case kAudioDevicePropertyDeviceIsRunning:
        case kAudioDevicePropertyDeviceCanBeDefaultDevice:
        case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice:
        case kAudioDevicePropertyLatency:
        case kAudioDevicePropertySafetyOffset:
        case kAudioDevicePropertyIsHidden:
        case kAudioDevicePropertyZeroTimeStampPeriod:
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyNominalSampleRate:
            *outDataSize = sizeof(Float64);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyAvailableNominalSampleRates:
            *outDataSize = kLoomixSupportedSampleRateCount * sizeof(AudioValueRange);
            return kAudioHardwareNoError;
        default:
            return kAudioHardwareUnknownPropertyError;
        }
    }
    else if (IsStreamObject(inObjectID))
    {
        switch (selector)
        {
        case kAudioObjectPropertyBaseClass:
        case kAudioObjectPropertyClass:
        case kAudioObjectPropertyOwner:
            *outDataSize = sizeof(AudioClassID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyOwnedObjects:
            *outDataSize = 0;
            return kAudioHardwareNoError;
        case kAudioStreamPropertyIsActive:
        case kAudioStreamPropertyDirection:
        case kAudioStreamPropertyTerminalType:
        case kAudioStreamPropertyStartingChannel:
        case kAudioStreamPropertyLatency:
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioStreamPropertyVirtualFormat:
        case kAudioStreamPropertyPhysicalFormat:
            *outDataSize = sizeof(AudioStreamBasicDescription);
            return kAudioHardwareNoError;
        case kAudioStreamPropertyAvailableVirtualFormats:
        case kAudioStreamPropertyAvailablePhysicalFormats:
            *outDataSize = kLoomixAvailableFormatCount * sizeof(AudioStreamRangedDescription);
            return kAudioHardwareNoError;
        default:
            return kAudioHardwareUnknownPropertyError;
        }
    }
    return kAudioHardwareBadObjectError;
}

static OSStatus LoomixDriver_GetPropertyData(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID,
                                              const AudioObjectPropertyAddress *inAddress, UInt32 inQualifierDataSize,
                                              const void *inQualifierData, UInt32 inDataSize, UInt32 *outDataSize, void *outData)
{
    (void)inDriver;
    (void)inClientProcessID;

    AudioObjectPropertySelector selector = inAddress->mSelector;

    if (inObjectID == kObjectID_PlugIn)
    {
        switch (selector)
        {
        case kAudioObjectPropertyBaseClass:
            *(AudioClassID *)outData = kAudioObjectClassID;
            *outDataSize = sizeof(AudioClassID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyClass:
            *(AudioClassID *)outData = kAudioPlugInClassID;
            *outDataSize = sizeof(AudioClassID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyOwner:
            *(AudioObjectID *)outData = kAudioObjectUnknown;
            *outDataSize = sizeof(AudioObjectID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyManufacturer:
            *(CFStringRef *)outData = CFSTR("Loomix");
            *outDataSize = sizeof(CFStringRef);
            return kAudioHardwareNoError;
        case kAudioPlugInPropertyBundleID:
            *(CFStringRef *)outData = CFSTR("com.loomix.audiodriver");
            *outDataSize = sizeof(CFStringRef);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyOwnedObjects:
        case kAudioPlugInPropertyDeviceList:
            if (inDataSize < sizeof(AudioObjectID))
            {
                return kAudioHardwareBadPropertySizeError;
            }
            *(AudioObjectID *)outData = kObjectID_Device;
            *outDataSize = sizeof(AudioObjectID);
            return kAudioHardwareNoError;
        case kAudioPlugInPropertyBoxList:
        case kAudioPlugInPropertyClockDeviceList:
            *outDataSize = 0;
            return kAudioHardwareNoError;
        case kAudioPlugInPropertyTranslateUIDToDevice:
        {
            AudioObjectID result = kAudioObjectUnknown;
            if (inQualifierDataSize == sizeof(CFStringRef) && inQualifierData != NULL)
            {
                CFStringRef uid = *(const CFStringRef *)inQualifierData;
                if (uid != NULL && CFEqual(uid, CFSTR("com.loomix.audiodriver.in1")))
                {
                    result = kObjectID_Device;
                }
            }
            *(AudioObjectID *)outData = result;
            *outDataSize = sizeof(AudioObjectID);
            return kAudioHardwareNoError;
        }
        default:
            return kAudioHardwareUnknownPropertyError;
        }
    }
    else if (inObjectID == kObjectID_Device)
    {
        switch (selector)
        {
        case kAudioObjectPropertyBaseClass:
            *(AudioClassID *)outData = kAudioObjectClassID;
            *outDataSize = sizeof(AudioClassID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyClass:
            *(AudioClassID *)outData = kAudioDeviceClassID;
            *outDataSize = sizeof(AudioClassID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyOwner:
            *(AudioObjectID *)outData = kObjectID_PlugIn;
            *outDataSize = sizeof(AudioObjectID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyName:
            *(CFStringRef *)outData = CFSTR("Loomix In 1");
            *outDataSize = sizeof(CFStringRef);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyManufacturer:
            *(CFStringRef *)outData = CFSTR("Loomix");
            *outDataSize = sizeof(CFStringRef);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyDeviceUID:
            *(CFStringRef *)outData = CFSTR("com.loomix.audiodriver.in1");
            *outDataSize = sizeof(CFStringRef);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyModelUID:
            *(CFStringRef *)outData = CFSTR("com.loomix.audiodriver.model");
            *outDataSize = sizeof(CFStringRef);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyOwnedObjects:
        case kAudioDevicePropertyStreams:
        {
            AudioObjectID *ids = (AudioObjectID *)outData;
            UInt32 count = 0;
            if (inAddress->mScope == kAudioObjectPropertyScopeGlobal)
            {
                if (inDataSize < 2 * sizeof(AudioObjectID))
                {
                    return kAudioHardwareBadPropertySizeError;
                }
                ids[0] = kObjectID_Stream_Input;
                ids[1] = kObjectID_Stream_Output;
                count = 2;
            }
            else if (inAddress->mScope == kAudioObjectPropertyScopeInput)
            {
                if (inDataSize < sizeof(AudioObjectID))
                {
                    return kAudioHardwareBadPropertySizeError;
                }
                ids[0] = kObjectID_Stream_Input;
                count = 1;
            }
            else
            {
                if (inDataSize < sizeof(AudioObjectID))
                {
                    return kAudioHardwareBadPropertySizeError;
                }
                ids[0] = kObjectID_Stream_Output;
                count = 1;
            }
            *outDataSize = count * sizeof(AudioObjectID);
            return kAudioHardwareNoError;
        }
        case kAudioDevicePropertyRelatedDevices:
            if (inDataSize < sizeof(AudioObjectID))
            {
                return kAudioHardwareBadPropertySizeError;
            }
            *(AudioObjectID *)outData = kObjectID_Device;
            *outDataSize = sizeof(AudioObjectID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyControlList:
            *outDataSize = 0;
            return kAudioHardwareNoError;
        case kAudioDevicePropertyTransportType:
            *(UInt32 *)outData = kAudioDeviceTransportTypeVirtual;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyClockDomain:
        case kAudioDevicePropertyLatency:
        case kAudioDevicePropertyIsHidden:
            *(UInt32 *)outData = 0;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioDevicePropertySafetyOffset:
            /* Gives WriteMix a guaranteed head start over ReadInput each
             * cycle. At 0 the host has no reason to keep the read side
             * behind the write side, and occasionally schedules a read
             * for a ring-buffer slot WriteMix hasn't reached yet that
             * cycle, which reads back a stale, already-delivered frame
             * instead of a fresh one -- observed as a single duplicated
             * block partway through an otherwise bit-exact capture. */
            *(UInt32 *)outData = inAddress->mScope == kAudioObjectPropertyScopeInput ? kLoomixInputSafetyOffsetFrames : 0;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyDeviceIsAlive:
        case kAudioDevicePropertyDeviceCanBeDefaultDevice:
            *(UInt32 *)outData = 1;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice:
            *(UInt32 *)outData = 0;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyDeviceIsRunning:
            *(UInt32 *)outData = gState.mIsRunning ? 1 : 0;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyZeroTimeStampPeriod:
            *(UInt32 *)outData = kZeroTimeStampPeriod;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyNominalSampleRate:
            *(Float64 *)outData = gState.mSampleRate;
            *outDataSize = sizeof(Float64);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyAvailableNominalSampleRates:
        {
            UInt32 maxCount = inDataSize / sizeof(AudioValueRange);
            UInt32 count = maxCount < kLoomixSupportedSampleRateCount ? maxCount : (UInt32)kLoomixSupportedSampleRateCount;
            AudioValueRange *ranges = (AudioValueRange *)outData;
            for (UInt32 i = 0; i < count; i++)
            {
                ranges[i].mMinimum = kLoomixSupportedSampleRates[i];
                ranges[i].mMaximum = kLoomixSupportedSampleRates[i];
            }
            *outDataSize = count * sizeof(AudioValueRange);
            return kAudioHardwareNoError;
        }
        default:
            return kAudioHardwareUnknownPropertyError;
        }
    }
    else if (IsStreamObject(inObjectID))
    {
        switch (selector)
        {
        case kAudioObjectPropertyBaseClass:
            *(AudioClassID *)outData = kAudioObjectClassID;
            *outDataSize = sizeof(AudioClassID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyClass:
            *(AudioClassID *)outData = kAudioStreamClassID;
            *outDataSize = sizeof(AudioClassID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyOwner:
            *(AudioObjectID *)outData = kObjectID_Device;
            *outDataSize = sizeof(AudioObjectID);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyOwnedObjects:
            *outDataSize = 0;
            return kAudioHardwareNoError;
        case kAudioStreamPropertyIsActive:
            *(UInt32 *)outData = 1;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioStreamPropertyDirection:
            *(UInt32 *)outData = inObjectID == kObjectID_Stream_Input ? 1 : 0;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioStreamPropertyTerminalType:
            *(UInt32 *)outData = kAudioStreamTerminalTypeLine;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioStreamPropertyStartingChannel:
            *(UInt32 *)outData = 1;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioStreamPropertyLatency:
            *(UInt32 *)outData = 0;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioStreamPropertyVirtualFormat:
        case kAudioStreamPropertyPhysicalFormat:
            if (inDataSize < sizeof(AudioStreamBasicDescription))
            {
                return kAudioHardwareBadPropertySizeError;
            }
            *(AudioStreamBasicDescription *)outData = CurrentStreamFormat();
            *outDataSize = sizeof(AudioStreamBasicDescription);
            return kAudioHardwareNoError;
        case kAudioStreamPropertyAvailableVirtualFormats:
        case kAudioStreamPropertyAvailablePhysicalFormats:
        {
            AudioStreamRangedDescription all[kLoomixAvailableFormatCount];
            UInt32 total = FillAvailableFormats(all);
            UInt32 maxCount = inDataSize / sizeof(AudioStreamRangedDescription);
            UInt32 count = maxCount < total ? maxCount : total;
            memcpy(outData, all, count * sizeof(AudioStreamRangedDescription));
            *outDataSize = count * sizeof(AudioStreamRangedDescription);
            return kAudioHardwareNoError;
        }
        default:
            return kAudioHardwareUnknownPropertyError;
        }
    }
    return kAudioHardwareBadObjectError;
}

static OSStatus LoomixDriver_SetPropertyData(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID,
                                              const AudioObjectPropertyAddress *inAddress, UInt32 inQualifierDataSize,
                                              const void *inQualifierData, UInt32 inDataSize, const void *inData)
{
    (void)inClientProcessID;
    (void)inQualifierDataSize;
    (void)inQualifierData;

    if (inObjectID == kObjectID_Device && inAddress->mSelector == kAudioDevicePropertyNominalSampleRate)
    {
        if (inDataSize != sizeof(Float64))
        {
            return kAudioHardwareBadPropertySizeError;
        }
        Float64 requested = *(const Float64 *)inData;
        if (!FormatIsSupported(requested, gState.mChannelCount))
        {
            return kAudioDeviceUnsupportedFormatError;
        }
        if (requested == gState.mSampleRate)
        {
            return kAudioHardwareNoError;
        }

        LoomixConfigurationChange *change = (LoomixConfigurationChange *)malloc(sizeof(LoomixConfigurationChange));
        change->mNewSampleRate = requested;
        change->mNewChannelCount = gState.mChannelCount;
        gState.mHost->RequestDeviceConfigurationChange(gState.mHost, kObjectID_Device, 0, change);
        return kAudioHardwareNoError;
    }
    else if (IsStreamObject(inObjectID) &&
             (inAddress->mSelector == kAudioStreamPropertyVirtualFormat || inAddress->mSelector == kAudioStreamPropertyPhysicalFormat))
    {
        if (inDataSize != sizeof(AudioStreamBasicDescription))
        {
            return kAudioHardwareBadPropertySizeError;
        }
        const AudioStreamBasicDescription *requested = (const AudioStreamBasicDescription *)inData;
        Boolean isFloat32Packed = (requested->mFormatID == kAudioFormatLinearPCM) &&
                                   (requested->mFormatFlags & kAudioFormatFlagIsFloat) &&
                                   (requested->mFormatFlags & kAudioFormatFlagIsPacked) &&
                                   (requested->mBitsPerChannel == 8 * sizeof(Float32));
        if (!isFloat32Packed || !FormatIsSupported(requested->mSampleRate, requested->mChannelsPerFrame))
        {
            return kAudioDeviceUnsupportedFormatError;
        }
        if (requested->mSampleRate == gState.mSampleRate && requested->mChannelsPerFrame == gState.mChannelCount)
        {
            return kAudioHardwareNoError;
        }

        LoomixConfigurationChange *change = (LoomixConfigurationChange *)malloc(sizeof(LoomixConfigurationChange));
        change->mNewSampleRate = requested->mSampleRate;
        change->mNewChannelCount = requested->mChannelsPerFrame;
        gState.mHost->RequestDeviceConfigurationChange(gState.mHost, kObjectID_Device, 0, change);
        return kAudioHardwareNoError;
    }
    (void)inDriver;
    return kAudioHardwareUnknownPropertyError;
}

#pragma mark IO operations

static OSStatus LoomixDriver_StartIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID)
{
    (void)inDriver;
    (void)inClientID;
    if (inDeviceObjectID != kObjectID_Device)
    {
        return kAudioHardwareBadObjectError;
    }
    if (__sync_add_and_fetch(&gState.mIORunningClients, 1) == 1)
    {
        gState.mStartHostTime = mach_absolute_time();
        LoomixRingBuffer_Reset(&gState.mRing);
        gState.mIsRunning = true;
    }
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_StopIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID)
{
    (void)inDriver;
    (void)inClientID;
    if (inDeviceObjectID != kObjectID_Device)
    {
        return kAudioHardwareBadObjectError;
    }
    if (__sync_sub_and_fetch(&gState.mIORunningClients, 1) == 0)
    {
        gState.mIsRunning = false;
    }
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_GetZeroTimeStamp(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID,
                                               Float64 *outSampleTime, UInt64 *outHostTime, UInt64 *outSeed)
{
    (void)inDriver;
    (void)inClientID;
    if (inDeviceObjectID != kObjectID_Device)
    {
        return kAudioHardwareBadObjectError;
    }

    static mach_timebase_info_data_t timebase = {0, 0};
    if (timebase.denom == 0)
    {
        mach_timebase_info(&timebase);
    }

    UInt64 now = mach_absolute_time();
    UInt64 elapsedTicks = now - gState.mStartHostTime;
    Float64 elapsedSeconds = (Float64)elapsedTicks * (Float64)timebase.numer / (Float64)timebase.denom / 1e9;
    Float64 elapsedFrames = elapsedSeconds * gState.mSampleRate;

    UInt64 periodIndex = (UInt64)elapsedFrames / kZeroTimeStampPeriod;
    Float64 sampleTime = (Float64)(periodIndex * kZeroTimeStampPeriod);
    Float64 periodSeconds = (Float64)kZeroTimeStampPeriod / gState.mSampleRate;
    UInt64 hostTicksPerPeriod = (UInt64)(periodSeconds * 1e9 * (Float64)timebase.denom / (Float64)timebase.numer);

    *outSampleTime = sampleTime;
    *outHostTime = gState.mStartHostTime + periodIndex * hostTicksPerPeriod;
    *outSeed = 1;
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_WillDoIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID,
                                                UInt32 inOperationID, Boolean *outWillDo, Boolean *outWillDoInPlace)
{
    (void)inDriver;
    (void)inDeviceObjectID;
    (void)inClientID;
    Boolean willDo = (inOperationID == kAudioServerPlugInIOOperationReadInput || inOperationID == kAudioServerPlugInIOOperationWriteMix);
    if (outWillDo != NULL)
    {
        *outWillDo = willDo;
    }
    if (outWillDoInPlace != NULL)
    {
        *outWillDoInPlace = true;
    }
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_BeginIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID,
                                               UInt32 inOperationID, UInt32 inIOBufferFrameSize, const AudioServerPlugInIOCycleInfo *inIOCycleInfo)
{
    (void)inDriver;
    (void)inDeviceObjectID;
    (void)inClientID;
    (void)inOperationID;
    (void)inIOBufferFrameSize;
    (void)inIOCycleInfo;
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_DoIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, AudioObjectID inStreamObjectID,
                                            UInt32 inClientID, UInt32 inOperationID, UInt32 inIOBufferFrameSize,
                                            const AudioServerPlugInIOCycleInfo *inIOCycleInfo, void *ioMainBuffer, void *ioSecondaryBuffer)
{
    (void)inDriver;
    (void)inClientID;
    (void)ioSecondaryBuffer;
    if (inDeviceObjectID != kObjectID_Device || ioMainBuffer == NULL)
    {
        return kAudioHardwareBadObjectError;
    }

    if (inOperationID == kAudioServerPlugInIOOperationWriteMix && inStreamObjectID == kObjectID_Stream_Output)
    {
        LoomixRingBuffer_Write(&gState.mRing, inIOCycleInfo->mOutputTime.mSampleTime, inIOBufferFrameSize, (const Float32 *)ioMainBuffer);
    }
    else if (inOperationID == kAudioServerPlugInIOOperationReadInput && inStreamObjectID == kObjectID_Stream_Input)
    {
        LoomixRingBuffer_Read(&gState.mRing, inIOCycleInfo->mInputTime.mSampleTime, inIOBufferFrameSize, (Float32 *)ioMainBuffer);
    }
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_EndIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID,
                                             UInt32 inOperationID, UInt32 inIOBufferFrameSize, const AudioServerPlugInIOCycleInfo *inIOCycleInfo)
{
    (void)inDriver;
    (void)inDeviceObjectID;
    (void)inClientID;
    (void)inOperationID;
    (void)inIOBufferFrameSize;
    (void)inIOCycleInfo;
    return kAudioHardwareNoError;
}
