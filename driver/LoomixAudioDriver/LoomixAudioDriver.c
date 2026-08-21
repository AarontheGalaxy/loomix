/*
 * Loomix virtual audio driver, M2: full topology -- 8 "Loomix In" devices
 * (spec section 1.1: playback devices apps write into, feeding the
 * mixer's hardware strips) and 8 "Loomix Out" devices (A1-A5, B1-B3,
 * carrying the bus outputs). Every device is still a self-contained
 * loopback pair as in M1 (an input stream and an output stream joined by
 * a ring buffer indexed by the device's own synthesized sample-time
 * clock), since the mixing engine that would feed the "Out" devices from
 * a real bus mix doesn't exist before M3. Implemented here directly
 * against Apple's public <CoreAudio/AudioServerPlugIn.h> contract (spec
 * section 2.1: read BlackHole for the shape, write an independent
 * implementation).
 */

#include "LoomixAudioDriver.h"

#include <CoreFoundation/CoreFoundation.h>
#include <mach/mach_time.h>
#include <stdlib.h>
#include <string.h>

/* Every allocation the driver makes goes through these two names rather
 * than calloc/malloc directly, so driver/tests/test_driver_host.c can
 * build a second copy of this file with -DLOOMIX_CALLOC=<fn> and
 * -DLOOMIX_MALLOC=<fn> pointed at functions that always return NULL, and
 * exercise the failure path of every call site for real -- without a
 * mocking framework, and without coreaudiod. Production behavior is
 * identical either way. */
#ifndef LOOMIX_CALLOC
#define LOOMIX_CALLOC calloc
#endif
#ifndef LOOMIX_MALLOC
#define LOOMIX_MALLOC malloc
#endif
/* Declared via macro expansion so this compiles either way: when the
 * macros are left at their defaults these just redeclare calloc/malloc
 * (harmless, matches stdlib.h), and when a test build points them at
 * always-failing replacements this is what makes those visible here
 * without this file naming them. */
extern void *LOOMIX_CALLOC(size_t inCount, size_t inSize);
extern void *LOOMIX_MALLOC(size_t inSize);

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

#pragma mark Device identity tables

static const CFStringRef kLoomixDeviceNames[kLoomixDeviceCount] = {
    CFSTR("Loomix In 1"),    CFSTR("Loomix In 2"),    CFSTR("Loomix In 3"),    CFSTR("Loomix In 4"),
    CFSTR("Loomix In 5"),    CFSTR("Loomix In 6"),    CFSTR("Loomix In 7"),    CFSTR("Loomix In 8"),
    CFSTR("Loomix Out A1"), CFSTR("Loomix Out A2"), CFSTR("Loomix Out A3"), CFSTR("Loomix Out A4"),
    CFSTR("Loomix Out A5"), CFSTR("Loomix Out B1"), CFSTR("Loomix Out B2"), CFSTR("Loomix Out B3"),
};

static const CFStringRef kLoomixDeviceUIDs[kLoomixDeviceCount] = {
    CFSTR("com.loomix.audiodriver.in1"),   CFSTR("com.loomix.audiodriver.in2"),   CFSTR("com.loomix.audiodriver.in3"),
    CFSTR("com.loomix.audiodriver.in4"),   CFSTR("com.loomix.audiodriver.in5"),   CFSTR("com.loomix.audiodriver.in6"),
    CFSTR("com.loomix.audiodriver.in7"),   CFSTR("com.loomix.audiodriver.in8"),   CFSTR("com.loomix.audiodriver.outA1"),
    CFSTR("com.loomix.audiodriver.outA2"), CFSTR("com.loomix.audiodriver.outA3"), CFSTR("com.loomix.audiodriver.outA4"),
    CFSTR("com.loomix.audiodriver.outA5"), CFSTR("com.loomix.audiodriver.outB1"), CFSTR("com.loomix.audiodriver.outB2"),
    CFSTR("com.loomix.audiodriver.outB3"),
};

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

static LoomixDriverState gState;
static Boolean gBasicStateInitialized = false;

#define kZeroTimeStampPeriod ((UInt32)16384)

typedef struct
{
    UInt32 mDeviceIndex;
    Float64 mNewSampleRate;
    UInt32 mNewChannelCount;
} LoomixConfigurationChange;

#pragma mark Object ID helpers

/* An object ID maps to a device if it's one of kLoomixDeviceCount evenly
 * spaced device slots; to a stream if it's one of the two slots right
 * after a device's. Centralizing this arithmetic here, rather than
 * repeating range checks at each of the ~10 call sites that need it,
 * means the object numbering scheme in LoomixAudioDriver.h only has one
 * place that interprets it. */
static Boolean DeviceIndexForObjectID(AudioObjectID inObjectID, UInt32 *outIndex)
{
    if (inObjectID < kLoomixFirstDeviceObjectID)
    {
        return false;
    }
    UInt32 offset = inObjectID - kLoomixFirstDeviceObjectID;
    if (offset % kLoomixObjectsPerDevice != 0)
    {
        return false;
    }
    UInt32 index = offset / kLoomixObjectsPerDevice;
    if (index >= kLoomixDeviceCount)
    {
        return false;
    }
    *outIndex = index;
    return true;
}

static Boolean StreamInfoForObjectID(AudioObjectID inObjectID, UInt32 *outDeviceIndex, Boolean *outIsInput)
{
    if (inObjectID < kLoomixFirstDeviceObjectID)
    {
        return false;
    }
    UInt32 offset = inObjectID - kLoomixFirstDeviceObjectID;
    UInt32 index = offset / kLoomixObjectsPerDevice;
    UInt32 remainder = offset % kLoomixObjectsPerDevice;
    if (index >= kLoomixDeviceCount || remainder == 0)
    {
        return false;
    }
    *outDeviceIndex = index;
    *outIsInput = remainder == 1;
    return true;
}

#pragma mark CFPlugIn factory

void *LoomixAudioDriver_Create(CFAllocatorRef inAllocator, CFUUIDRef inRequestedTypeUUID);
void *LoomixAudioDriver_Create(CFAllocatorRef inAllocator, CFUUIDRef inRequestedTypeUUID)
{
    (void)inAllocator;
    if (!CFEqual(inRequestedTypeUUID, kAudioServerPlugInTypeUUID))
    {
        return NULL;
    }
    if (!gBasicStateInitialized)
    {
        /* Only cheap, always-succeeding work here: field defaults and a
         * mutex init. No allocation happens for any device until that
         * specific device's first StartIO (EnsureDeviceRingAllocated) --
         * this factory function runs synchronously on coreaudiod's
         * plugin-loading path, and 16 devices eagerly calloc'ing ring
         * storage here, unconditionally, whether or not anything ever
         * uses them, is exactly the kind of front-loaded work a plugin
         * load path should not be doing (spec section 4.1's real-time
         * rules are about the IO callback specifically, but "don't do
         * heavy, checked-nowhere work on a path the host doesn't expect
         * to block" is the same principle one level up). */
        pthread_mutex_init(&gState.mStateMutex, NULL);
        for (UInt32 i = 0; i < kLoomixDeviceCount; i++)
        {
            LoomixDevice *device = &gState.mDevices[i];
            device->mSampleRate = kLoomixDefaultSampleRate;
            device->mChannelCount = kLoomixDefaultChannels;
            device->mExtraLatencyFrames = 0;
        }
        gBasicStateInitialized = true;
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

/* Every Loomix device is static, published at Initialize, not created by
 * the host dynamically -- CreateDevice/DestroyDevice are for the "create
 * a custom device from Audio MIDI Setup" flow, which this driver doesn't
 * support. */
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
    UInt32 deviceIndex;
    if (!DeviceIndexForObjectID(inDeviceObjectID, &deviceIndex) || inChangeInfo == NULL)
    {
        return kAudioHardwareBadObjectError;
    }

    LoomixConfigurationChange *change = (LoomixConfigurationChange *)inChangeInfo;
    LoomixDevice *device = &gState.mDevices[deviceIndex];
    pthread_mutex_lock(&gState.mStateMutex);
    device->mSampleRate = change->mNewSampleRate;
    device->mChannelCount = change->mNewChannelCount;
    LoomixRingBuffer_SetChannelCount(&device->mRing, change->mNewChannelCount);
    pthread_mutex_unlock(&gState.mStateMutex);
    free(change);

    if (gState.mHost != NULL)
    {
        AudioObjectPropertyAddress changed[] = {
            {kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain},
            {kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain},
            {kAudioStreamPropertyPhysicalFormat, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain},
        };
        gState.mHost->PropertiesChanged(gState.mHost, inDeviceObjectID, 3, changed);
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

static AudioStreamBasicDescription CurrentStreamFormat(const LoomixDevice *inDevice)
{
    AudioStreamBasicDescription format;
    memset(&format, 0, sizeof(format));
    format.mSampleRate = inDevice->mSampleRate;
    format.mFormatID = kAudioFormatLinearPCM;
    format.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
    format.mBytesPerPacket = inDevice->mChannelCount * sizeof(Float32);
    format.mFramesPerPacket = 1;
    format.mBytesPerFrame = inDevice->mChannelCount * sizeof(Float32);
    format.mChannelsPerFrame = inDevice->mChannelCount;
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

    UInt32 deviceIndex;
    Boolean isInputStream;
    Boolean isDeviceObject = DeviceIndexForObjectID(inObjectID, &deviceIndex);
    Boolean isStreamObject = StreamInfoForObjectID(inObjectID, &deviceIndex, &isInputStream);

    Boolean isDeviceSampleRate = isDeviceObject && inAddress->mSelector == kAudioDevicePropertyNominalSampleRate;
    Boolean isDeviceLatency = isDeviceObject && inAddress->mSelector == kAudioDevicePropertySafetyOffset;
    Boolean isStreamFormat = isStreamObject &&
                              (inAddress->mSelector == kAudioStreamPropertyVirtualFormat || inAddress->mSelector == kAudioStreamPropertyPhysicalFormat);
    *outIsSettable = isDeviceSampleRate || isDeviceLatency || isStreamFormat;
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
    UInt32 deviceIndex;
    Boolean isInputStream;

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
            *outDataSize = kLoomixDeviceCount * sizeof(AudioObjectID);
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
    else if (DeviceIndexForObjectID(inObjectID, &deviceIndex))
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
        case kAudioObjectPropertyCustomPropertyInfoList:
            *outDataSize = sizeof(AudioServerPlugInCustomPropertyInfo);
            return kAudioHardwareNoError;
        case kLoomixCustomProperty_BufferStatistics:
            *outDataSize = sizeof(CFPropertyListRef);
            return kAudioHardwareNoError;
        default:
            return kAudioHardwareUnknownPropertyError;
        }
    }
    else if (StreamInfoForObjectID(inObjectID, &deviceIndex, &isInputStream))
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
    UInt32 deviceIndex;
    Boolean isInputStream;

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
        {
            UInt32 maxCount = inDataSize / sizeof(AudioObjectID);
            UInt32 count = maxCount < kLoomixDeviceCount ? maxCount : (UInt32)kLoomixDeviceCount;
            AudioObjectID *ids = (AudioObjectID *)outData;
            for (UInt32 i = 0; i < count; i++)
            {
                ids[i] = kLoomixDeviceObjectID(i);
            }
            *outDataSize = count * sizeof(AudioObjectID);
            return kAudioHardwareNoError;
        }
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
                if (uid != NULL)
                {
                    for (UInt32 i = 0; i < kLoomixDeviceCount; i++)
                    {
                        if (CFEqual(uid, kLoomixDeviceUIDs[i]))
                        {
                            result = kLoomixDeviceObjectID(i);
                            break;
                        }
                    }
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
    else if (DeviceIndexForObjectID(inObjectID, &deviceIndex))
    {
        const LoomixDevice *device = &gState.mDevices[deviceIndex];
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
            *(CFStringRef *)outData = kLoomixDeviceNames[deviceIndex];
            *outDataSize = sizeof(CFStringRef);
            return kAudioHardwareNoError;
        case kAudioObjectPropertyManufacturer:
            *(CFStringRef *)outData = CFSTR("Loomix");
            *outDataSize = sizeof(CFStringRef);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyDeviceUID:
            *(CFStringRef *)outData = kLoomixDeviceUIDs[deviceIndex];
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
                ids[0] = kLoomixStreamInputObjectID(deviceIndex);
                ids[1] = kLoomixStreamOutputObjectID(deviceIndex);
                count = 2;
            }
            else if (inAddress->mScope == kAudioObjectPropertyScopeInput)
            {
                if (inDataSize < sizeof(AudioObjectID))
                {
                    return kAudioHardwareBadPropertySizeError;
                }
                ids[0] = kLoomixStreamInputObjectID(deviceIndex);
                count = 1;
            }
            else
            {
                if (inDataSize < sizeof(AudioObjectID))
                {
                    return kAudioHardwareBadPropertySizeError;
                }
                ids[0] = kLoomixStreamOutputObjectID(deviceIndex);
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
            *(AudioObjectID *)outData = kLoomixDeviceObjectID(deviceIndex);
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
             * block partway through an otherwise bit-exact capture.
             * mExtraLatencyFrames is the M2 "configurable driver side
             * latency" on top of that fixed minimum, settable per device
             * (spec section 3.4 M2, and the virtual driver control panel
             * in spec section 1.17). */
            *(UInt32 *)outData =
                inAddress->mScope == kAudioObjectPropertyScopeInput ? kLoomixInputSafetyOffsetFrames + device->mExtraLatencyFrames : 0;
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
            *(UInt32 *)outData = device->mIsRunning ? 1 : 0;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyZeroTimeStampPeriod:
            *(UInt32 *)outData = kZeroTimeStampPeriod;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        case kAudioDevicePropertyNominalSampleRate:
            *(Float64 *)outData = device->mSampleRate;
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
        case kAudioObjectPropertyCustomPropertyInfoList:
        {
            if (inDataSize < sizeof(AudioServerPlugInCustomPropertyInfo))
            {
                return kAudioHardwareBadPropertySizeError;
            }
            AudioServerPlugInCustomPropertyInfo *info = (AudioServerPlugInCustomPropertyInfo *)outData;
            info[0].mSelector = kLoomixCustomProperty_BufferStatistics;
            info[0].mPropertyDataType = kAudioServerPlugInCustomPropertyDataTypeCFPropertyList;
            info[0].mQualifierDataType = kAudioServerPlugInCustomPropertyDataTypeNone;
            *outDataSize = sizeof(AudioServerPlugInCustomPropertyInfo);
            return kAudioHardwareNoError;
        }
        case kLoomixCustomProperty_BufferStatistics:
        {
            if (inDataSize < sizeof(CFPropertyListRef))
            {
                return kAudioHardwareBadPropertySizeError;
            }
            /* A fresh CFDictionary per query -- this is diagnostic data
             * read occasionally by a control app, not part of the audio
             * IO path, so building it isn't real-time-safety-sensitive.
             * Ownership transfers to the caller, matching the convention
             * documented for kAudioPlugInPropertyResourceBundle. */
            SInt64 writeCursor = (SInt64)(uint64_t)device->mRing.writeCursorSampleTime;
            SInt64 writeDiscontinuities = (SInt64)(uint64_t)device->mRing.writeDiscontinuityCount;
            SInt64 readDiscontinuities = (SInt64)(uint64_t)device->mRing.readDiscontinuityCount;
            SInt64 ringCapacityFrames = (SInt64)kLoomixRingBufferFrameCapacity;

            CFNumberRef values[4] = {
                CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt64Type, &writeCursor),
                CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt64Type, &writeDiscontinuities),
                CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt64Type, &readDiscontinuities),
                CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt64Type, &ringCapacityFrames),
            };
            CFStringRef keys[4] = {CFSTR("WriteCursorSampleTime"), CFSTR("WriteDiscontinuities"), CFSTR("ReadDiscontinuities"),
                                    CFSTR("RingCapacityFrames")};

            CFDictionaryRef stats = NULL;
            Boolean allValuesCreated = values[0] != NULL && values[1] != NULL && values[2] != NULL && values[3] != NULL;
            if (allValuesCreated)
            {
                stats = CFDictionaryCreate(kCFAllocatorDefault, (const void **)keys, (const void **)values, 4, &kCFTypeDictionaryKeyCallBacks,
                                            &kCFTypeDictionaryValueCallBacks);
            }
            for (UInt32 i = 0; i < 4; i++)
            {
                if (values[i] != NULL)
                {
                    CFRelease(values[i]);
                }
            }
            if (stats == NULL)
            {
                return kAudioHardwareUnspecifiedError;
            }

            *(CFPropertyListRef *)outData = stats;
            *outDataSize = sizeof(CFPropertyListRef);
            return kAudioHardwareNoError;
        }
        default:
            return kAudioHardwareUnknownPropertyError;
        }
    }
    else if (StreamInfoForObjectID(inObjectID, &deviceIndex, &isInputStream))
    {
        const LoomixDevice *device = &gState.mDevices[deviceIndex];
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
            *(AudioObjectID *)outData = kLoomixDeviceObjectID(deviceIndex);
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
            *(UInt32 *)outData = isInputStream ? 1 : 0;
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
            *(AudioStreamBasicDescription *)outData = CurrentStreamFormat(device);
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
    (void)inDriver;
    (void)inClientProcessID;
    (void)inQualifierDataSize;
    (void)inQualifierData;

    UInt32 deviceIndex;
    Boolean isInputStream;

    if (DeviceIndexForObjectID(inObjectID, &deviceIndex) && inAddress->mSelector == kAudioDevicePropertyNominalSampleRate)
    {
        LoomixDevice *device = &gState.mDevices[deviceIndex];
        if (inDataSize != sizeof(Float64))
        {
            return kAudioHardwareBadPropertySizeError;
        }
        Float64 requested = *(const Float64 *)inData;
        if (!FormatIsSupported(requested, device->mChannelCount))
        {
            return kAudioDeviceUnsupportedFormatError;
        }
        if (requested == device->mSampleRate)
        {
            return kAudioHardwareNoError;
        }

        LoomixConfigurationChange *change = (LoomixConfigurationChange *)LOOMIX_MALLOC(sizeof(LoomixConfigurationChange));
        if (change == NULL)
        {
            return kAudioHardwareUnspecifiedError;
        }
        change->mDeviceIndex = deviceIndex;
        change->mNewSampleRate = requested;
        change->mNewChannelCount = device->mChannelCount;
        gState.mHost->RequestDeviceConfigurationChange(gState.mHost, inObjectID, 0, change);
        return kAudioHardwareNoError;
    }
    else if (DeviceIndexForObjectID(inObjectID, &deviceIndex) && inAddress->mSelector == kAudioDevicePropertySafetyOffset)
    {
        /* The M2 "configurable driver side latency" knob: extra frames of
         * input-scope safety margin on top of the fixed minimum. Applied
         * directly rather than through RequestDeviceConfigurationChange,
         * since it doesn't change the stream format or ring buffer shape
         * -- only how far behind the write side the host schedules reads
         * -- so there's no IO state to tear down and rebuild around it. */
        if (inDataSize != sizeof(UInt32))
        {
            return kAudioHardwareBadPropertySizeError;
        }
        gState.mDevices[deviceIndex].mExtraLatencyFrames = *(const UInt32 *)inData;
        if (gState.mHost != NULL)
        {
            AudioObjectPropertyAddress changed = {kAudioDevicePropertySafetyOffset, kAudioObjectPropertyScopeInput,
                                                    kAudioObjectPropertyElementMain};
            gState.mHost->PropertiesChanged(gState.mHost, inObjectID, 1, &changed);
        }
        return kAudioHardwareNoError;
    }
    else if (StreamInfoForObjectID(inObjectID, &deviceIndex, &isInputStream) &&
             (inAddress->mSelector == kAudioStreamPropertyVirtualFormat || inAddress->mSelector == kAudioStreamPropertyPhysicalFormat))
    {
        LoomixDevice *device = &gState.mDevices[deviceIndex];
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
        if (requested->mSampleRate == device->mSampleRate && requested->mChannelsPerFrame == device->mChannelCount)
        {
            return kAudioHardwareNoError;
        }

        LoomixConfigurationChange *change = (LoomixConfigurationChange *)LOOMIX_MALLOC(sizeof(LoomixConfigurationChange));
        if (change == NULL)
        {
            return kAudioHardwareUnspecifiedError;
        }
        change->mDeviceIndex = deviceIndex;
        change->mNewSampleRate = requested->mSampleRate;
        change->mNewChannelCount = requested->mChannelsPerFrame;
        gState.mHost->RequestDeviceConfigurationChange(gState.mHost, kLoomixDeviceObjectID(deviceIndex), 0, change);
        return kAudioHardwareNoError;
    }
    return kAudioHardwareUnknownPropertyError;
}

#pragma mark IO operations

/* Allocates and initializes a device's ring buffer on its first use, not
 * for every device unconditionally at driver load (see
 * LoomixAudioDriver_Create). Idempotent: a device already allocated is a
 * no-op. Returns false, changing nothing, if calloc fails -- the caller
 * (StartIO) is expected to fail that one call cleanly rather than let a
 * null ring buffer pointer reach DoIOOperation. */
static Boolean EnsureDeviceRingAllocated(LoomixDevice *device)
{
    if (device->mRingStorage != NULL)
    {
        return true;
    }
    float *storage = (float *)LOOMIX_CALLOC((size_t)kLoomixRingBufferFrameCapacity * kLoomixMaxChannels, sizeof(float));
    if (storage == NULL)
    {
        return false;
    }
    device->mRingStorage = storage;
    LoomixRingBuffer_Init(&device->mRing, storage, device->mChannelCount);
    return true;
}

static OSStatus LoomixDriver_StartIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID)
{
    (void)inDriver;
    (void)inClientID;
    UInt32 deviceIndex;
    if (!DeviceIndexForObjectID(inDeviceObjectID, &deviceIndex))
    {
        return kAudioHardwareBadObjectError;
    }
    LoomixDevice *device = &gState.mDevices[deviceIndex];
    if (!EnsureDeviceRingAllocated(device))
    {
        return kAudioHardwareUnspecifiedError;
    }
    if (__sync_add_and_fetch(&device->mIORunningClients, 1) == 1)
    {
        device->mStartHostTime = mach_absolute_time();
        LoomixRingBuffer_Reset(&device->mRing);
        device->mIsRunning = true;
    }
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_StopIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID)
{
    (void)inDriver;
    (void)inClientID;
    UInt32 deviceIndex;
    if (!DeviceIndexForObjectID(inDeviceObjectID, &deviceIndex))
    {
        return kAudioHardwareBadObjectError;
    }
    LoomixDevice *device = &gState.mDevices[deviceIndex];
    if (__sync_sub_and_fetch(&device->mIORunningClients, 1) == 0)
    {
        device->mIsRunning = false;
    }
    return kAudioHardwareNoError;
}

static OSStatus LoomixDriver_GetZeroTimeStamp(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID,
                                               Float64 *outSampleTime, UInt64 *outHostTime, UInt64 *outSeed)
{
    (void)inDriver;
    (void)inClientID;
    UInt32 deviceIndex;
    if (!DeviceIndexForObjectID(inDeviceObjectID, &deviceIndex))
    {
        return kAudioHardwareBadObjectError;
    }
    const LoomixDevice *device = &gState.mDevices[deviceIndex];

    static mach_timebase_info_data_t timebase = {0, 0};
    if (timebase.denom == 0)
    {
        mach_timebase_info(&timebase);
    }

    UInt64 now = mach_absolute_time();
    UInt64 elapsedTicks = now - device->mStartHostTime;
    Float64 elapsedSeconds = (Float64)elapsedTicks * (Float64)timebase.numer / (Float64)timebase.denom / 1e9;
    Float64 elapsedFrames = elapsedSeconds * device->mSampleRate;

    UInt64 periodIndex = (UInt64)elapsedFrames / kZeroTimeStampPeriod;
    Float64 sampleTime = (Float64)(periodIndex * kZeroTimeStampPeriod);
    Float64 periodSeconds = (Float64)kZeroTimeStampPeriod / device->mSampleRate;
    UInt64 hostTicksPerPeriod = (UInt64)(periodSeconds * 1e9 * (Float64)timebase.denom / (Float64)timebase.numer);

    *outSampleTime = sampleTime;
    *outHostTime = device->mStartHostTime + periodIndex * hostTicksPerPeriod;
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
    UInt32 deviceIndex;
    if (!DeviceIndexForObjectID(inDeviceObjectID, &deviceIndex) || ioMainBuffer == NULL)
    {
        return kAudioHardwareBadObjectError;
    }
    LoomixDevice *device = &gState.mDevices[deviceIndex];
    if (device->mRingStorage == NULL)
    {
        /* StartIO is required to have allocated this before IO begins;
         * this is defensive insurance against reaching here any other
         * way, not the expected path -- returning cleanly instead of
         * dereferencing a null ring buffer either way. */
        return kAudioHardwareNotRunningError;
    }

    if (inOperationID == kAudioServerPlugInIOOperationWriteMix && inStreamObjectID == kLoomixStreamOutputObjectID(deviceIndex))
    {
        LoomixRingBuffer_Write(&device->mRing, inIOCycleInfo->mOutputTime.mSampleTime, inIOBufferFrameSize, (const Float32 *)ioMainBuffer);
    }
    else if (inOperationID == kAudioServerPlugInIOOperationReadInput && inStreamObjectID == kLoomixStreamInputObjectID(deviceIndex))
    {
        LoomixRingBuffer_Read(&device->mRing, inIOCycleInfo->mInputTime.mSampleTime, inIOBufferFrameSize, (Float32 *)ioMainBuffer);
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
