# Loomix

An 8x8 virtual audio mixer for macOS: hardware and virtual input strips,
gain-layered bus routing, internal FX, a recorder, MIDI mapping and network
audio, in front of a purpose-built `AudioServerPlugIn` driver.

`docs/SPEC.md` is the single source of truth for scope, architecture and
milestones. Nothing ships that isn't covered there.

Loomix is not affiliated with, endorsed by or a redistribution of
Voicemeeter or VB-Audio software. The name Voicemeeter, VB-Audio, VBAN,
Banana and Potato are not used in this product's name, bundle identifier,
UI strings or marketing copy; feature-compatibility with the reference
product is a design goal, its identity is not borrowed. The one exception
is the VBAN wire protocol name, which may appear in technical documentation
and code comments describing protocol interoperability.

The virtual audio driver draws on the design of
[BlackHole](https://github.com/ExistentialAudio/BlackHole) (MIT licensed)
as a reference for building a CoreAudio `AudioServerPlugIn` on macOS;
Loomix's driver is a separate implementation shaped for eight independent
virtual I/O pairs with a private control channel.

## Status

The description above is the finished product `docs/SPEC.md` is building
toward, milestone by milestone (section 3.4) -- it is not what's runnable
today. As of milestone M4:

**Built:**

* The `AudioServerPlugIn` driver: 8 virtual I/O pairs (16 devices total),
  stable UIDs, hot-plug and `coreaudiod`-restart survival (M1, M2).
* The engine core: 8 hardware/virtual input strips, 8 output buses, the
  full 8x8 assignment matrix, per-bus gain layers, mute, solo, mono, fader
  law, metering (M3).
* Hardware I/O and clocking: device enumeration and selection, hog mode,
  clock master selection, drift-corrected resampling, internal-clock
  fallback, hot-plug handling, aggregate devices (M4).

**Not yet built** (milestone order): strip processing -- gate,
compressor, limiter, denoiser, pan laws (M5); the parametric EQ engine
(M6); bus modes and patching (M7); the app shell and first UI -- React +
Tauri, the strip/bus layout, faders, meters, device selection (M8);
internal FX -- reverb, multitap delay, multiband compressor (M9); the
recorder (M10); the control surface -- request script, MIDI mapping,
macro buttons (M11); network audio (M12); and the polish/release
milestone -- preset scenes, installer, docs site (M13).

## Repository layout

```
crates/           Rust workspace: engine, HAL, network, RPC, recorder, config, CLI, app backend
driver/           AudioServerPlugIn bundle (C), Xcode project and static checks
ui/               TypeScript front end
packaging/        Signed, notarised .pkg build
docs/             SPEC.md (source of truth), ARCHITECTURE.md (decision log)
testdata/         Golden renders and fixtures
```

## Prerequisites

* Xcode and the Xcode command line tools, for the driver.
* [`just`](https://github.com/casey/just), to run the commands below.
* `clang-tidy`, for the driver's static analysis pass. Not part of the
  Xcode command line tools -- install it with `brew install llvm` and add
  `$(brew --prefix llvm)/bin` to `PATH` (or leave it off `PATH`;
  `driver/tests/run-static-checks.sh` finds it at the Homebrew prefix
  either way).
* Node.js 22, for the UI.

## Developer commands

```
just build             # cargo build + a debug driver build
just test              # cargo test, workspace + release golden tests
just lint               # fmt, clippy, cargo deny, driver static checks, ui typecheck/lint
just cover              # coverage gate, 80% line minimum workspace-wide
just bench               # criterion bench + regression check against testdata/bench-baseline/
just install-driver      # ad-hoc-signs and installs the driver, restarts coreaudiod
just uninstall-driver    # removes the driver, restarts coreaudiod
just restart-coreaudio   # sudo killall coreaudiod
```

`install-driver`, `uninstall-driver` and `restart-coreaudio` need `sudo`
and restart `coreaudiod`, which briefly interrupts all audio on the
machine -- each says so before it runs.

Without `just`, the underlying commands:

```
cargo build --workspace
xcodebuild -project driver/LoomixAudioDriver.xcodeproj -scheme LoomixAudioDriver -configuration Release build CODE_SIGNING_ALLOWED=NO
npm ci --prefix ui
cargo test --workspace --all-features
cargo test -p loomix-core --features rt-assert -- realtime   # real-time safety harness, spec 3.3
cargo bench -p loomix-core
npm run --prefix ui test -- --coverage
```

CI (`.github/workflows/ci.yml`) runs the same checks, plus `clippy`,
`rustfmt`, `cargo deny`, and coverage (80% line minimum, 90% in
`loomix-core`), on every push and pull request.

## Non-goals

* No Windows or Linux support.
* No screen sharing or video streaming.
* No licence activation, no telemetry, no analytics.
* No cloud accounts.
* No attempt to bypass any macOS security mechanism. The driver is signed
  and notarised or it is not shipped.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at
your option.
