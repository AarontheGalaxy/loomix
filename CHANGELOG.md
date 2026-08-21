# Changelog

All notable changes to this project are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

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
