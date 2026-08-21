# Architecture decision log

Decisions made during implementation that `docs/SPEC.md` leaves to
engineering judgement, dated, so the reasoning survives past the PR that
made them. `SPEC.md` remains the source of truth for anything it does
specify; this file never contradicts it.

## 2026-08-21 — rt-assert release-mode fix

**The real-time safety harness (spec 3.3 — "the single most valuable test
in the project") did not work in release builds, and nothing caught it.**
Discovered starting M3, not caused by it: `cargo test --workspace --release
--all-features` on plain `main` failed
`rt_assert::tests::realtime_guard_traps_allocation` — not by correctly
detecting the trapped allocation, but by the panic escaping
`catch_unwind` entirely and surfacing as an uncaught panic in that test's
own thread.

Root cause, confirmed with a standalone minimal reproduction outside this
crate (a `#[global_allocator]` whose `alloc()` panics when a thread-local
flag is set, wrapped in `std::panic::catch_unwind`): compiled with `rustc
-O`, the panic escapes `catch_unwind` and the process exits 101
uncaught; the identical program compiled without optimisation is caught
cleanly. Panicking from inside a `GlobalAlloc` method is not reliably
unwindable once LLVM optimises the allocator shim — a real, general Rust
behaviour, not a defect specific to this file. It was invisible until now
because `ci.yml`'s `test` job matrix (`profile: [debug, release]`, lifted
from spec 4.3's `ci.yml` skeleton verbatim) never actually passed
`--release` to the `--all-features` run on either matrix leg — both legs
ran the identical `cargo test --workspace --all-features` command, so the
"release" leg was decorative from M0 onward and every green run of it
asserted nothing.

The fix moves the panic out of the allocator entirely rather than trying
to make an allocator-internal panic unwind reliably (fighting the
underlying platform behaviour instead of avoiding it). `RtAssertAlloc`'s
methods now only record a thread-local "violation happened" flag and let
the real allocation proceed; `RealtimeGuard::drop` — ordinary code,
outside any allocator boundary — checks that flag and panics there
instead, once the guarded scope has already ended. This is caught
reliably under `catch_unwind` in every profile (verified by rerunning
`realtime_guard_traps_allocation` in both), and it's arguably a better
shape independent of the bug it fixes: the guarded scope always completes
normally rather than aborting mid-allocation partway through whatever it
was doing. `drop` skips its own panic when `std::thread::panicking()` is
already true, so a violation recorded during a callback that panics for
its own unrelated reason doesn't turn one panic into an unwind-aborting
double panic —
`rt_assert::tests::the_callbacks_own_panic_is_not_swallowed_by_a_concurrent_violation`
is the regression test for exactly that. The fix was verified negatively,
too: temporarily disabling the new panic in `drop` was confirmed to make
`realtime_guard_traps_allocation` itself fail in release mode, proving the
passing state means the trap actually fires rather than the assertion
having become vacuous.

`ci.yml`'s `test` job now gates its steps on `matrix.profile` so the two
legs are no longer identical: the `debug` leg runs the plain
`--all-features` suite, and the `release` leg runs that same suite with
`--release` plus, separately, the release-only ignored `golden` tests
(spec 4.1 layer 4) that were already release-gated before this fix.

## 2026-08-21 — M2

**Installing the M2 driver crashed `coreaudiod` (or wedged it badly enough
to look crashed); this is not a pre-existing daemon problem.** The
sequence, from the daemon's own log: `com.loomix.audiodriver.in1` was alive
and enumerable at device ID 2051 before the M2 install; after it, no
Loomix device appeared under any ID, the daemon never recovered on its
own, and it was later found pegged at 100%+ CPU, unresponsive even to
`system_profiler SPAudioDataType`, requiring a hard restart. `coreaudiod`
was healthy immediately before the install and unhealthy immediately
after, on the same machine, with nothing else changing in between — that
is the M2 driver's fault, stated plainly, not something to hedge as
"the daemon acting up."

The leading (never independently instrumented inside a live `coreaudiod`,
since that would have meant more install cycles) suspect: M1's driver
initialized one device eagerly; M2's `Create` initialized sixteen, each
`calloc`ing a ~1.7 MB ring buffer (`kLoomixRingBufferFrameCapacity *
kLoomixMaxChannels` floats), unconditionally, synchronously, on the
daemon's plug-in-load path, with the `calloc` return value unchecked. An
audit turned up the same unchecked-allocation pattern in two more places
that had never actually been exercised: two `malloc(sizeof
(LoomixConfigurationChange))` calls in `SetPropertyData`'s sample-rate and
stream-format change paths, and the buffer-statistics custom property's
`CFNumberCreate`/`CFDictionaryCreate` calls. All of these now check their
result and return `kAudioHardwareUnspecifiedError` on failure instead of
dereferencing NULL. Independent of whether allocation failure was the
actual trigger, eager per-device allocation at load time was itself the
wrong shape: `EnsureDeviceRingAllocated` now allocates a device's ring
lazily, on that device's first `StartIO`, not sixteen times unconditionally
in `Create`. `kLoomixRingBufferFrameCapacity` also dropped from 384000 to
65536 frames — still comfortably above spec 1.19's 3x-engine-buffer floor
(6144 frames), but not sized for devices that mostly sit idle.

**Debugging this went through the installed driver twice, each costing a
system-wide audio interruption and twice leaving the machine in a bad
state — that stopped, in favor of a host-side harness, before it became a
third.** `LoomixAudioDriver_Create` is the plugin's one non-static exported
symbol, and none of `LoomixAudioDriver.c`'s own code calls CoreAudio's
client API (only CoreFoundation) — so `driver/tests/test_driver_host.c`
links `LoomixAudioDriver.c` and `RingBuffer.c` directly into a small binary
and drives the returned `AudioServerPlugInDriverInterface` vtable exactly
as `coreaudiod` would, entirely in-process, with no installed driver and
no daemon involved. It covers `Initialize`, the plug-in's device list
(including a deliberately undersized caller buffer, checked against
overflow with poisoned guard slots past what was asked for), object-ID
lookups for in-range IDs (exhaustively, all 48 device/stream objects, not
just the first and last), out-of-range IDs, and "misaligned" ones — a real
object ID paired with a property selector that belongs to a different
object type, proving property dispatch actually gates on both the ID *and*
the selector rather than one alone. A second build of the same source,
compiled with `-DLOOMIX_CALLOC=FailingCalloc -DLOOMIX_MALLOC=FailingMalloc`
(`LoomixAudioDriver.c` routes every allocation through the `LOOMIX_CALLOC`/
`LOOMIX_MALLOC` macros for exactly this reason), forces every allocation in
the driver to fail for real and confirms `Create`, `Initialize`, `StartIO`
and `SetPropertyData` all fail cleanly rather than crash — including
proving, as a regression guard, that `Create`/`Initialize` still allocate
nothing at all even under forced failure. Both builds run in `just test`
and CI's `driver` job. The installed driver is now the final confirmation
of a milestone already proven correct host-side, not the loop debugging
happens in.

**A background run of `count_loomix_devices` against a wedged `coreaudiod`
hung for 17 minutes, drove the daemon to 105% CPU, and forced a machine
restart — the second machine-down incident in the same day.** CoreAudio's
client API has no built-in timeout; a call into a wedged daemon blocks
forever, and running that in the background meant nothing was watching it
or able to kill it. Two changes came out of this, not one: `driver/tests/
timeout_guard.h` gives every tool that queries the installed driver
(`count_loomix_devices`, `query_device_stats`, `set_sample_rate`) a hard
wall-clock timeout (`alarm()`/`SIGALRM`, an async-signal-safe write to
stderr, `_exit(1)`) so a wedged daemon fails the tool in seconds instead of
hanging it forever; the default is 5 seconds, long enough that every one of
these queries finishes in under half a second against a healthy daemon,
short enough that CI (via the `LOOMIX_COREAUDIO_TIMEOUT_SECONDS`
environment override) doesn't have to burn its whole time budget waiting
on a wedged one. Separately: CoreAudio-touching commands run one at a
time, in the foreground, for the rest of this project — a background run
is what turned a single blocked call into an unwatched, unkillable one.

**`driver/scripts/install.sh` and `driver/tests/loopback_test.sh` both
fail loudly, not silently, when zero Loomix devices are visible.**
Previously a crashed or wedged daemon showed up only as a confusing
downstream failure — an `ffmpeg` device-index lookup coming up empty, a
30-second capture timing out — far from the actual cause.
`count_loomix_devices` (built fresh by both scripts, with the same timeout
guard) prints the count and exits non-zero on zero; `install.sh` polls it
for up to 15 seconds after restarting `coreaudiod` (a fixed `sleep 3` was
tried first and rejected — a restart under load can legitimately take
longer than any single guess, and guessing too short misreports a healthy
install as a crash) and now exits non-zero itself if the count is still
zero at the deadline, rather than printing a warning and reporting success
anyway.

## 2026-08-21 — M1

**The loopback test does not assert bit-exactness from sample zero, and that's deliberate — the full arc, so this isn't misread as a lowered bar later.**

M1's acceptance text calls for a bit-exact loopback test. Getting there took several wrong turns worth recording so the final shape doesn't look arbitrary:

1. *First bug, real.* The initial implementation gave `WriteMix`/`ReadInput` a shared ring buffer with `SafetyOffset = 0` and no check on what `ReadInput` was about to hand back. A loopback test (sine tone, byte-for-byte substring search) showed a clean, repeating pattern: a chunk of already-delivered audio reappearing verbatim a few dozen frames later. Diagnosis: with zero scheduling margin, the host would occasionally schedule a read for a ring slot the matching write hadn't reached yet that cycle, and the read returned whatever stale data was already sitting there from a previous lap.

2. *First fix, incomplete.* Set `kAudioDevicePropertySafetyOffset` to 512 input-scope frames, giving `WriteMix` a guaranteed head start. This reduced the failure rate but didn't eliminate it — a longer test still occasionally reproduced the same stale-repeat signature. A fixed safety offset is a scheduling *hint* to the host, not a guarantee; it doesn't change what `ReadInput` does when the hint isn't enough.

3. *Actual root cause.* The real defect was in `ReadRingBuffer` (now `LoomixRingBuffer_Read`) itself: it copied from the ring unconditionally, with no way to distinguish "this slot holds this cycle's fresh write" from "this slot holds a previous lap's write, or nothing at all." The fix adds a write high-water mark (`writeCursorSampleTime`) that `Read` checks per frame: anything at or past it gets silence, never the ring's raw storage. This is a correctness fix by construction, not a tuned margin — it holds regardless of how much or little scheduling slack the host gives it. The safety offset stayed at 512 frames as a (still useful, now non-load-bearing) scheduling hint, and `driver/tests/test_ring_buffer.c`'s `test_unwritten_region_is_silent_not_poison` is the regression test: it poisons the ring's storage with non-zero sentinel values first, so the test would fail if silence ever came from the memory happening to already be zero rather than from the cursor check.

4. *The harness turned out to be the next confound, twice.* After the real fix, a longer automated test (ffmpeg's `avfoundation` capture process and `audiotoolbox` playback process, two independent client processes) still failed intermittently. Two separate findings, each confirmed the same way — running the identical test against BlackHole (a mature, widely used, already-installed reference driver) instead of Loomix In 1:
   - *Dropped capture buffers under load.* BlackHole reproduced an identical failure signature: a clean forward gap of exactly one capture chunk's worth of frames, never a duplicate or a rewind. A bug specific to this driver would not reproduce against a different driver under the same test code; a lossy test harness would. It's the latter — macOS occasionally drops a whole buffer between the device and the ffmpeg client process under system load, upstream of anything either driver controls.
   - *Live reconfiguration.* A version of the test that changed the device's nominal sample rate while ffmpeg's capture and playback processes were still attached produced heavy corruption afterward — not a brief blip, but roughly a third of everything captured post-change, as implausible large float values (not duplicated or skipped ramp values, which is what a driver-side bug would produce). BlackHole reproduces this too under the identical live rate change. Neither driver's client-facing behavior is being exercised realistically here: a real client tears its IO down and rebuilds it around a format change rather than expecting an open capture handle to survive the device changing rate underneath it.

5. *A third attempted fix that never worked, and was removed rather than debugged further.* To make the harness's pass/fail independent of client-delivery drops, the driver briefly exposed its internal write/read discontinuity counters as custom `AudioObject` properties (`'wdis'`/`'rdis'`), so a test client could confirm the driver's own IO timeline stayed gapless regardless of what the capture file showed. Every read of them failed with `kAudioHardwareUnknownPropertyError`: CoreAudio's host only forwards a *custom* property (as opposed to a standard one) to the plugin if it's been declared through `kAudioObjectPropertyCustomPropertyInfoList` first, which wasn't implemented, and per `<CoreAudio/AudioServerPlugIn.h>` a registered custom property's data has to be marshaled as a `CFString` or `CFPropertyList`, not the raw `UInt32` the code was returning. Rather than build out that machinery for a value nothing outside the driver needs — the counters' actual job, proving the ring buffer's own contiguity, is already done deterministically and load-bearingly by `test_ring_buffer.c` — the property dispatch code was removed. The counter fields themselves stay in `RingBuffer.h`, used internally and by the unit test.

**Net result — what "M1 is bit-exact" actually means here:** `test_ring_buffer.c` is the load-bearing, deterministic proof that the ring buffer never loses, duplicates, or reorders a sample and never serves stale data; it runs in CI on every push with no installed driver and no CoreAudio. `driver/tests/loopback_test.sh` is a manual, installed-driver smoke test on top of that, scored honestly against what it can actually prove given a harness two independent driver implementations (this one and BlackHole) both show is lossy under load and around live reconfiguration: zero duplicated or reordered samples is a hard failure (that would be a real bug reaching a client); a capped number of unexplained non-ramp samples is a hard failure (uncounted, they'd hide exactly what corruption looks like); a forward gap is not a failure (demonstrated, twice, to be the OS's client-delivery path, not either driver). The rate change is tested between two closed 30-second segments rather than under an open client connection, matching spec section 1.11's actual claim ("supports 44.1 through 192 kHz") without also asserting the unrelated, harder, and untrue-for-BlackHole-too claim that a live client survives a live reconfiguration.

**Branch protection on `main` requires all 8 `ci.yml` jobs**
(`lint`, `test (debug)`, `test (release)`, `rt_safety`, `coverage`, `driver`,
`ui`, `bench`), with branches required to be up to date before merging.
`enforce_admins` is off, so the repo owner can still push directly during
this early bootstrap phase; no required review count was set since none was
requested. Configured via the GitHub API once the repo had a remote — this
is a repository setting, not a file, so it isn't visible in this checkout.

**The bench-regression baseline is captured on GitHub's `macos-15` runner,
not a developer machine.** The M0 baseline for `rt_assert_guard_overhead`
was recorded on the machine that built it (~1.15ns) and failed the first
real CI run at +60.89% against a 10% gate — the runner's numbers cluster
around 1.7-1.9ns, a real, consistent hardware difference, not noise (two
separate CI runs landed at 1.856ns and 1.775ns, agreeing within 5%).
`ci.yml`'s `bench` job now uploads `target/criterion/*/pr/estimates.json`
as a build artifact (`if: always()`, so it uploads even when the gate
fails) precisely so this baseline can be regenerated from runner output
instead of a laptop; `ci.yml` also gained a `workflow_dispatch` trigger to
make that possible without a throwaway commit. The lesson generalises to
every bench baseline this project checks in from here on: capture it from
a CI run, never from a local machine.

## 2026-08-19 — M0

**Licence: dual MIT / Apache-2.0, copyright held by "Loomix contributors".**
Standard choice across the Rust ecosystem, compatible with the notarised,
commercially distributed installer the project ships (spec 4.5), and with
crediting BlackHole (MIT) as a design reference (spec 2.1). No personal
legal name was available to attribute the copyright to.

**Workspace edition 2021, shared package metadata via `[workspace.package]`.**
Every crate carries `version.workspace = true` etc. so a version bump is a
one-line change. Crate versions start at `0.0.0`; spec 3.4 calls out `0.1.0`
as the tag for the first usable build, at the end of M4.

**The rt-assert harness lives inside `loomix-core`, not a separate crate.**
Implementing `GlobalAlloc` requires `unsafe impl`, which conflicts with
`loomix-core` being one of the crates required to forbid unsafe code
(spec 4.2). Rather than carve out a third unsafe-permitting crate beyond
the two the spec names (`loomix-hal`, the driver bindings), `lib.rs` uses
`#![cfg_attr(not(test), forbid(unsafe_code))]`: the shipped, non-test build
still forbids unsafe code entirely, and the panicking allocator — test-only
infrastructure, never linked into a release binary — is permitted only
under `cfg(test)`. See `crates/loomix-core/src/rt_assert.rs`.

**`loomix-cli` and `loomix-app` ship as library stubs with no `[[bin]]` yet.**
An M0 `main()` with nothing to do but print a version string can't be
exercised by `cargo test`, and dragged the workspace under the 80% line
coverage gate for no real benefit. The executable entry point lands with
the milestone that gives each crate actual behaviour: M10 for the CLI's
subcommands, the first milestone that needs a UI surface for the Tauri
backend.

**The M0 driver target is a placeholder dynamic library, not yet the real
`AudioServerPlugIn` bundle.** Its only job right now is to prove the
`xcodebuild` + static-analysis + CI pipeline (spec 4.3) ahead of M1, which
adds the real entry point, factory function and `Info.plist`.

**`driver/tests/run-static-checks.sh` requires `clang-tidy` and fails if
it's missing, rather than skipping it.** It doesn't ship with the Xcode
command line tools; it comes from Homebrew's keg-only `llvm` formula, which
isn't linked onto `PATH` by default. The script checks `PATH` first, then
falls back to `$(brew --prefix llvm)/bin` directly, so installing it is
enough without also editing `PATH`; only a genuinely missing install fails,
with a message naming `brew install llvm`. `driver/.clang-tidy` configures
the enabled checks (`clang-analyzer-*`, `bugprone-*`, `performance-*`,
`portability-*`), since clang-tidy errors out with none enabled by default.
README documents the prerequisite.

**`ui/` is a bare TypeScript + Vitest + ESLint project, no React or Tauri
yet.** Proves the `typecheck` / `lint` / `test` pipeline the `ui` CI job
needs without pulling in a UI framework before there's a UI to build with
it.

**Bench regression gate uses checked-in JSON baselines under
`testdata/bench-baseline/`, one file per benchmark, written by
`scripts/save-bench-baseline.sh` and checked by
`scripts/check-bench-regression.sh <max-percent>`.** Mirrors the golden-file
rule in spec 4.1 layer 4 — regenerated deliberately, reviewed in the diff —
applied to benches, since the spec's `ci.yml` calls the check script
without specifying its comparison mechanism. A benchmark with no stored
baseline yet is reported and skipped rather than failing the build, so the
first bench for a new function doesn't need a baseline commit in the same
PR.

**`cargo-deny`'s license allow-list is broader than the current dependency
graph.** It includes the permissive licences (BSD, ISC, Zlib, Unicode-3.0,
CC0) that show up across most of the Rust ecosystem, to avoid a `deny.toml`
edit every time a new dependency needs one already-vetted. Unused entries
show up as informational "unmatched license allowance" warnings, not
failures.

**`nightly.yml`'s fuzz, soak and `release.yml`'s packaging jobs are
guarded or documented as inert until the milestones that create their
inputs land** (fuzz targets at M10/M11, the soak harness at M4/M9,
`packaging/build-pkg.sh` and the Developer ID secrets at M4). The
workflows ship now per the M0 requirement to have all of section 4.3 in
place from the start; they activate themselves the moment those milestones
add the files and secrets they check for, no workflow edit required.

**`CODEOWNERS` is set to the repository's git user.** Branch protection
requiring every `ci.yml` job (spec 4.3) is a GitHub repository setting, not
a file, and needs a GitHub remote to configure — tracked as an open item
for whoever pushes this repository to GitHub.

**A `justfile` wraps the build/test/lint/cover/bench/install-driver/
uninstall-driver/restart-coreaudio commands.** `docs/SPEC.md` doesn't
actually specify a justfile or these target names anywhere in section 3.4
or elsewhere — added on direct request, not because the spec calls for it;
noted here so this file doesn't misattribute it. Each recipe wraps the same
commands documented in the README and run by CI, so there's exactly one
place that knows how to run a check. `install-driver` and
`uninstall-driver` operate on the current placeholder driver product
(`libLoomixAudioDriver.dylib`); the copy/sign/restart mechanics carry over
unchanged once M1 turns it into the real bundle target.
