# Changelog

All notable changes to this project are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

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
