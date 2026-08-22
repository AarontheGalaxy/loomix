# Changelog

All notable changes to this project are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.1.0] - 2026-08-23

Spec 3.4 milestones M0 through M4: virtual audio driver, engine core, and
hardware I/O and clocking. The first release with something usable
end-to-end -- see the caveat under Known limitations before relying on the
drift-correction claim.

### Added

- Cargo workspace skeleton: `loomix-core`, `loomix-hal`, `loomix-net`,
  `loomix-rpc`, `loomix-recorder`, `loomix-config`, `loomix-cli`,
  `loomix-app`.
- Real-time safety harness in `loomix-core` (spec section 3.3): a
  `RealtimeGuard` / `assert_realtime` scope marker, and a test-only global
  allocator, gated behind the `rt-assert` feature, that panics on any
  allocation made while the guard is active.
- `driver/` Xcode project skeleton, building a placeholder dynamic library
  with `-Wall -Wextra -Werror`, plus a static-analysis script.
- `ui/` TypeScript project skeleton with typecheck, lint and test scripts.
- CI: `.github/workflows/ci.yml` (lint, test, real-time safety, coverage,
  driver, ui, bench jobs), `nightly.yml` (fuzz, soak, dependency
  freshness), `release.yml` (tagged, signed, notarised release build);
  Dependabot, CODEOWNERS, and a pull request template.
- `cargo deny` license and advisory policy (`deny.toml`).
- Dual MIT / Apache-2.0 licensing.
- `docs/ARCHITECTURE.md` decision log.
- The `AudioServerPlugIn` virtual audio driver (spec 3.4 M1, M2): all 8
  input pairs and 8 output endpoints (16 devices total, `Loomix In 1-8`,
  `Loomix Out A1-5`/`B1-3`), stable UIDs, correct channel layouts, a
  deterministic ring buffer that never loses, duplicates, reorders or
  serves stale samples (`test_ring_buffer.c`, spec 4.1 layer 4's bit-exact
  proof), lazily-allocated per-device ring buffers, and install/uninstall
  scripts with device-count verification.
- A host-side driver test harness (`test_driver_host.c`) that links the
  driver's C sources directly and drives its `AudioServerPlugInDriverInterface`
  vtable in-process -- no installed driver or `coreaudiod` needed -- plus a
  fault-injection build (`test_driver_host_fault_injection`) that forces
  every allocation in the driver to fail and confirms it fails cleanly
  rather than crashing.
- `loomix-core` engine core (spec 3.4 M3): 8 strips, 8 buses, the full
  8x8 assignment matrix, per-bus independent gain layers, mute, solo,
  strip and bus mono, the shared fader law (`gain_db_to_linear` /
  `gain_linear_to_db`), and peak-hold metering. No effects processing yet
  (denoiser, gate, comp, EQ, pan, FX sends, bus modes land M5 onward).
- An offline deterministic render harness (`loomix-core::render`) and the
  routing truth-table test (spec 4.1 layer 5) that is M3's acceptance
  criterion: exhaustive per-cell assignment cross-talk checks, full
  mute/solo combination sweeps, independent per-bus gain layer checks, and
  strip/bus mono checks, all via a Goertzel tone-magnitude probe.
- Fader law and engine-state property tests (spec 4.1 layers 1-2):
  monotonicity, continuity and round-trip of the dB/linear conversion,
  `-inf` as exact digital silence, and a finite/bounded-output check under
  randomised parameter sequences.
- Hardware I/O and clocking (spec 3.4 M4): device enumeration and
  selection, hog mode, clock master selection with internal-clock
  fallback (`loomix-hal::clock`), drift estimation with a slow PI
  controller and discontinuity detection for clock jumps
  (`loomix-hal::drift`), a windowed-sinc polyphase resampler
  (`loomix-hal::resample`), hot-plug and hog-mode decision logic as
  exhaustive transition tables (`loomix-hal::hotplug`, `loomix-hal::hog`),
  and the drift-corrected real-time IOProc I/O stage
  (`loomix-hal::ioproc`, `loomix-hal::master_clock`) proven against a
  synthetic fake-device harness before any CoreAudio call existed to
  drive it.
- CoreAudio device glue (`loomix-hal::device`): enumeration, channel-count
  and default-input/output queries, hog mode, a device-list-change
  listener, capture/render/master IOProc registration, and aggregate
  device creation.
- `loomix-app::engine_io`: assembles per-strip and per-bus ring buffers
  around `Engine::process_block`, driven by whichever device's IOProc
  calls it once per callback; `select_clock_master` decides which device
  drives that call.
- `loomix-soak`: a manual two-device soak harness for spec 3.4 M4's
  30-minute acceptance test, reporting a measured dropout count and drift
  ratio rather than a runnable-only demo.

### Fixed

- The real-time safety harness's panicking test allocator (spec 3.3) now
  fails an offending test reliably in release builds, not just debug:
  panicking from inside a `#[global_allocator]` method is not reliably
  unwindable once LLVM optimises it, so the allocator now only records
  that a violation happened and `RealtimeGuard::drop` panics from
  ordinary, non-allocator code once the guarded scope ends.
- `ci.yml`'s `test` job matrix legs (`debug`, `release`) now run distinct
  commands gated on `matrix.profile`; previously both legs ran the same
  debug-only `--all-features` suite, so the release leg never actually
  exercised a release build.
- Unchecked allocations in the driver's device-creation and
  sample-rate/stream-format change paths, and eager per-device ring
  allocation at plug-in load time (16 devices x ~1.7 MB, unconditionally),
  which crashed or wedged `coreaudiod` on install. Allocation is now
  checked everywhere and rings are allocated lazily, on a device's first
  `StartIO`.
- `ci.yml`'s `driver` job built the Release driver bundle but never
  installed it (`CODE_SIGNING_ALLOWED=NO` with no `-derivedDataPath`, so
  even calling `install.sh` would have found no product), so any check
  assuming Loomix's virtual devices were enumerable in CI was decorative.
  The job now builds ad-hoc-signed and actually installs, confirmed on
  the real runner (16 devices visible).
- `ci.yml`'s `rt_safety` job only ever tested `loomix-core`; every
  `assert_realtime` test in `loomix-hal` and `loomix-app` had only run
  with the flag-only, non-panicking version of the guard, never the real
  trapping allocator. Now covered via `--features loomix-core/rt-assert`
  against both crates.
- The capture and render IOProc trampolines assumed one `AudioBuffer` per
  channel; every real output device tried (built-in speaker, an external
  monitor, BlackHole) delivers one interleaved buffer instead, so a
  second channel's ring buffer was never drained and overran continuously
  within about a second on real hardware. Fixed by deinterleaving/
  interleaving through pre-allocated scratch storage rather than trusting
  a non-interleaved format request, which CoreAudio can silently ignore.
- CI's `coverage` job failed after this milestone's work (78.21% against
  an 80% gate). Traced to code that starts real device I/O or creates a
  real system device -- deliberately never run by the automated suite --
  split into its own files (`loomix-hal::device_lifecycle`,
  `loomix-app::device_wiring`) and excluded from the gate by name; the
  threshold itself was not lowered, and the two genuinely testable pieces
  found this way (`select_clock_master`, `parse_duration`) got real tests
  instead of being excluded.

### Known limitations

- Drift correction (`loomix-hal::drift`, `loomix-hal::resample`,
  `loomix-hal::ioproc`) is validated by synthetic-clock tests only. The
  manual 30-minute two-device soak spec 3.4 M4 calls for as its
  acceptance criterion has been run, but used two software-clocked
  Loomix virtual devices sharing one timer, not two physically
  independent interfaces, and carried silence rather than real signal --
  the drift ratio held at exactly 1.0 for the full run, meaning the
  corrector was never actually exercised. It has not yet been validated
  against real hardware drift. See `docs/ARCHITECTURE.md`, "M4's real
  acceptance criterion is not yet met."
