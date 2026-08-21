# Architecture decision log

Decisions made during implementation that `docs/SPEC.md` leaves to
engineering judgement, dated, so the reasoning survives past the PR that
made them. `SPEC.md` remains the source of truth for anything it does
specify; this file never contradicts it.

## 2026-08-21

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
