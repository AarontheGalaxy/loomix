# Changelog

All notable changes to this project are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Bus modes and patching (spec 3.4 M7): all 12 bus modes (`loomix-core::
  bus_mode`), the composite bus patch, the insert patch, and both pre/post
  switches.
  - `BusMode` (Normal, Mix Down A/B, Stereo Repeat, Composite, Up Mix
    TV/2.1/4.1/6.1, Center/LFE/Rear Only) is a pure transform on the bus's
    summed 8-channel frame, applied before the bus EQ (spec 1.2 step 3,
    proved with an order-inverting test the same way M6's EQ-before-mono
    order was). Mix Down A/B implement the spec 1.6-corrected `FR`-based
    right-channel formula, not the vendor manual's published (typo'd,
    `RL`-based) one, with a dedicated test proving the two disagree.
  - Composite mode fills a bus's 8 channels from a shared, engine-level
    patch (`Patch::composite`), each slot either the bus's own ordinary
    sum (`Default`) or a specific `(strip, channel)` source taken pre- or
    post-fader (`Patch::composite_post_fader`) — addressed against this
    engine's own strip/channel model rather than the vendor manual's
    Windows-specific flat channel numbering.
  - The insert patch (`Patch::insert`, 22 toggles) and its pre/post-FX
    switch are config-only this milestone: no send/return path exists yet
    (spec 2.3 defers that past M7), so they have no audio effect, proven
    by a bit-exact-identical-output test.
  - `Engine::process_block`'s loop is now sample-outer/strip-middle/
    bus-inner (was strip-outer/bus-inner) so Composite mode can read any
    strip's just-processed frame from any bus, without re-running a
    strip's stateful chain a second time. A new bench
    (`loomix-core::benches::engine`) measured the mute-handling tradeoff
    this required directly: running every strip's chain unconditionally
    regressed a mostly-muted mixer's per-block cost by 2.6x versus running
    only strips that are unmuted, soloed-in, or actually referenced by an
    active composite tap — see `docs/ARCHITECTURE.md`.
- Strip processing (spec 3.4 M5): the per-strip effects chain, run once per
  input frame ahead of bus summing (`loomix-core::strip_dsp`).
  - Hardware strips (spec 1.2's order): denoiser (adaptive noise-floor
    expander), gate (band-pass sidechain detector), compressor (soft-knee,
    auto makeup), the three mutually exclusive Intellipan pad modes (Color
    tonal tilt, Position binaural ILD/ITD, Modulation chorus/feedback), a
    balance-law pan pot, and a sample-accurate brickwall limiter.
  - Virtual strips: a 3-band peaking EQ, M.C. (center-channel mute), Karaoke
    (K-m/K-1/K-2/K-v, AUX strip only), a balance-law 5.1 position pad, and
    the same limiter.
  - Gate, Compressor and Denoiser share one macro-knob curve (0..10,
    `knob_curve.rs`) — Loomix's own documented mapping, not a reproduction
    of Voicemeeter's unpublished one (`docs/DSP.md`); `knob <= 0.0` is a
    true bypass on all three.
  - A shared RBJ/Audio EQ Cookbook biquad (`biquad.rs`) backs the gate
    sidechain, the 3-band EQ, Color's tonal shaping and Karaoke.
  - `Strip` is now built per spec 1.1's fixed Potato topology
    (`Strip::for_topology_index`) rather than uniformly, since hardware vs.
    virtual strips now have materially different chains.
  - `Engine` gained a `sample_rate` field/setter, propagated to every
    strip's chain, and `process_block`'s loop is now strip-outer/bus-inner
    so each strip's stateful chain runs exactly once per frame.
- Every block above has a null test at its own true-neutral setting (spec
  4.1's bit-exact passthrough requirement), a known-answer test, a
  frequency-response or static-curve test where applicable, and a
  stability test under randomised parameter automation — written before
  each block's implementation.
- Parametric EQ (spec 3.4 M6): the shared 6-cell engine (`parametric_eq.rs`),
  wired into both the hardware strip EQ (stereo, spec 1.2 step 7) and the
  bus EQ (8 independent channels, spec 1.2 step 4).
  - 7 cell types (spec 1.7): peak, low pass, high pass, low shelf, high
    shelf, band pass, notch — 4 new RBJ/Cookbook coefficient constructors
    added to `biquad.rs` alongside the existing peaking/band-pass.
  - Per-channel trim (-24..+24dB) and delay (0..500ms, a fixed-capacity
    ring sized to the current sample rate).
  - A/B memory, `FLAT` reset, `CH COPY` / `COPY ALL` (including copying
    between a strip's and a bus's differently-sized channel sets, since
    the parameter model is shared per spec 1.7).
  - Load/save as a versioned JSON file (`loomix-config::eq_file`).
  - `Biquad::set_coeffs` now smooths coefficient changes over a fixed
    64-sample ramp instead of applying them in one step, since M6's cells
    are the first callers swept live by a dragged control — an
    instantaneous coefficient swap on accumulated filter state is an
    audible discontinuity, worst on a cell-type switch. Engaging/leaving
    bypass stays instantaneous, matching every block's existing true-
    neutral convention.
  - `ui/src/eqResponse.ts` / `eqGraph.ts`: an independent TypeScript
    implementation of the same response math plus a framework-free SVG
    renderer, cross-checked against a fixture generated from the real
    Rust engine rather than trusted independently by either side.

### Known limitations

- Intellipan's Color pad tonal-shaping ships without its "small reverb on
  the upper half" (spec 1.3); Position pad ships without its "small room
  effect" for the same reason. Both are strip-local reverb-family effects
  that would duplicate M8's real send/return reverb engine if built now —
  deferred to M8, not silently dropped. See `docs/ARCHITECTURE.md`.
- Virtual-strip processing order (EQ/M.C./Karaoke relative to the 5.1 pan
  pad) is not specified by spec 1.4 and is Loomix's own judgement call,
  checked by an order-proving test rather than the spec's own wording. See
  `docs/DSP.md`'s "Per-strip chain order" and `docs/ARCHITECTURE.md`.
- Karaoke's K-1/K-2/K-v depths and every macro-knob curve are Loomix's own
  values, not derived from or verified against Voicemeeter, since no
  published reference exists for either.
- The EQ graph is response-curve math and an SVG renderer only, not yet an
  interactive UI control — `ui/` stays a bare TypeScript project (no
  React/Tauri) until the milestone that stands up the app shell. Right-
  click-to-type-a-value and right-click-to-change-the-dB-scale (spec 1.7)
  are real UI work for that milestone; the renderer already takes a dB
  range as a parameter so that control has something to drive.

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
