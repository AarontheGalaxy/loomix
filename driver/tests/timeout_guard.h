#ifndef LOOMIX_TIMEOUT_GUARD_H
#define LOOMIX_TIMEOUT_GUARD_H

/* Every tool in this directory queries the installed driver through
 * coreaudiod over Mach IPC, which has no built-in timeout of its own -- if
 * the daemon is wedged, the call blocks forever, and a background retry
 * loop piling more blocked calls onto a wedged daemon is exactly what took
 * coreaudiod and the whole audio subsystem down during M2 (see
 * docs/ARCHITECTURE.md). ArmTimeout kills the process with a clear message
 * and a non-zero exit instead of hanging. Call it first thing in main().
 */
#include <signal.h>
#include <stdlib.h>
#include <unistd.h>

/* 5s is comfortably longer than any of these queries takes against a
 * healthy coreaudiod (all measured well under half a second) but short
 * enough that a wedged daemon fails fast instead of compounding.
 * Overridable at runtime via LOOMIX_COREAUDIO_TIMEOUT_SECONDS so CI can
 * shorten it rather than let one wedged run eat its whole time budget. */
#define kLoomixDefaultCoreAudioTimeoutSeconds 5

static unsigned int LoomixTimeoutGuard_Seconds(void)
{
    const char *override = getenv("LOOMIX_COREAUDIO_TIMEOUT_SECONDS");
    if (override != NULL)
    {
        int parsed = atoi(override);
        if (parsed > 0)
        {
            return (unsigned int)parsed;
        }
    }
    return kLoomixDefaultCoreAudioTimeoutSeconds;
}

static void LoomixTimeoutGuard_Fire(int signum)
{
    (void)signum;
    static const char message[] = "error: timed out waiting on coreaudiod\n";
    write(STDERR_FILENO, message, sizeof(message) - 1); /* async-signal-safe; fprintf is not */
    _exit(1);
}

static void ArmTimeout(void)
{
    signal(SIGALRM, LoomixTimeoutGuard_Fire);
    alarm(LoomixTimeoutGuard_Seconds());
}

#endif /* LOOMIX_TIMEOUT_GUARD_H */
