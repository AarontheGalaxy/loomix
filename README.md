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

## Repository layout

```
crates/           Rust workspace: engine, HAL, network, RPC, recorder, config, CLI, app backend
driver/           AudioServerPlugIn bundle (C), Xcode project and static checks
ui/               TypeScript front end
packaging/        Signed, notarised .pkg build
docs/             SPEC.md (source of truth), ARCHITECTURE.md (decision log)
testdata/         Golden renders and fixtures
```

## Building

```
cargo build --workspace
xcodebuild -project driver/LoomixAudioDriver.xcodeproj -scheme LoomixAudioDriver -configuration Release build CODE_SIGNING_ALLOWED=NO
npm ci --prefix ui
```

## Testing

```
cargo test --workspace --all-features
cargo test -p loomix-core --features rt-assert -- realtime   # real-time safety harness, spec 3.3
cargo bench -p loomix-core
npm run --prefix ui test -- --coverage
```

CI (`.github/workflows/ci.yml`) runs all of the above, plus `clippy`,
`rustfmt`, `cargo deny`, coverage (80% line minimum, 90% in `loomix-core`),
and the driver's static analysis pass, on every push and pull request.

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
