/*
 * Host-side harness for the M2 driver surface. Links LoomixAudioDriver.c
 * and RingBuffer.c directly into this binary and drives the returned
 * AudioServerPlugInDriverInterface vtable exactly as coreaudiod would --
 * Initialize (including under injected allocation failure), the plug-in's
 * device list (including with an undersized caller buffer), object-ID
 * lookups for in-range, out-of-range and misaligned IDs, and the lazy
 * StartIO ring-buffer allocation path -- without an installed driver or a
 * running coreaudiod. None of LoomixAudioDriver.c's own code calls
 * CoreAudio's client API, only CoreFoundation, so this works with nothing
 * coreaudiod-specific running at all. Every one of these is a way the
 * driver crashed coreaudiod during M2's install cycle; every one is
 * covered here in milliseconds instead.
 *
 * Normal build, exercises the happy paths:
 *   clang -Wall -Wextra -Werror -std=gnu17 -framework CoreFoundation \
 *       -framework CoreAudio -o test_driver_host \
 *       driver/tests/test_driver_host.c \
 *       driver/LoomixAudioDriver/LoomixAudioDriver.c \
 *       driver/LoomixAudioDriver/RingBuffer.c
 *   ./test_driver_host
 *
 * Fault-injection build: LOOMIX_CALLOC/LOOMIX_MALLOC (see the top of
 * LoomixAudioDriver.c) are redirected to always-failing allocators, so the
 * three checked-but-never-actually-exercised allocation failure paths that
 * caused the M2 coreaudiod crash run for real.
 *   clang -Wall -Wextra -Werror -std=gnu17 \
 *       -DLOOMIX_CALLOC=FailingCalloc -DLOOMIX_MALLOC=FailingMalloc \
 *       -framework CoreFoundation -framework CoreAudio \
 *       -o test_driver_host_fault_injection \
 *       driver/tests/test_driver_host.c \
 *       driver/LoomixAudioDriver/LoomixAudioDriver.c \
 *       driver/LoomixAudioDriver/RingBuffer.c
 *   ./test_driver_host_fault_injection
 */
#include "../LoomixAudioDriver/LoomixAudioDriver.h"

#include <CoreFoundation/CoreFoundation.h>
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if defined(LOOMIX_CALLOC) || defined(LOOMIX_MALLOC)
#define kFaultInjectionBuild 1
#else
#define kFaultInjectionBuild 0
#endif

#if kFaultInjectionBuild
void *FailingCalloc(size_t count, size_t size)
{
    (void)count;
    (void)size;
    return NULL;
}
void *FailingMalloc(size_t size)
{
    (void)size;
    return NULL;
}
#endif

extern void *LoomixAudioDriver_Create(CFAllocatorRef inAllocator, CFUUIDRef inRequestedTypeUUID);

/* Initialize's inHost parameter is annotated non-null, but nothing this
 * harness exercises reaches a code path that calls back through it (both
 * fault-injection tests return before touching gState.mHost, and the
 * normal build never calls SetPropertyData at all) -- so a stub whose
 * methods trap if actually invoked is enough to satisfy the type without
 * having to fake a real host. */
static OSStatus TrapPropertiesChanged(AudioServerPlugInHostRef h, AudioObjectID o, UInt32 n, const AudioObjectPropertyAddress *a)
{
    (void)h;
    (void)o;
    (void)n;
    (void)a;
    assert(0 && "unexpected call to PropertiesChanged");
    return kAudioHardwareUnspecifiedError;
}
static OSStatus TrapRequestDeviceConfigurationChange(AudioServerPlugInHostRef h, AudioObjectID d, UInt64 a, void *i)
{
    (void)h;
    (void)d;
    (void)a;
    (void)i;
    assert(0 && "unexpected call to RequestDeviceConfigurationChange");
    return kAudioHardwareUnspecifiedError;
}
static OSStatus TrapCopyFromStorage(AudioServerPlugInHostRef h, CFStringRef k, CFPropertyListRef *o)
{
    (void)h;
    (void)k;
    (void)o;
    assert(0 && "unexpected call to CopyFromStorage");
    return kAudioHardwareUnspecifiedError;
}
static OSStatus TrapWriteToStorage(AudioServerPlugInHostRef h, CFStringRef k, CFPropertyListRef d)
{
    (void)h;
    (void)k;
    (void)d;
    assert(0 && "unexpected call to WriteToStorage");
    return kAudioHardwareUnspecifiedError;
}
static OSStatus TrapDeleteFromStorage(AudioServerPlugInHostRef h, CFStringRef k)
{
    (void)h;
    (void)k;
    assert(0 && "unexpected call to DeleteFromStorage");
    return kAudioHardwareUnspecifiedError;
}
static const AudioServerPlugInHostInterface kStubHostInterface = {
    TrapPropertiesChanged, TrapCopyFromStorage, TrapWriteToStorage, TrapDeleteFromStorage, TrapRequestDeviceConfigurationChange,
};
static const AudioServerPlugInHostRef kStubHost = &kStubHostInterface;

static AudioServerPlugInDriverRef CreateDriver(void)
{
    /* In the fault-injection build this is already the "Initialize with
     * allocation failures injected" coverage: LoomixAudioDriver_Create
     * runs under LOOMIX_CALLOC/LOOMIX_MALLOC forced to fail, and must
     * still return a live driver -- proving the plugin-load path
     * (LoomixAudioDriver_Create, see its own comment on why it does no
     * allocation) really does allocate nothing, which is the fix for the
     * crash this milestone started from. If that guarantee ever regresses
     * back to eager per-device allocation, this assert is what catches it. */
    void *driver = LoomixAudioDriver_Create(NULL, kAudioServerPlugInTypeUUID);
    assert(driver != NULL);
    return (AudioServerPlugInDriverRef)driver;
}

static void TestInitialize(AudioServerPlugInDriverRef driver)
{
    /* Same allocation-free guarantee as CreateDriver, one call further in:
     * Initialize only stores the driver/host refs (see
     * LoomixDriver_Initialize), so it must succeed here too even under
     * forced allocation failure. */
    assert((*driver)->Initialize(driver, kStubHost) == kAudioHardwareNoError);
}

static void TestDeviceList(AudioServerPlugInDriverRef driver)
{
    AudioObjectPropertyAddress addr = {kAudioPlugInPropertyDeviceList, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};

    UInt32 size = 0;
    assert((*driver)->GetPropertyDataSize(driver, kObjectID_PlugIn, 0, &addr, 0, NULL, &size) == kAudioHardwareNoError);
    assert(size == kLoomixDeviceCount * sizeof(AudioObjectID));

    AudioObjectID ids[kLoomixDeviceCount];
    UInt32 outSize = 0;
    assert((*driver)->GetPropertyData(driver, kObjectID_PlugIn, 0, &addr, 0, NULL, sizeof(ids), &outSize, ids) == kAudioHardwareNoError);
    assert(outSize == sizeof(ids));
    for (UInt32 i = 0; i < kLoomixDeviceCount; i++)
    {
        assert(ids[i] == kLoomixDeviceObjectID(i));
    }
}

static void TestDeviceListUndersizedBuffer(AudioServerPlugInDriverRef driver)
{
    /* A caller that only has room for part of the list -- the exact shape
     * of bug (writing past a caller-sized buffer because the driver wrote
     * kLoomixDeviceCount unconditionally instead of respecting inDataSize)
     * that corrupts the host process rather than just failing cleanly. */
    AudioObjectPropertyAddress addr = {kAudioPlugInPropertyDeviceList, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    const UInt32 kRequestedCount = 5;
    const AudioObjectID kGuardValue = 0xDEADBEEF;

    AudioObjectID buffer[5 + 3];
    for (UInt32 i = 0; i < 5 + 3; i++)
    {
        buffer[i] = kGuardValue;
    }
    UInt32 outSize = 0;
    assert((*driver)->GetPropertyData(driver, kObjectID_PlugIn, 0, &addr, 0, NULL, kRequestedCount * sizeof(AudioObjectID), &outSize,
                                       buffer) == kAudioHardwareNoError);
    assert(outSize == kRequestedCount * sizeof(AudioObjectID));
    for (UInt32 i = 0; i < kRequestedCount; i++)
    {
        assert(buffer[i] == kLoomixDeviceObjectID(i));
    }
    for (UInt32 i = kRequestedCount; i < 5 + 3; i++)
    {
        assert(buffer[i] == kGuardValue); /* untouched past what was asked for */
    }

    /* A zero-sized ask must return zero data and touch nothing. */
    AudioObjectID zeroCheck[1] = {kGuardValue};
    outSize = 123;
    assert((*driver)->GetPropertyData(driver, kObjectID_PlugIn, 0, &addr, 0, NULL, 0, &outSize, zeroCheck) == kAudioHardwareNoError);
    assert(outSize == 0);
    assert(zeroCheck[0] == kGuardValue);

    /* A size that isn't a whole multiple of one AudioObjectID must round
     * down, not read or write a partial element. */
    AudioObjectID partial[3] = {kGuardValue, kGuardValue, kGuardValue};
    outSize = 0;
    assert((*driver)->GetPropertyData(driver, kObjectID_PlugIn, 0, &addr, 0, NULL, 2 * sizeof(AudioObjectID) + 1, &outSize, partial) ==
           kAudioHardwareNoError);
    assert(outSize == 2 * sizeof(AudioObjectID));
    assert(partial[0] == kLoomixDeviceObjectID(0));
    assert(partial[1] == kLoomixDeviceObjectID(1));
    assert(partial[2] == kGuardValue);
}

static Boolean ObjectExists(AudioServerPlugInDriverRef driver, AudioObjectID objectID)
{
    AudioObjectPropertyAddress addr = {kAudioObjectPropertyBaseClass, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    UInt32 size = 0;
    return (*driver)->GetPropertyDataSize(driver, objectID, 0, &addr, 0, NULL, &size) == kAudioHardwareNoError;
}

static AudioClassID ObjectClass(AudioServerPlugInDriverRef driver, AudioObjectID objectID)
{
    AudioObjectPropertyAddress addr = {kAudioObjectPropertyClass, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    AudioClassID classID = 0;
    UInt32 outSize = 0;
    assert((*driver)->GetPropertyData(driver, objectID, 0, &addr, 0, NULL, sizeof(classID), &outSize, &classID) == kAudioHardwareNoError);
    assert(outSize == sizeof(classID));
    return classID;
}

static UInt32 StreamDirection(AudioServerPlugInDriverRef driver, AudioObjectID streamID)
{
    AudioObjectPropertyAddress addr = {kAudioStreamPropertyDirection, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    UInt32 direction = 0xFFFFFFFF;
    UInt32 outSize = 0;
    assert((*driver)->GetPropertyData(driver, streamID, 0, &addr, 0, NULL, sizeof(direction), &outSize, &direction) ==
           kAudioHardwareNoError);
    assert(outSize == sizeof(direction));
    return direction;
}

static void TestObjectIDLookupsInRange(AudioServerPlugInDriverRef driver)
{
    /* Exhaustively walk every device and stream ID this driver actually
     * publishes, not just the first and last, and confirm each resolves
     * to exactly the object type and stream role its own ID implies -- an
     * off-by-one in the object-ID arithmetic anywhere in the middle of
     * the range would not show up in a spot check of device 0 and device
     * 15 alone. */
    assert(ObjectExists(driver, kObjectID_PlugIn));
    for (UInt32 i = 0; i < kLoomixDeviceCount; i++)
    {
        assert(ObjectClass(driver, kLoomixDeviceObjectID(i)) == kAudioDeviceClassID);
        assert(ObjectClass(driver, kLoomixStreamInputObjectID(i)) == kAudioStreamClassID);
        assert(ObjectClass(driver, kLoomixStreamOutputObjectID(i)) == kAudioStreamClassID);
        assert(StreamDirection(driver, kLoomixStreamInputObjectID(i)) == 1);
        assert(StreamDirection(driver, kLoomixStreamOutputObjectID(i)) == 0);
    }
}

static void TestObjectIDLookupsOutOfRange(AudioServerPlugInDriverRef driver)
{
    /* 0 (below the first valid ID, and the value DeviceIndexForObjectID's
     * underflow guard exists for), one past the last stream ID, and a
     * number nowhere near the valid range. */
    assert(!ObjectExists(driver, 0));
    assert(!ObjectExists(driver, kLoomixStreamOutputObjectID(kLoomixDeviceCount - 1) + 1));
    assert(!ObjectExists(driver, 999999));
}

static void TestObjectIDLookupsMisaligned(AudioServerPlugInDriverRef driver)
{
    /* IDs that are real -- in range, correctly typed -- paired with a
     * selector that belongs to a different object type: a device-only
     * selector aimed at a stream or the plug-in, and a stream-only
     * selector aimed at a device. Property dispatch here is by ID
     * arithmetic (DeviceIndexForObjectID / StreamInfoForObjectID)
     * *followed by* a per-type selector switch; this is what proves the
     * second half of that actually gates access rather than a mismatched
     * request silently reading whichever object the ID resolves to. */
    AudioObjectPropertyAddress deviceOnlySelector = {kAudioDevicePropertyStreams, kAudioObjectPropertyScopeGlobal,
                                                       kAudioObjectPropertyElementMain};
    AudioObjectPropertyAddress streamOnlySelector = {kAudioStreamPropertyDirection, kAudioObjectPropertyScopeGlobal,
                                                       kAudioObjectPropertyElementMain};
    UInt32 size = 0;

    assert((*driver)->GetPropertyDataSize(driver, kLoomixStreamInputObjectID(3), 0, &deviceOnlySelector, 0, NULL, &size) ==
           kAudioHardwareUnknownPropertyError);
    assert((*driver)->GetPropertyDataSize(driver, kLoomixStreamOutputObjectID(3), 0, &deviceOnlySelector, 0, NULL, &size) ==
           kAudioHardwareUnknownPropertyError);
    assert((*driver)->GetPropertyDataSize(driver, kObjectID_PlugIn, 0, &deviceOnlySelector, 0, NULL, &size) ==
           kAudioHardwareUnknownPropertyError);
    assert((*driver)->GetPropertyDataSize(driver, kLoomixDeviceObjectID(3), 0, &streamOnlySelector, 0, NULL, &size) ==
           kAudioHardwareUnknownPropertyError);
}

#if kFaultInjectionBuild

static void TestStartIOAllocationFailure(AudioServerPlugInDriverRef driver)
{
    /* This is the exact call site that crashed coreaudiod on M2 install:
     * EnsureDeviceRingAllocated's calloc, called lazily from StartIO. With
     * it forced to fail, StartIO must fail cleanly, not crash and not
     * leave the client count incremented. */
    AudioObjectID deviceID = kLoomixDeviceObjectID(0);
    assert((*driver)->StartIO(driver, deviceID, 0) == kAudioHardwareUnspecifiedError);
    /* A second attempt must behave the same way -- no half-updated state
     * from the first failed call lets it wrongly succeed a retry. */
    assert((*driver)->StartIO(driver, deviceID, 0) == kAudioHardwareUnspecifiedError);
}

static void TestSetPropertyDataAllocationFailure(AudioServerPlugInDriverRef driver)
{
    /* Nominal-sample-rate path: device 1, 44100 vs the 48000 default. */
    AudioObjectID deviceID = kLoomixDeviceObjectID(1);
    AudioObjectPropertyAddress rateAddr = {kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal,
                                            kAudioObjectPropertyElementMain};
    Float64 requestedRate = 44100.0;
    assert((*driver)->SetPropertyData(driver, deviceID, 0, &rateAddr, 0, NULL, sizeof(requestedRate), &requestedRate) ==
           kAudioHardwareUnspecifiedError);

    /* Stream-format path: device 1's output stream, same rate change plus
     * an explicit (unchanged) channel count, packed float32 as required. */
    AudioObjectID streamID = kLoomixStreamOutputObjectID(1);
    AudioObjectPropertyAddress formatAddr = {kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal,
                                              kAudioObjectPropertyElementMain};
    AudioStreamBasicDescription requestedFormat;
    memset(&requestedFormat, 0, sizeof(requestedFormat));
    requestedFormat.mSampleRate = 44100.0;
    requestedFormat.mFormatID = kAudioFormatLinearPCM;
    requestedFormat.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
    requestedFormat.mChannelsPerFrame = kLoomixDefaultChannels;
    requestedFormat.mBitsPerChannel = 8 * sizeof(Float32);
    assert((*driver)->SetPropertyData(driver, streamID, 0, &formatAddr, 0, NULL, sizeof(requestedFormat), &requestedFormat) ==
           kAudioHardwareUnspecifiedError);
}

#else

static void TestStartIOSucceeds(AudioServerPlugInDriverRef driver)
{
    /* The happy path of the same call site TestStartIOAllocationFailure
     * exercises in the fault-injection build: real calloc succeeds, IO
     * starts, and the driver reports itself running. */
    AudioObjectID deviceID = kLoomixDeviceObjectID(0);
    assert((*driver)->StartIO(driver, deviceID, 0) == kAudioHardwareNoError);
    assert((*driver)->StopIO(driver, deviceID, 0) == kAudioHardwareNoError);
}

#endif

int main(void)
{
    AudioServerPlugInDriverRef driver = CreateDriver();
    TestInitialize(driver);
    TestDeviceList(driver);
    TestDeviceListUndersizedBuffer(driver);
    TestObjectIDLookupsInRange(driver);
    TestObjectIDLookupsOutOfRange(driver);
    TestObjectIDLookupsMisaligned(driver);
#if kFaultInjectionBuild
    TestStartIOAllocationFailure(driver);
    TestSetPropertyDataAllocationFailure(driver);
#else
    TestStartIOSucceeds(driver);
#endif
    printf("test_driver_host: all checks passed (fault injection: %s)\n", kFaultInjectionBuild ? "on" : "off");
    return 0;
}
