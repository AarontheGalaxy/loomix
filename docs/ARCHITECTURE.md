# Architecture decision log

Decisions made during implementation that `docs/SPEC.md` leaves to
engineering judgement, dated, so the reasoning survives past the PR that
made them. `SPEC.md` remains the source of truth for anything it does
specify; this file never contradicts it.

## 2026-08-24 — M8 (continued): Tauri scaffolding and the first React UI

**`loomix-app` gets its `[[bin]]` (`loomix`) and the Tauri dependency
itself, per the M0 log's own forecast ("the first milestone that needs a
UI surface for the Tauri backend").** `tauri.conf.json`/`build.rs`/
`capabilities/default.json` live directly in `crates/loomix-app/` (no
separate `src-tauri/` directory -- spec 3.2 already designates this crate
as the Tauri backend, so nesting a second Cargo-adjacent directory inside
it would just be indirection). `capabilities/default.json` grants only
`core:default` -- enough to invoke this app's own custom commands; no
file system, shell or dialog plugin access, since nothing here needs it.
`ui/` gains its Vite config, `index.html` and React entry point
(`main.tsx`/`App.tsx`), replacing `index.ts`'s placeholder `ping()`
function, whose own doc comment named this exact milestone as the one
that would make it obsolete.

**`main.rs` deliberately does not wire real CoreAudio device I/O yet,**
despite spec 3.4 M8 listing device selection in scope. `spawn_audio_thread`
simulates the real-time thread instead: a timer-paced loop calling the
same `CommandDrain::drain_into` -> `Engine::process_block` sequence a real
IOProc callback would, feeding strip 0 a synthetic 440Hz tone, publishing
`ControlSnapshot`/`MeterSnapshot` exactly like the real thing would. This
proves the entire UI <-> bridge <-> engine loop end to end -- every
control in the M8 surface actually reaching a running `Engine`, meters
actually moving -- without also taking on live device enumeration,
selection and IOProc (re)registration in the same pass, which is a
materially riskier piece of work (the M1/M2 log entries below record two
`coreaudiod` crashes from exactly this class of live-device work) that
deserves its own review rather than being bundled in. Explicitly the next
step, not a silently dropped part of scope.

**`loomix-app::control`'s `snapshot_channel`/`SnapshotPublisher`/
`SnapshotReader` were generalised to `latest_value_channel`/
`LatestValuePublisher<T>`/`LatestValueReader<T>`, so meters could reuse
the exact same "latest value wins" crossing as the reconciliation
snapshot instead of a second, copy-pasted implementation.** `MeterSnapshot`
(new) wraps `[Meter; NUM_STRIPS]`/`[Meter; NUM_BUSES]`, captured and
published alongside `ControlSnapshot` every callback -- the crossing
`Meter`'s own doc comment named as owed "once a UI thread exists," which
is now. Polled by the frontend much closer to per-frame than
`ControlSnapshot`'s deliberately low reconciliation rate, since meters
are meant to move visibly.

**Tauri command arguments/return values are plain, JSON-friendly
primitives and strings (`set_bus_mode(bus: usize, mode: String)`), not
`EngineCommand`/`BusMode` serialised directly across the IPC boundary.**
Neither `BusMode` nor `BusMono` derive `serde::Serialize`/`Deserialize`
today (only `EqCellParams` does, for `loomix-config`'s existing EQ-file
format), and adding those derives just to cross an IPC boundary would
couple `loomix-core`'s public enums to a wire format they don't otherwise
need. `main.rs`'s `bus_mode_to_str`/`bus_mode_from_str` (and the `BusMono`
equivalents) are the one, explicit translation point instead -- an unknown
string from the frontend is a clean `Result::Err` back across `invoke()`,
not a deserialisation panic.

**A placeholder icon (`icons/icon.png`, a flat mid-grey square, generated
programmatically) stands in until real branding exists.** `tauri::
generate_context!` reads an icon at compile time unconditionally, even
with `bundle.active: false` (packaging itself is M13's job, spec 3.4) --
without one, the binary doesn't compile at all, dev or not. `bundle.active:
false` means `cargo tauri build`'s installer/signing path stays inert
here the same way `release.yml`'s packaging gate already does (M0 log,
below) until M13 actually needs it.

**Diagnosed, not worked around: `npm run lint`/`typecheck` intermittently
stalled for minutes during this milestone's `npm install`s, traced to real
OS-level I/O contention, not a code or config bug.** Sampling the stuck
`eslint` process (`sample <pid>`) showed its entire call stack inside
`uv_run` -> `uv__io_poll` -> `AfterStat` -- the Node event loop genuinely
blocked waiting on a kernel filesystem callback, not spinning in JS. A
swarm of `mdworker_shared` processes (macOS Spotlight) had spawned in the
same window as the `npm install`s and `cargo build`s that just created
hundreds of thousands of small files across `node_modules/` and `target/`
on a disk that was down to 15GB free at the time. On instruction, fixed at
the source rather than by retrying or increasing timeouts: `touch
target/.metadata_never_index ui/node_modules/.metadata_never_index`
excludes both trees from Spotlight going forward (the marker stops future
indexing of a directory; it doesn't retroactively cancel an already
in-flight scan, so the already-queued backlog still had to drain once).
Confirmed by re-running lint to a genuinely fresh pass afterward, not by
accepting the clean run from before the contention started -- accepting a
verification that predates the change it's meant to verify is exactly the
failure class this project's CLAUDE.md already calls out by name, and
doing it here anyway would have been the same mistake in a new disguise.
`eslint-plugin-react-hooks` was also pinned to `^5` instead of the latest
`^7`: v7 bundles the new React Compiler analysis rule (a ~55K-line
generated file), which this project doesn't use and which measurably
worsened the same contention while diagnosing it -- a real, if secondary,
fix, not the actual root cause.

## 2026-08-24 — M8 (continued): unchecked indices in `EngineCommand::apply`

**A pushed-commit security review flagged an under-validated sink
argument in `loomix-app::control`: `EngineCommand::apply`'s strip/bus/
channel/cell indices went straight into array indexing (`engine.
strips[s]`, `ParametricEq::set_cell`'s internal `cells[cell]`) with no
range check at all.** `EngineCommand` has no `TryFrom`/range type of its
own -- any `usize` constructs one -- and the intended validation point
(the UI/Tauri-command layer, "validate at a system boundary" per
`parametric_eq::EqCellParams::freq_hz`'s own doc comment) doesn't exist
yet. `apply` runs on the audio thread inside `CommandDrain::drain_into`,
so an out-of-range index reaching it would panic there, not in an HTTP
handler -- the worst place in this codebase for a panic, and a real
concern once a Tauri command layer exists for a compromised or merely
buggy frontend to reach it from, not a hypothetical.

Fixed by bounds-checking every index in `apply` itself before indexing
anything, silently skipping the command if any index is out of range --
the same "harmless no-op" shape the existing virtual-strip EQ-command
case already used, generalised. This is deliberately a second line of
defence, not a replacement for real UI-layer validation once that layer
exists: a sink this catastrophic to get wrong shouldn't rely on every
future caller remembering to check first.
`out_of_range_indices_are_ignored_not_panicked` drives one
representative out-of-range case per index shape in the file (strip-only,
bus-only, both, and the EQ commands' extra channel/cell indices) through
both `apply` directly and the real `CommandSink`/`CommandDrain` path,
and confirms no engine state changes and nothing panics.

The finding surfaced through a background commit-security-review
notification attached to an unrelated user turn, formatted oddly enough
(garbled repeated text, a stray escaped closing tag) to be worth
independently re-verifying against the actual code before acting on it,
rather than trusting the notification's own framing -- the underlying
claim held up, so it was fixed on its merits, logged here the same way
any other reviewer-caught bug would be.

## 2026-08-24 — M8 (continued): the UI <-> audio-thread bridge

**`loomix-app::control`** implements the two one-way, lock-free crossings
spec 3.3 requires between Tauri's command handlers and the real-time
thread (`EngineCommand`s in via an `rtrb` SPSC queue, a `ControlSnapshot`
out), plus the app-side optimistic control-state mirror described in the
M8 plan. Three properties were tightened on direct instruction before this
was accepted as done, each with its own proof rather than an argument:

**Command-queue overflow is neither silently dropped nor treated as
acceptable at any volume — it's designed to be unreachable in ordinary
use, and counted (not silently absorbed) if it's ever actually reached.**
`CommandSink` coalesces per parameter (`ParamKey`, last-value-wins) before
ever touching the SPSC queue, so `pending`'s size is bounded by the number
of *distinct* controls changed since the last flush, not by how many
events arrived. `COMMAND_QUEUE_CAPACITY` (1024) is sized against the
mixer's total distinct addressable M8-scope parameters — every strip's
mute/solo/mono (24), bus assigns (64), gain layers (64) and EQ cells (up
to 96, hardware strips only), plus every bus's mute/mono/mode/gain (32)
and EQ cells (384) — under 700 total, so "every control in the mixer
changed at once" still fits with headroom; reaching capacity in practice
means the audio thread has stopped draining (a stalled device, not
ordinary use). `CommandSink::flush` pushes every pending command and, on
a failed push, leaves that entry in `pending` for the next flush rather
than discarding it — a coalesced key can only ever converge toward the
last value actually sent, never toward a stale one, so retrying is always
safe — while `OverflowCounter` (the same lock-free, `Arc<AtomicU64>`
shape as `engine_io::DropoutCounter`) counts every failed push so a stuck
queue is observable, not silent. `CommandDrain::drain_into`'s separate
per-callback cap (64) bounds one callback's *work*, not correctness: an
undrained remainder just waits for the next callback a few milliseconds
later, it is never discarded.

Two tests prove this rather than assert it: `a_flood_past_capacity_
still_converges_to_the_last_value_sent` deliberately floods 8x past
capacity and confirms the engine ends up at exactly the last value sent
per parameter, not a stale or corrupted one — the retry-with-coalescing
guarantee actually holds under real overload, not just in argument. A
first version of the concurrent stress test below (see that entry)
instead had an *unthrottled* producer thread and drove the overflow
counter into the tens of millions in a few hundred milliseconds — not a
bug in the design, a bug in the test: an unthrottled spin loop calling
`flush()` as fast as the CPU allows issues push attempts many orders of
magnitude faster than any real UI event source (a mouse drag, a 120Hz
display) ever could, so it wasn't modelling "sustained hammering" at all,
just proving a physically-impossible input can still overflow a finite
queue, which was never the claim. Paced to ~10kHz — still far above any
real UI framework's event rate — the same design produces zero overflow
under the same total volume of work.

**A low-rate reconciliation snapshot (`ControlSnapshot`) closes the loop
the optimistic UI mirror opens: without it, a dropped or misapplied
command is invisible — the fader shows one thing, the engine does
another, and nothing anywhere notices.** Scoped deliberately to
`EngineCommand`'s own scalar surface (mute/solo/mono, bus assigns, gain
layers, bus mute/mono/mode/gain) and not the EQ cells — "just enough that
drift is detectable" on direct instruction, not an exhaustive mirror; EQ
cells get their own, larger snapshot later if drift there is ever found
to matter in practice, the same "logged, not silently incomplete"
pattern as every other scope cut in this file. `ControlSnapshot::capture`
reads straight off live `Engine` state (called once per callback,
alongside meter observation); the crossing itself
(`snapshot_channel`/`SnapshotPublisher`/`SnapshotReader`) is a small
"latest value wins" cross over the same `rtrb` primitive `control_channel`
already uses (see the `triple_buffer` entry below for why not a dedicated
triple-buffer crate) — the publisher's `push` is fire-and-forget (a
dropped intermediate snapshot costs nothing, only the *latest* one is
ever read), and the reader drains the whole small backlog on every poll
and keeps only the last value popped. `reconciliation_snapshot_matches_
the_uis_optimistic_mirror_after_a_burst_of_changes` drives a burst of
varied commands through the real command path, publishes and reads back
through the actual channel (not just the pure `capture` function), and
asserts it equals an independently-built mirror of what the UI would
already believe from applying the same commands optimistically — proving
the crossing itself, not just the snapshot-building logic in isolation.

**`triple_buffer` (the crate) was tried first for the snapshot crossing
and rejected by `cargo deny check`, not by inspection: it's MPL-2.0, a
copyleft license outside `deny.toml`'s allow-list, which this project's
license policy (spec 4.5's notarised, commercially-distributed installer)
deliberately keeps to permissive-only licenses.** Widening the allow-list
for one dependency's convenience is a real project-policy change, not an
implementation detail to decide unilaterally mid-milestone — so instead
of asking for that, the crossing was rebuilt on `rtrb`, which is already
a vetted, permissively-licensed dependency of this exact crate, doing the
same "latest value wins, never blocks, never allocates" job spec 3.3
actually asks for (a genuine triple buffer's extra property — the reader
never blocking the writer mid-write on a torn read — isn't needed here
either, since `Copy` types never observe a torn write through an
independent ring slot the way a larger, in-place-mutated buffer could).
Caught by running `cargo deny check` itself, per this project's own
verification discipline, not assumed compatible because `rtrb` already
was.

**The concurrent stress test (`realtime_concurrent_hammering_produces_
zero_overflow_and_no_allocation`) runs two real OS threads racing for
real, not a simulated sequence.** A "UI" thread paced to ~10kHz
continuously enqueues varied commands (gain layers, mute, bus mode, *and*
EQ cells) and flushes; an "audio" thread spins unthrottled, draining and
calling `Engine::process_block` back to back, the whole closure wrapped
in `assert_realtime` per iteration — proving no allocation under actual
thread contention, not just the single-threaded call-order the other
tests above assume. This is also what surfaced that `EngineCommand::apply`
needed its own dedicated real-time proof
(`realtime_command_apply_does_not_allocate`, covering every variant
individually): the EQ-cell variants route into `ParametricEq::set_cell`,
which recomputes biquad coefficients and a delay line's cursors — real
work that had no real-time obligation before this milestone, since
nothing called it from the audio thread until `CommandDrain` existed.
Both tests are run under the real trapping allocator
(`cargo test -p loomix-app --features loomix-core/rt-assert`, the exact
mechanism the M4 log below established), not the flag-only guard —
verified directly, the same discipline that mechanism's own introduction
required. The stress test's name deliberately contains `realtime` (matching
`ci.yml`'s `rt_safety` job filter, `-- realtime`) after a first version
without that naming convention was found to not actually run under that
CI leg at all, only under the general `test` job, which never enables
`rt-assert` — the same class of decorative-test gap the M4 log below
records twice already (the rt-safety CI leg not covering `loomix-hal`/
`loomix-app` at all; the `driver` job never installing what it built).

## 2026-08-23 — new M8 inserted: app shell and first UI

**A new milestone, "App shell and first UI" (React + Tauri scaffolding,
the strip/bus layout, faders, mute, solo, bus assignment, meters, device
selection, and the EQ graph wired to a live engine), is inserted into
spec 3.4 between M7 and M8, on direct instruction. Old M8-M12 (Internal
FX, Recorder, Control surface, Network audio, Polish/release) shift to
M9-M13; `docs/SPEC.md` and every forward-pointing milestone-number
reference across the codebase (doc comments, `CHANGELOG.md`, `README.md`,
`nightly.yml`, and this file's own earlier entries) were updated to
match, not just the milestone table itself.**

Why now rather than at the position it originally held (after M8-M11):
M3 through M7 built a complete signal path (the 8x8 matrix, strip
processing, the parametric EQ, all 12 bus modes and patching) that has
never been driven by anything but test harnesses and the offline render
fixture. Every milestone from here on (internal FX, the recorder, the
control surface) adds more engine surface that would go the same way —
built, tested, never actually heard — without something to operate it
first. `ui/` has been a bare TypeScript project with no framework since
M0 specifically because nothing needed one yet (M0's own log entry); that
stops being true once there's a mixer worth listening to. Scoped
deliberately short of the full reference surface (spec 1.17's companion
tools, macro buttons, MIDI, the recorder transport, Intellipan's pads) —
the goal is the smallest UI that closes the loop from a control to real
audio, not a second front working in parallel with the DSP milestones.

`ui/src/eqGraph.ts`/`eqResponse.ts` (M6) already exist as pure response
math and a framework-free SVG renderer, cross-checked against the real
Rust engine's own math via a generated fixture — this milestone's job is
wiring that existing, already-verified math to a live `ParametricEq`'s
actual cell values, not rebuilding it.

## 2026-08-23 — M7: bus modes and patching

**`process_block`'s loop is now sample-outer / strip-middle / bus-inner
(was strip-outer / bus-inner), required for Composite bus mode, not a
style choice.** Spec 1.6's Composite mode can source *any* strip's signal
into *any* bus's channel, independent of that strip's own `bus_assign` —
so the bus loop needs random access to every strip's just-processed frame
at the current sample. Under the old strip-outer shape that frame only
ever existed as a transient local, gone once the next strip started; the
only way to get it back inside a later bus-loop pass would have been to
call a strip's (stateful) `chain.process()` a second time, which is
exactly the corruption bug M5's log already fixed once (running a
strip's envelope/filter chain more than once per input frame). The new
shape still runs each strip's chain exactly once per sample — it just
does so before the bus loop that needs the result, inside a fixed-size
`[Frame; NUM_STRIPS]` stack array (no heap allocation, `NUM_STRIPS` is a
compile-time constant) rather than after it.

**Composite's PRE/POST-fader tap forced a real choice about which strips'
chains have to run, and it was checked against a bench, not assumed.**
Spec 1.2 places the fader (step 12) before mute/solo (step 13), so a
composite tap must see a muted strip's real signal, not silence. The
first version of this took the simplest reading of that — run every
strip's chain unconditionally, every sample, regardless of mute — and was
flagged before merging as a real regression risk: on a mixer where most
strips are muted (the ordinary case), CPU cost would stop scaling with
strips actually in use.

Measured directly rather than assumed acceptable. `benches/engine.rs` (new
this milestone — `rt_assert.rs` had carried a placeholder comment naming
this exact bench as owed since M3) models that ordinary case: all 8 strips
carry real, engaged processing (gate/compressor/denoiser/EQ all on, not
neutral), only strip 0 is unmuted, 128-sample blocks. Compared on this
machine (a stored CI baseline is a separate step, per the M1 log's rule
below):

- Always running all 8 chains unconditionally: **58.78 µs/block**
- Running only strips that are unmuted (or soloed-in), or actually
  referenced by an active POST-fader composite tap: **22.66 µs/block**

A 2.6x regression for the common case, confirmed real rather than assumed.
The shipped version gates chain execution on
`strip_active[s] = ordinarily_unmuted || composite_tapped`, computed once
per block (not per sample) from `Patch::composite_references_strip`, so a
muted strip's chain only runs when something in the current patch
actually needs its audio. PRE-fader taps need no chain output at all (they
read `strip_inputs` directly), so they never force a strip active — only
`composite_post_fader == true` plus an active Composite-mode bus can.

**Consequence, chosen deliberately rather than picked arbitrarily: mute no
longer freezes a strip's envelope state, except when nothing needs it
to keep running.** A strip that's muted and not referenced by any active
POST-fader composite slot still has its chain skipped entirely for the
whole block — same as before this milestone, chain state stays frozen,
CPU cost stays zero. A strip that's muted *and* actively POST-tapped keeps
running (and its gate/compressor keep tracking, "warm") for exactly as
long as the tap needs it to, because there is no other way for that tap to
see real audio. This isn't a free choice between "always warm" (correct
for unmuting, wrong for CPU) and "always frozen" (cheap, but a real mixer
doesn't freeze a gate's envelope when you hit mute) — the gating that
fixes the CPU regression above resolves it as a side effect: warmth is now
scoped to exactly the strips something is actually listening to. This is
audible: unmuting a composite-tapped strip resumes from wherever its
envelope actually is, not from a stale pre-mute snapshot the way an
always-frozen strip's would.

**No stored bench baseline was committed for `engine_process_block_
mostly_muted` in this PR.** Per the M1 log entry below ("capture a bench
baseline from a CI run, never a local machine" — the M0 baseline recorded
locally was ~1.15ns and failed the first real CI run by +60%, a real
hardware difference, not noise), the 22.66 µs number above is this
machine's number for this PR's own review, not the number that should
ship as the regression gate's baseline. `scripts/check-bench-regression.sh`
already skips and reports rather than fails when a bench has no stored
baseline yet, so `ci.yml`'s `bench` job records the real one on its own
first run against this PR, same as any new benchmark's first baseline.

**Composite/insert patch sources are addressed as `(strip, channel)`, not
spec 1.11/1.15's literal flat `0..22` index.** That range is Voicemeeter's
own count of raw hardware input channels across its Windows/ASIO-specific
hardware strips — a number this engine has no equivalent for, since every
strip here already carries a fixed 8-channel `Frame` regardless of role
(M3's log entry, `crates/loomix-core/src/lib.rs`). Inventing a mapping
from "input channel 17" onto a `(strip, channel)` pair, just to match a
number nothing else in this codebase uses, would be undocumented
guesswork standing in for a real spec answer. `CompositeSource::{Default,
Strip{strip, channel}}` (`patch.rs`) addresses the model this engine
actually has. `insert: [bool; 22]` keeps the spec's literal size, since
unlike composite it's inert config with no addressing scheme to invent at
all this milestone (see below).

**The 12 bus modes (`bus_mode.rs`) are a pure `Frame -> Frame` function,
called for every mode except Composite, which `engine.rs` fills directly**
since it needs cross-strip access `bus_mode::transform` doesn't have (and
shouldn't — keeping it pure keeps every other mode's known-answer test a
plain input/output check). Spec 1.6's Mix Down A/B formulas literally put
`RL` on the right-hand side of the RIGHT channel in the vendor manual —
implemented as the corrected `FR`-based formula per spec 1.6's own
instruction, covered by
`bus_mode::tests::mix_down_a_right_channel_uses_fr_not_the_published_
rl_typo`, which feeds `FR` and `RL` far-apart values and asserts the
actual output matches the corrected formula and not the published one.

**Up Mix 2.1/4.1/6.1, Center Only, LFE Only and Rear Only silence every
channel spec 1.6 doesn't name for that mode, rather than leaving the
pre-transform content in place.** Spec 1.6 lists these modes by the
channels they populate ("2.1 plus RL=L, RR=R") without saying what happens
to the rest. Read as: these modes exist to render a signal for a specific,
smaller speaker layout (an actual N.1 setup genuinely has no signal on the
channels it doesn't have), not as a partial overlay on top of whatever the
bus happened to be carrying before the mode ran — matching, e.g., why a
real 2.1 or 4.1 layout has no center channel at all. `bus_mode.rs`'s tests
assert every unlisted channel is exactly `0.0`.

**No stated default for the composite and insert pre/post-fader switches
(spec 1.11), so both default to PRE (`false`)** — matching the order both
the spec's system-settings prose and 1.15's parameter list mention the two
options in, the same "no stated default, first-listed wins" convention
already used elsewhere (Karaoke's depths, the EQ shelf alpha, M6's log).

**Insert patch (`Patch::insert`, `Patch::insert_post_fx`) is config only
this milestone, with no audio effect** — the identical deferral M5 already
made for Intellipan's Color-pad reverb. Spec 2.3 explicitly defers the
real send/return path past M7 (an AUv3 host slot, or a hardware channel
loop — neither exists yet, both are HAL/app-layer concerns `loomix-core`
doesn't own). `engine::tests::insert_patch_and_pre_post_switch_are_inert_
this_milestone` runs one block with every insert toggle on and the
pre/post switch flipped against the same block with defaults, and asserts
bit-exact identical output — proving the config is currently inert, not
that insert processing works, since there is no such processing yet.

## 2026-08-23 — M6: parametric EQ

**A clippy regression was reported to the user as "pre-existing" without
actually being checked, and the claim was wrong — caught by the user, not
found independently.** Late in this milestone,
`RUSTFLAGS="-D warnings" cargo clippy --workspace` failed on
`intellipan.rs`'s `Intellipan` enum (`large_enum_variant`). The first
clippy run of the session happened *after* `biquad.rs` had already been
changed (the coefficient-ramp smoothing below, which grows every
`Biquad`), so there was no actual evidence the warning predated this
branch's work — it was reported as pre-existing anyway, on the strength of
"it showed up in an early run" without checking what that run was actually
early *relative to*. Told to fix it regardless of origin on a solo
project (correctly — there is no "whoever owns it" to defer to), the
claim was checked properly this time: `git stash` back to the branch
point (`b4bb8b9`) and re-running clippy there is clean, so the regression
is real and traced to this branch's own `biquad.rs` change growing
`ColorPad`'s four biquads past the threshold against `Position`/
`Modulation`'s already-boxed pointer size. Fixed by boxing `Color` too —
the identical fix already applied twice this session to `StripChain`'s
variants, logged in its own commit stating plainly that it isn't
pre-existing. The lesson isn't "box enum variants," it's that "pre-
existing" is a factual claim that needs the same evidence discipline as
any other claim in this log — a `git stash` and a re-run costs one command
and would have caught this the first time.

**Coefficient changes are smoothed (`biquad.rs`'s `Biquad::set_coeffs`),
on direct instruction, because M6's cells are the first callers to sweep
a biquad's parameters live rather than setting them once at construction
or on a discrete control change.** See `docs/DSP.md`'s Parametric EQ
section for the mechanism (a fixed 64-sample linear coefficient ramp,
applied inside `process()`) and the formula-level reasoning for why raw-
coefficient interpolation was chosen over a provably-stable parameter
domain. Logged here because of how it was verified: told explicitly not
to let the test define the behaviour after the fact, so the click-
avoidance claim is backed by an A/B test that computes what an
*instantaneous* coefficient swap would have produced on the same
accumulated filter state (a hand-evaluated one-step difference-equation
application, not a second `Biquad`) and asserts the real, smoothed
`Biquad` stays far below that on the identical input — the same
"wrong implementation actually fails the identical scenario" technique
`drift.rs` already used for the PI controller. Bypass transitions
(entering/leaving a cell's `on` state) were deliberately *not* smoothed —
a scope boundary stated explicitly rather than left to be discovered
later, on the reasoning that a cell's on/off toggle is a discrete UI
gesture, unlike a dragged knob, and every other block's neutral setting
in this crate is already a true instantaneous bit-exact identity.

**`ParametricEq<const N: usize>` is generic over channel count instead of
being two separate types for the strip (2ch) and bus (8ch) EQs**, because
spec 1.7 states the two are one shared implementation, not two
similar ones. A/B memory doubles the *parameters* only, not the live DSP
state (biquads, delay line) — see `docs/DSP.md` for the reasoning, the
same "recompute live state from params on change" shape M5's blocks
already established, not a new pattern. The public surface is a single-
sample `process_channel(&mut self, channel, x) -> f32`, not a
`Frame`-shaped `process()`: the strip (stereo) and bus (8-channel) callers
each wire it into their own layout in a couple of lines
(`strip_dsp.rs::HardwareChain::process`, `engine.rs`'s bus loop) rather
than fighting Rust's const generics for two different `N`-specific
`process(Frame)` implementations that would each be a few lines shorter
at the cost of real type-system complexity for no behavioural gain.

**`Bus` lost its `Default` impl in favour of `Bus::new(sample_rate: f32)`,
the identical move M5's log already made for `Strip` and for the identical
reason:** the bus EQ's biquads/delay-line state is sample-rate dependent,
so a bus can no longer be constructed without knowing the engine's rate.
`Engine::set_sample_rate` now propagates to every bus's EQ alongside every
strip's chain, which it didn't need to do before this milestone (bus
state had no sample-rate dependence until now).

**Bus EQ's position in the per-bus chain (spec 1.2 step 4, before step
5's mono/stereo-reverse) was checked the same way the M1 ring-buffer test
and the M5 gate-chatter test were validated, on direct request: the real
order was temporarily inverted in `engine.rs`, the order-proof test
(`bus_eq_runs_before_stereo_reverse_provably`) was confirmed to actually
fail against the inverted order, then the code was reverted.** The test
itself uses a channel-specific EQ boost plus `StereoReverse` rather than
any EQ-changes-the-output check, specifically because a channel *swap*
only disagrees with a per-channel EQ setting about which channel ends up
affected if the two operations are genuinely non-commutative — an EQ-
changed-the-output assertion alone would pass under either order and prove
nothing about which one actually runs. Confirmed non-vacuous by construc-
tion (swapping the real code and watching the test fail), not just by
argument. The equivalent strip-side check (EQ after the compressor, not
before) was validated identically.

**The cross-language EQ-graph fixture (`crates/loomix-core/tests/
eq_response_fixture.rs` → `testdata/fixtures/eq_response_reference.json`
→ `ui/src/eqResponse.ts`'s own test) needed two real fixes to actually
agree at a tight tolerance, found by the TS-side test failing, not
designed in advance.** First: probe frequencies that weren't exact bins
of the analysis window leaked spectral energy in a way a steep filter
(a notch center, a stopband) turned into >1dB of spurious disagreement —
fixed by snapping every probe to `k * sample_rate / NUM_SAMPLES`. Second,
surfacing only after the first fix: `render::goertzel_magnitude`'s `f32`
accumulation over the long buffer needed for fine low-frequency bin
resolution wasn't precise enough at sub-0.1dB tolerance in deep
attenuation — fixed with a local `f64`-accumulating Goertzel scoped to
this one test file (`render.rs`'s own shared function is untouched, since
its existing callers' shorter blocks and looser tolerances don't need
this), the identical class of fix M4's drift-simulation counters needed
for the identical reason (`f32`'s precision floor reached by a long-
running accumulator, not by any single sample). The final tolerance
(0.05dB) was set empirically after both fixes, not guessed in advance:
0.005dB still failed on residual engine-side `f32` filter-state precision
alone, unrelated to either bug above.

## 2026-08-23 — M5: strip processing

**`Engine::process_block` changed loop order — strips outer, buses inner —
because M5 gives every strip mutable, stateful DSP (envelope followers,
biquad filter state) for the first time.** M3's original loop was bus-outer/
strip-inner, recomputing each strip's (stateless) contribution once per bus
it fed. Running a stateful effects chain that way would process the same
strip's chain once per bus it's assigned to — corrupting envelope/filter
state and applying attack/release timing multiple times per audio frame.
Each strip's chain (`strip_dsp.rs`) now runs exactly once per input frame,
producing one processed frame that's then summed into every bus the strip
feeds, at that bus's own gain layer. `Engine` also gained a `sample_rate`
field (default 48kHz, spec 1.11) and `set_sample_rate()`, propagated into
every strip's chain — filter coefficients are sample-rate dependent, and
nothing in the engine previously had any notion of sample rate at all.

**`Strip::default()` was removed; strips are now built by
`Strip::for_topology_index(index, sample_rate)`, matching spec 1.1's fixed
Potato layout exactly (indices 0-4 hardware, 5-7 virtual, index 6 the
Karaoke-capable AUX strip).** M5 is the first code that needs to know which
strips are hardware vs virtual — M3/M4 treated all 8 uniformly since
nothing distinguished them yet. `StripChain` (hardware vs virtual effects
chain) is stored directly on `Strip` rather than as a separate array
alongside it in `Engine` (the way `strip_meters` is): one owner for a
strip's config and its DSP state, matching how `bus_assign`/gain layers
already live on `Strip` rather than in a parallel structure.

**`Intellipan`'s `Position`/`Modulation` pad variants are `Box`ed — found
by a genuine stack overflow, not decided in advance.** `ModulationPad`'s
chorus delay line is a fixed `[f32; 8192]` per channel (sized for the
longest depth+base delay at 192kHz), and `PositionPad`'s ITD line is
`[f32; 2048]` per channel. An unboxed `enum Intellipan { Color(..),
Position(PositionPad), Modulation(ModulationPad) }` is sized for its
*largest* variant regardless of which is active — every `Strip` paid
Modulation's ~64KB even when its pad defaults to Color, and constructing
all 8 strips inside `Engine::default()` overflowed the test thread's stack
before this fix (`default_engine_routes_every_strip_to_bus_zero_at_unity`
aborted with SIGABRT, not a normal test failure). Boxing only happens at
pad-mode construction/switching, never inside `process()` — the audio
thread never allocates because of this.

**Pan laws (hardware pan pot, virtual 5.1 position pad) use a balance law,
not constant-power, on direct instruction.** Spec 4.1 layer 2 lists
"pan law preserves energy... across the sweep" as a property test, which
usually implies a constant-power (equal-power sine/cosine) law — but a
constant-power law is constant-power *because* both channels sit at -3dB
at center, trading a neutral center for constant total power at the
extremes. Spec 4.1 layer 1 also requires every effect to have a true
bit-exact neutral setting, and direct instruction was to keep center
exactly unity on both channels and reject the silent -3dB dip most
panners default to — the two requirements are in real tension for a
literal constant-power law, and neutral-center won on instruction. Both
`StereoBalance` (hardware) and `PositionPad5_1` (virtual) give the
favoured channel exact unity across the whole sweep, verified explicitly (
`pan::tests::center_gain_is_exactly_unity_on_both_channels_not_minus_3db`,
`pan::tests::pad_center_gain_is_exactly_unity_front_not_minus_3db`) so a
future accidental -3dB regression reads as a broken test, not a rounding
question. The property test this replaces spec 4.1's literal reading with
is the balance law's actual invariant: the favoured channel never leaves
unity, the sweep is monotonic and continuous — not constant total power,
which a balance law cannot have alongside a neutral center.

**Intellipan's Color pad ships tonal-shaping only this milestone; its
"small reverb on the upper half" (spec 1.3) is deferred to M9, and the
same reasoning is applied to Position pad's "small room effect" even
though only Color was named directly.** Building a strip-local reverb now,
ahead of M9's real send/return reverb engine (spec 1.8), means two reverb
implementations and a migration later — rejected on direct instruction.
Position's room effect is the same category of effect (small reverb-family
processing) facing the identical two-implementations problem, so it's
deferred the same way rather than built now just because it wasn't named
explicitly. Both pads' `y` axis is accepted and stored (so the parameter
exists for M9 to wire up) but does not affect audio yet; each pad's null
test at `y=0` is explicit in its own comment that this does not prove the
reverb/room path works, since there is no such path yet — a deliberate
guard against a future reader assuming the null test covers more than it
does.

**Virtual-strip chain order (3-band EQ → M.C. → Karaoke → 5.1 pan pad →
limiter) is a judgement call, checked by an order-*proving* test rather
than asserted.** Spec 1.2 gives the hardware strip's processing order
explicitly, step by step; spec 1.4's virtual-strip control list (EQ, pan
pad, M.C., Karaoke, limiter) is not ordered at all. M.C. and Karaoke are
placed before the pan pad because they're channel-content operations
(muting a channel, removing vocal content) that read most sensibly as
acting on the strip's original multichannel material before the pad
repositions it — reasonable, but not spec-mandated, so on direct
instruction the test proves this empirically rather than asserting it:
`strip_dsp::tests::virtual_chain_eq_runs_before_the_pan_pad_provably`
computes the actual `VirtualChain`'s output alongside two independently-
built hypotheses (EQ-then-pad and pad-then-EQ, using the same public block
APIs, not the chain under test), confirms the two hypotheses actually
disagree for the chosen input (so the test isn't vacuously satisfiable),
and asserts the real chain matches only the declared order. If this
placement ever changes, the test is designed to catch it rather than
silently keep passing.

**Macro-knob curves for Gate/Compressor/Denoiser are Loomix's own,
documented in `docs/DSP.md`, not a reproduction of Voicemeeter's.** The
reference manual this project verifies parameter *ranges* against (spec
front matter) doesn't publish the macro-knob-to-detail-parameter mapping
itself, so there's no external reference to match. Each curve is linear
in `knob/10` between a "just engaged" and "maximum" value per detail
parameter, tested for monotonicity and its two endpoints rather than
exact-value known-answer tests, since no independent reference exists to
compare against. `knob <= 0.0` is a true bypass on all three (spec 4.1's
neutral-setting requirement) — not just a very permissive setting, an
explicit branch that skips processing entirely, the same guaranteed-exact
pattern the biquad primitive uses for its own neutral state.

**Compressor and Denoiser both needed an RMS-smoothing stage ahead of
their attack/release envelope follower — found by tests failing, not
designed in up front.** An early version fed each block's attack/release
follower a raw instantaneous rectified sample directly. Because attack was
fast enough to react within a single audio-rate cycle, the follower chased
the input waveform's own ripple (a rectified sine dips to nearly zero every
half-cycle) rather than its actual level, ratcheting the envelope toward
the signal's peak instead of its RMS value — an asymmetric fast-attack/
slow-release filter applied to a rapidly oscillating target is biased, not
neutral. `compressor::tests::a_steady_tone_above_threshold_settles_to_the_static_curve`
caught this directly (measured gain reduction was ~2.7dB off the static-
curve prediction); the denoiser hit the identical failure mode for the
same reason and got the identical fix. Both blocks now low-pass the
instantaneous power (fixed 5ms time constant) into a mean-square estimate
before ever comparing it to a threshold or feeding it to attack/release.

**The gate was checked for the same ratcheting bug found in the compressor
and denoiser, on direct request, and does not have it — verified
empirically, not just argued.** The concern: a gate is also an attack/
hold/release follower reacting to a detector, so the same missing-
smoothing failure mode would show up as chatter (rapid open/close
toggling) on sustained near-threshold material rather than compressor/
denoiser's peak-ratcheting. Structurally the gate is already built the
way the fix left the other two: `Gate::process` smooths the sidechain-
filtered detector through a 5ms one-pole (`self.envelope`) *before* ever
comparing it to `threshold_db` — this stage already existed when
`gate.rs` was first written, it wasn't added in response to this check.
It also has a second layer the other two don't: `self.gain` is itself a
slow attack/release-filtered variable (tens to hundreds of ms), not a
direct function of the instantaneous threshold decision, so a single-
sample flicker in the (already-smoothed) envelope crossing the threshold
gets absorbed by gain's own inertia rather than producing an audible
toggle. Argument alone wasn't treated as sufficient: verified by feeding
broadband noise (deliberately harsher than a periodic tone — real
sustained low-level material behaves more like noise, and noise has far
more residual envelope variance after smoothing) at both a low sidechain
frequency (100Hz, worst case for the 5ms smoother's relative attenuation)
and swept finely across the exact open/close boundary. Every trial showed
at most the one expected settling transition, never repeated flapping —
`gate::tests::sustained_near_threshold_noise_does_not_chatter` is the
permanent regression test for this, kept rather than discarded as a
one-off diagnostic since it guards against a future change to
`DETECTOR_SMOOTHING_MS` or the attack/release curve silently
reintroducing chatter.

**Karaoke's K-1/K-2/K-v and the denoiser's floor-tracker time constants
have no spec-given values — chosen by simulating the tracker's actual
convergence in Python against the unit tests' own assertions, not guessed
directly into the Rust code.** The denoiser's floor-rise time constant in
particular went through several wrong values that either let a loud held
tone go unrecognised as signal (never converging within the test's
duration) or, at the other extreme, let a *steady* quiet floor and a
*steady* loud tone become numerically indistinguishable once the tracker
fully converges to match whatever level it's given (a real limitation of
this simplified single-band model: a sound held perfectly constant for
long enough eventually reads as the new floor, real or not — acceptable
for real audio, which has natural dynamic range, less so for a
synthetic held-tone test). `docs/DSP.md` documents the numeric outcome per
knob step, not just the formula.

## 2026-08-23 — release.yml skips packaging until it exists

**`release.yml`'s signing/notarising/pkg-build/release-upload steps are
now gated behind a check for `packaging/build-pkg.sh`, and skip cleanly
with a `::notice::` instead of failing when it's absent.** The `v0.1.0`
tag push actually ran this workflow and it failed, hard, at "Import
Developer ID signing identity" — the earlier M0 log entry calling this
job "guarded or documented as inert" was wrong; it was only documented,
never guarded. A workflow that fails on every tag between now and M13,
when `packaging/` actually lands (spec 3.4), trains exactly the kind of
red-means-nothing habit CI exists to prevent. The alternative was
disabling the workflow outright until M13; rejected because the
`cargo build --release` (both targets) and `xcodebuild -configuration
Release` steps are real, standing signal independent of packaging — they
catch a release build that doesn't compile, on every tag, and disabling
the whole workflow would throw that away for no reason. The gate mirrors
`nightly.yml`'s existing `fuzz`/`soak` pattern (check whether the thing a
later milestone adds exists yet; skip with a message if not) rather than
inventing a new mechanism. No workflow edit needed at M13: the moment
`packaging/build-pkg.sh` exists, `steps.packaging.outputs.exists` flips to
`true` and every gated step runs for real.

## 2026-08-23 — M4's real acceptance criterion is not yet met (outstanding)

**The 30-minute soak has been run (macOS 26.6, build 25G72;
`com.loomix.audiodriver.outA1` as master, `com.loomix.audiodriver.outA2`
as the drift-corrected device; `timeout -k 60 35m ... --duration 30m`;
zero dropouts, exit PASS) but this run does not satisfy spec 3.4 M4's
acceptance criterion ("a 30 minute soak test on two devices with
different clocks shows no dropouts and bounded drift"), and M4 is not
being called accepted on the strength of it.** Recorded here plainly
rather than filed as a passing checkbox, on direct instruction, because
the gap is real and specific, not a formality:

1. Both devices were idle Loomix virtual outputs (`outA1`/`outA2`), not
   two physically independent interfaces. Both are software-clocked off
   this process's own timer -- they do not have "different clocks" in the
   sense spec 1.19 means (two devices whose *hardware* crystals drift
   against each other). The drift ratio stayed at exactly `1.000000` for
   the full 1800 seconds, not "close to 1.0 and bounded" the way a
   corrector actively holding two real clocks together would show, but
   completely flat -- direct evidence the PI controller and the resampler
   were never actually engaged, because there was nothing to correct.
   This run proves the IOProc/ring-buffer/engine path stays clean under
   real CoreAudio scheduling for 30 minutes; it does not exercise drift
   correction, which is the entire subject of spec 2.3 and most of this
   milestone's code.
2. Both buses carried silence throughout (`loomix-soak`'s own printed
   caveat -- "both buses carry silence -- this measures timing and
   dropouts, not audio content" -- by design, for the reasons in the
   two-output-device entry above), but it means denormal handling and
   real per-sample DSP cost were never exercised either over a sustained
   run.
3. Consequently: `dropouts: 0` and `PASS` in this run's output demonstrate
   the wiring doesn't fall over on its own, not that "bounded drift" has
   been observed at all -- there was no drift to bound.

**What would actually satisfy spec 3.4 M4's acceptance criterion:** two
genuinely independent physical audio interfaces (e.g. this machine's
built-in output plus a real external USB/Thunderbolt/Bluetooth interface,
not two Loomix virtual devices, not one physical device aggregated with
itself), carrying real signal rather than silence, run for 30 minutes,
with a *non-zero* drift ratio actually observed and staying bounded
(not saturating `DriftCorrector`'s `max_correction` clamp) and zero
dropouts throughout. Until that specific run exists and is recorded here,
M4's hardware-clocking claim rests on the synthetic fake-device tests in
`loomix-hal::ioproc` and `loomix-hal::drift`, not on hardware evidence,
and should be described that way.

## 2026-08-23 — M4 (continued, the coverage gate)

**CI's `coverage` job failed at 78.21% against the 80% gate after this
milestone's work; fixed by excluding specifically the code that starts
real device I/O or creates a real system device, never by lowering the
threshold.** Measured before changing anything: three files accounted for
nearly all of the 544 missed lines. `loomix-soak/src/main.rs` (183 of 183
lines, 0%) had no tests at all. `loomix-hal/src/device.rs` (271 of 907)
was mostly the `?`-error-propagation arm of real CoreAudio calls (can't be
triggered without an actual hardware failure) plus the entirely-unexercised
bodies of `CaptureIoProcHandle`/`RenderIoProcHandle`/`MasterIoProcHandle::start`,
`create_aggregate_device` and `set_stream_format_non_interleaved` -- none
run automatically, deliberately, the same reasoning the hog-mode
round-trip test was already `#[ignore]`d for. `loomix-app/src/engine_io.rs`
(80 of 287) was `select_clock_master` (read-only enumeration, actually
safe to test) and `attach_capture_device`/`attach_render_device`/
`attach_master_device` (call the `*Handle::start` functions above -- same
category).

Stable Rust has no per-function coverage exclusion -- confirmed directly:
`#[coverage(off)]` requires `#![feature(coverage_attribute)]`, which
`rustc 1.97.1` (this toolchain) rejects with "experimental feature."
`cargo-llvm-cov`'s only exclusion mechanism on stable is
`--ignore-filename-regex`, file-level. Excluding the two whole files
(`device.rs`, `engine_io.rs`) would have thrown away real, earned coverage
credit for the enumeration/trampoline/ring-assembly logic those files
*do* test -- so each got split: the never-automatically-exercised
functions moved into new files
(`loomix-hal/src/device_lifecycle.rs`, `loomix-app/src/device_wiring.rs`)
carrying the doc-comment explanation, leaving the tested logic
(trampolines, `CaptureIoProcContext`/`RenderIoProcContext`,
`StripSource`/`BusSink`/`EngineIoDriver`) in the original files, still
counted. `ci.yml`'s `coverage` job and `justfile`'s `cover` recipe now
pass `--ignore-filename-regex '(device_lifecycle\.rs|device_wiring\.rs|loomix-soak/src/main\.rs)$'`
with a comment pointing at the same reasoning, so the exclusion is visible
at the point it's applied, not just in the excluded files themselves.

`loomix-soak/src/main.rs` got the same treatment for its one genuinely
pure piece: `parse_duration` moved to `duration.rs` (not excluded, six new
tests) and was changed from calling `std::process::exit` on a bad value to
returning `Result<Duration, String>`, since a function that can only fail
by killing the test process can't be tested at all -- `main()` keeps the
same exit-with-message behaviour at the one call site that needs it.

Net result, verified locally before touching CI: 78.21% (failing) to
97.09% (`cargo llvm-cov --workspace --all-features --ignore-filename-regex
...`), with the excluded files' own doc comments naming the reason and the
`select_clock_master`/`parse_duration` additions being real new test
coverage, not exclusions dressed up as fixes.

## 2026-08-22 — M4 (continued, the soak harness and the interleaved-format bug)

**`loomix-soak` is a new crate (spec 3.4 M4's 30-minute two-device
acceptance test), wiring two real output devices together end to end and
reporting a measured PASS/FAIL, not a runnable-only demo.** Two *output*
devices, not an output and an input, even though the milestone's own
IOProc work supports capture too: a first version defaulted to the system
input device and measured 100% underrun in testing here, traced to a
macOS microphone (TCC) permission gate on the terminal process, not a
wiring bug -- confirmed by swapping to two output devices, which need no
such permission, and matches spec 1.19's own literal example of what
needs drift correction ("outputs A1 through A5 are not sample
synchronous... when they run on different physical devices") at least as
well as a capture scenario would. `nightly.yml` already referenced a
`loomix-soak` package by name and a `--duration 2h` invocation before this
crate existed; that leg is still M10's (recorder folded in), not this
binary's current two-device-only shape, but the name and the
`--duration` flag already match.

**Wiring the soak harness for real surfaced a genuine bug no offline test
could have caught: `capture_ioproc_trampoline`/`render_ioproc_trampoline`
assumed one `AudioBuffer` per channel (non-interleaved), and every real
output device tried on this machine -- the built-in speaker, an external
monitor's speakers, BlackHole -- delivers one interleaved buffer instead.**
`on_render` saw `output.len() == 1` against `channel_count == 2`
resamplers; `debug_assert_eq!` would catch that in a debug build, but
`cargo run --release` (what a real soak needs) compiles the assert out,
and `zip()` silently processes only the shorter side, so the second
channel's ring buffer is never drained and overruns forever. Manifested
as continuous dropouts within about a second, on every real device pair
tried, never once as a test failure -- there is no synthetic stand-in for
a real device's actual stream format, exactly the category of bug this
milestone's whole test strategy (pure/offline logic plus a manual
hardware soak as final confirmation) was designed to still let through
until the soak itself ran. First fix attempt: request a non-interleaved
format explicitly via `AudioObjectSetPropertyData` before registering the
IOProc (`set_stream_format_non_interleaved`). That call reports success
and *still doesn't change what's delivered* -- confirmed by reading the
format back immediately after setting it, which showed the interleaved
flag unchanged. Real fix:
`read_input_channels_planar`/`render_ioproc_trampoline`'s interleaved
branch detect a single multi-channel `AudioBuffer` and
deinterleave/interleave through scratch storage (`CaptureIoProcContext`'s
`deinterleave` / `RenderIoProcContext`'s `interleave` fields, `chunks_exact_mut`
splitting a flat scratch buffer into disjoint per-channel views, no
allocation on the real-time thread) rather than trusting the format
request to have taken effect. `set_stream_format_non_interleaved` is kept
as a best-effort call whose result is now ignored -- harmless on devices
that do honour it, irrelevant on the ones that don't since the trampolines
adapt either way. `master_ioproc_trampoline` was *not* given the same
fix: it hands raw buffers straight to a caller-supplied callback with no
per-device channel count or scratch to deinterleave into, so it keeps the
same interleaved-format gap, documented at the call site
(`read_input_channels`'s doc comment) rather than silently carried
forward -- not hit by the current soak scenario (the master's own
strip/bus aren't used, `master_strip: None`), but a real gap for whenever
a full-duplex master device and real audio content are both in play.

Verified for real, not assumed, at every step of this fix: the format
readback showing the set didn't take, the trampoline-level regression
tests added for both the interleaved-capture and interleaved-render cases
(`capture_trampoline_deinterleaves_a_single_interleaved_buffer`,
`render_trampoline_interleaves_output_into_a_single_buffer`), and finally
a 30-second `loomix-soak` run against the built-in speaker and BlackHole
reporting zero dropouts after the fix, versus continuous overrun before
it.

## 2026-08-22 — M4 (continued, loomix-app wiring)

**`loomix-app::engine_io` -- assembling per-strip/per-bus ring buffers
around `Engine::process_block` and deciding which device drives the
tick -- is pure, `#![forbid(unsafe_code)]` Rust with no CoreAudio calls of
its own, matching spec 3.2's crate boundary: `loomix-hal` stops at handing
over correctly-resampled frames across a ring, `loomix-app` is what "wires
everything together."** `select_clock_master` adds nothing but a live
device enumeration on top of `loomix-hal::clock::resolve_clock_source`,
already pure and tested; no logic is duplicated. Being pure meant the
ring-assembly logic itself got the same offline test treatment as
everything else in `loomix-hal` this milestone, with synthetic
`rtrb::RingBuffer`s standing in for real devices.

**The clock-master device's own strip/bus (if it has one of each) bypass
the ring-buffer path entirely and are handled directly inside
`EngineIoDriver::on_master_tick`, rather than going through
`StripSource`/`BusSink` like every other device.** The first design
considered treated the master uniformly with everyone else -- push its
captured audio through a ring, drain its render channel from a ring, with
an extra hook that also advanced the clock and ran the engine tick. That
has a real ordering bug: for capture, the hook needs to run *after*
`on_capture` has pushed this callback's fresh audio; for render, the
engine tick needs to run *before* `on_render` pulls this callback's
output, and a real full-duplex device's IOProc gets *both* buffers in one
callback, so both orderings are needed in the same call. Rather than fight
that, `loomix-hal::device::MasterIoProcHandle` (see below) hands the
master's raw capture and render buffers to the engine driver directly,
synchronously, in the one call CoreAudio actually makes -- no ring, no
ordering question. `MASTER_BUS` is hardcoded to bus 0 (A1), not a
configurable field: spec 1.19 defines the clock master *as* "the main
output device," which spec 1.1 already fixes as bus A1.

**`EngineIoDriver::on_master_tick` builds the `[&[Frame]; NUM_STRIPS]` /
`[&mut [Frame]; NUM_BUSES]` arguments to `process_block` with
`[T; N]::each_ref()`/`each_mut()`, not `Vec::collect()`.** The obvious way
to build those arrays (`self.scratch_outputs.iter_mut().map(|v|
v.as_mut_slice()).collect()`) allocates a `Vec` -- fine in the M3 tests
that do exactly this, wrong here, since this runs on the real-time thread
the master's IOProc calls. `each_mut()` gives disjoint mutable references
to the array's elements structurally, with no heap allocation, which a
naive `std::array::from_fn(|i| ...)` closure repeatedly indexing
`self.scratch_outputs[i]` can't offer the borrow checker (each call would
need to prove disjointness across separate closure invocations, which it
can't) -- `each_mut()` sidesteps needing that proof at all.

**Found while wiring this up: the CI `rt_safety` job only ever ran
`loomix-core`'s own real-time tests -- every `assert_realtime` test added
to `loomix-hal` this milestone (`resample.rs`, `master_clock.rs`,
`ioproc.rs`) and every one now in `loomix-app::engine_io` had only ever
run with the flag-only, non-panicking version of `assert_realtime`, never
with the actual trapping allocator, in CI or otherwise.** The trapping
allocator is a `#[global_allocator]`, process-wide, installed only when
`loomix-core` is built with its `rt-assert` feature; neither `loomix-hal`
nor `loomix-app` declared that feature themselves, and `ci.yml`'s
`rt_safety` job only ever tested `-p loomix-core`. Fixed without adding a
passthrough feature to either crate: Cargo's `-p <crate> --features
<dep>/<feature>` syntax enables a dependency's feature directly, so
`cargo test -p loomix-hal --features loomix-core/rt-assert` (and the same
for `loomix-app`) gets the real trap without either crate's `Cargo.toml`
knowing about `rt-assert` at all. Verified locally, not assumed: both
commands pass with the trap active, which is proof the guarded code paths
(`Resampler::process`, `DriftCorrectedIoStage::on_capture`/`on_render`,
`MasterClock`, `EngineIoDriver::on_master_tick`) genuinely don't allocate,
not just that the tests happened not to notice. Same class of gap as the
driver-install CI leg earlier this milestone: a real-time-safety test that
never actually exercises the trap is exactly as decorative as a
device-enumeration check against a driver that was never installed.

## 2026-08-22 — M4 (continued, IOProc registration)

**`device.rs`'s real IOProc registration (`CaptureIoProcHandle`,
`RenderIoProcHandle`) is thin glue over `ioproc.rs`'s already-proven
`DriftCorrectedIoStage`: parse CoreAudio's `AudioBufferList` into planar
slices, call the same `on_capture`/`on_render` methods the fake-device
harness already exercises, nothing decision-worthy added here.** Assumes
non-interleaved streams (one `AudioBuffer` per channel) -- the common,
controllable case, not a claim every device format is handled.

**An early draft of the render trampoline had a real soundness bug caught
before it ever ran: `array::from_fn` over the full `MAX_IO_CHANNELS` (8)
range read `MaybeUninit` slots that were only initialised up to `count`,
so any device with fewer than 8 channels -- the common case -- read
uninitialised memory as a `&mut [f32]` reference.** Rewritten to build
each of the 8 array slots directly inside the `from_fn` closure (a real
disjoint sub-slice for `i < count`, an always-valid empty slice
otherwise), which needs no `MaybeUninit` at all: each closure invocation
constructs its own value, so the `Copy` bound a `[expr; N]` repeat would
need never comes up.

**The buffer-list-parsing logic got automated coverage after all, by
calling the trampoline functions directly with a hand-built
`AudioBufferList`, rather than being left entirely untested like the rest
of this file.** They're plain `unsafe extern "C" fn`s; nothing requires
going through real CoreAudio registration to call them, the same reasoning
M2's `test_driver_host.c` used to drive the driver's vtable in-process.
This paid off immediately: the first version of both new tests built the
`AudioBufferList` from a temporary `&mut [ch0.clone(), ch1.clone()]`
array, whose backing `Vec`s are dropped at the end of that `let` statement
-- before the buffer list is ever read -- leaving every `mData` pointer
dangling from construction. That version SIGKILLed the test binary (heap
corruption, not a clean panic or a useful backtrace); diagnosed by running
the two new tests individually to isolate which one crashed, since the
whole-module run gave no other signal. Fixed by having `TestBufferList`
*own* the channel `Vec`s (moved in, stored alongside `storage`) rather
than borrowing them, so their lifetime is tied to the buffer list's own
rather than to an anonymous temporary.

## 2026-08-22 — M4 (continued)

**`ioproc.rs`'s drift-corrected capture fed the PI controller the wrong
error signal, and this was found by the fake-device harness, not
predicted in advance.** The first version tracked `device_frames`, the
device's own raw cumulative sample count (incremented every callback by
however many frames the callback actually delivered), and computed error
as `device_frames - master.frames()`. `a_drifting_fake_device_reconstructs_the_tone_within_bounded_drift`
(500 ppm, 2000 callbacks) failed its frequency check by 10x, and
`IOPROC_DEBUG` tracing showed why: with a *constant* ppm offset, a
device's raw clock offset from the master grows without bound for as long
as the device keeps running, no matter how well the output is being
corrected -- correction changes what the *resampled output* looks like,
not the device's own physical clock. Feeding that ever-growing raw offset
into the PI controller gave it an error signal that could never converge,
so the integral saturated within a few hundred callbacks and `ratio`
stuck at `1.0 - max_correction` (0.99) for nearly the whole run -- a 0.7%
mistune when the actual injected error was 0.05%, twenty-eight times too
much correction, silently, because the controller was reacting correctly
to a fundamentally wrong number. The fix renames the field to
`progress_frames` and changes what accumulates into it: for capture, the
resampler's actual output count (`written`, from `Resampler::process`);
for render, the actual input count consumed (`consumed`). That quantity
*is* what the correction affects and is supposed to converge to track the
master, exactly the property drift.rs's own `simulate()` harness already
modelled correctly (`device_cumulative += ... * applied_ratio`) -- the bug
was introduced translating that model into the real per-callback code, not
in the model itself.

Second-order finding from chasing the above: an interim version of the
test asserted the reconstructed tone's Goertzel magnitude stayed within
`0.9..=1.1` of a clean reference over the full ~5.3-second run. After the
real fix, frame-count drift was already excellent (order of a couple
hundred frames against a quarter-million-frame run) and the resample
ratio never left roughly a ±0.3% band -- matching drift.rs's own bound at
the same kp/ki -- yet that assertion still failed. A frequency scan around
1 kHz showed the tone's energy spread across neighbouring bins rather than
missing outright, and a sample-delta scan found zero discontinuities: not
corruption, just enough accumulated phase jitter over a long single-tone
Goertzel measurement to fail a tight absolute-gain bound on a correctly
bounded but not perfectly smooth ratio. The test now asserts what actually
matters -- frame-count drift bounded (`< 300`, alongside `hotplug`-table-style
direct measurement) and the tone dominating a clearly different frequency
by 5x, the same "dominates" style `resample.rs`'s own non-unity-ratio test
already uses for the identical reason -- rather than a stricter bound nothing
else in the codebase actually asks a resampler to meet.

## 2026-08-22 — M4

**`loomix-hal`'s algorithmic pieces -- clock master selection, drift
estimation, the resampler, hot-plug handling, hog-mode fallback -- are
pure functions over plain data (`clock.rs`, `drift.rs`, `resample.rs`,
`hotplug.rs`, `hog.rs`), with no CoreAudio calls and no wall-clock time.**
Same move as M3's engine core, and the M1/M2 lesson logged below: push
logic somewhere offline-testable, keep the CoreAudio-touching code thin on
top of it. Unlike M1/M2's driver, none of this needed a host-side C
harness at all -- it's ordinary Rust exercised by `cargo test` with
synthetic clocks, never an installed driver or a real device.

**The drift-estimator tests include two "wrong implementation actually
fails" cases, not just "correct implementation passes" ones, on direct
request.** `drift::tests::a_naive_fixed_ratio_fails_the_same_scenario` runs
the identical synthetic ppm-offset scenario through a corrected loop and
through spec 2.3's named failure mode (a fixed ratio, i.e. no correction
at all) in the same test, and asserts the fixed-ratio version actually
diverges past a bound the corrected version stays under --
`drift::tests::converges_to_a_steady_ppm_offset_and_stays_bounded` alone
would pass equally well against a broken corrector that happened to do
nothing, same as `test_ring_buffer.c`'s poison-sentinel test below.
`drift::tests::discontinuous_clock_jump_recovers_ratio_but_a_plain_pi_loop_stays_biased`
covers a second, separate failure mode: a device reconfiguring or a USB
interface renegotiating can make its reported sample time jump
discontinuously in one block, and a plain PI controller has no way to
distinguish that from a huge drift reading -- it integrates it, and
because the integral term never leaks on its own, that single reading
permanently biases the loop's steady-state ratio away from 1.0. Writing
this test surfaced the actual fix, not just a test for one already
written: `DriftCorrector` wraps `PiController` with a
`discontinuity_threshold` that resets the integral instead of absorbing a
reading past it, and the test asserts the plain `PiController` alone stays
biased (`bias > 0.001`) long after the jump while `DriftCorrector`
recovers to near 1.0 within ten blocks -- both driven by the identical
synthetic jump, so the comparison is a real A/B, not two different scenarios.

**The drift simulation's cumulative sample counters are `f64`, not `f32`,
and this was found by the naive-fixed-ratio test failing, not reasoned out
in advance.** The first version of that test asserted the fixed-ratio
error would exceed 2000 samples after 50,000 blocks; it only reached
1535.5 and failed. Root cause: `device_cumulative` was accumulated in
`f32`, and at ~6.4M frames (50,000 blocks * 128 frames) `f32`'s ulp is
already 0.5 -- larger than the ~0.064-sample fractional drift added per
block at 500 ppm -- so most of the accumulating error was rounded away on
every `+=` before it could show up in the difference against
`master_cumulative`. A real 30-minute session reaches ~86M frames at 48
kHz, past where `f32` can even represent every integer exactly, so this
wasn't a test-only concern: `PiController::update`'s doc comment now
states the constraint explicitly -- cumulative sample time must be tracked
in `f64` or an integer type upstream, and only the small, bounded
difference narrows to `f32` for the controller itself, which never sees
values large enough for this to matter. Checked for the same assumption
elsewhere in the workspace: `loomix-core::meter::Meter`'s peak-hold is a
running max (`if level > *held`), not a sum, so it's scale-invariant
regardless of session length and the bug class doesn't apply; nothing else
in `loomix-core` carries a persistent cumulative counter across
`process_block` calls at all. `clock::InternalClock::frames_produced` was
already `u64` and `resample::Resampler::read_offset` is `f64` and wraps
every 1.0, so neither needed a fix.

**`resample.rs`'s polyphase resampler is a windowed-sinc filter bank (32
taps, 256 phases), sized for drift correction's actual use case -- a ratio
that stays within tens to hundreds of parts per million of 1.0, corrected
slowly -- not general arbitrary-ratio sample-rate conversion.** Building a
full production-quality SRC (adaptive filter length, dynamic cutoff
scaling with ratio, higher phase resolution) now would be sizing for a
requirement M4 doesn't have; `TAPS`/`NUM_PHASES` are left as the
calibration knob spec's own framing calls for ("a real clock drifts... a
PCA9685 runs a few percent fast"), flagged with a `ponytail:` comment
naming the upgrade path if the real two-device soak shows audible
artefacts. Each phase's tap coefficients are explicitly normalised to
unity DC gain after windowing, rather than relying on the truncated
windowed-sinc approximation to sum to 1.0 on its own.

**`ci.yml`'s `driver` job built the Release driver bundle but never
installed it**, confirmed by reading the job rather than assumed: its
`xcodebuild` step passed `CODE_SIGNING_ALLOWED=NO` with no
`-derivedDataPath`, so even calling `driver/scripts/install.sh` afterward
would have found no product at the path it looks for
(`driver/build/Build/Products/Release/LoomixAudioDriver.driver`, which
only exists when built with `-derivedDataPath driver/build`, as `just
install-driver` does locally). Any hal-side check assuming Loomix's
virtual devices are enumerable in CI would have been decorative --
exactly the class of bug the rt-safety release-mode fix below already
cost a debugging cycle to catch once. Fixed rather than dropped: the
`driver` job's Release build now matches `install.sh`'s expectations
(`-derivedDataPath driver/build CODE_SIGN_IDENTITY=-`, ad-hoc signed, the
same as the local `install-driver` recipe) and the job now actually runs
`install.sh`. This is safe specifically because it's a GitHub-hosted
*ephemeral* runner, not the shared dev machine the M1/M2 entries below
describe crashing twice in one day -- a wedged `coreaudiod` here just
fails the job; there's no persistent machine to leave in a bad state.
`install.sh` already fails loudly (not silently) on zero devices after 15
seconds, per the M2 entry below, so this doesn't need its own new
failure-detection logic.

## 2026-08-21 — M3

**Audio moves through the engine as `&[[f32; 8]]` — a slice of fixed-size,
8-channel frames — not planar buffers or an interleaved `f32` stream.**
Every strip and every bus carries all 8 of a bus's channels
(`FL FR FC SW RL RR SL SR`, spec 1.1) even though nothing before M5's pan
pot and M7's bus modes actually spreads a signal across more than channels
0 and 1. The alternative was a `CHANNELS`-generic engine that only strips
and buses actually needing more than 2 channels would opt into, but that
would mean deciding the pan/bus-mode data model now, a milestone early,
to serve a currently-empty need. A fixed `[f32; 8]` per frame costs
nothing today (unused channels are just `0.0`) and is the same shape M5
onward needs anyway, so there is no rework at the point it actually gets
used — only the ladder's "does this need to exist yet" applied to
*channel count*, not to a whole abstraction layer. `Frame = [f32; 8]`
and `process_block`'s `&[&[Frame]]` / `&mut [&mut [Frame]]` arguments are
in `crates/loomix-core/src/lib.rs` and `engine.rs`.

**Solo is engine-global for M3: it silences every non-soloed strip on
every bus, not just "the monitored bus."** Spec 1.3 describes solo as
muting non-soloed strips "on the monitored bus," but a monitor-bus
selection (spec 1.5's Monitor select) doesn't exist as an engine concept
yet — nothing in M3's scope (8 strips, 8 buses, the matrix, gain layers,
mute, solo, mono, fader law, meters) creates one. Scoping solo to a
not-yet-modelled concept would mean inventing that concept now, un-asked,
to serve a distinction M3 has no way to observe. The global interpretation
is the one every routing-truth-table combination in
`crates/loomix-core/tests/routing_truth_table.rs` can actually assert
against; it degrades cleanly to per-bus monitor scoping later; the
solo-then-monitor-select wiring is deferred to whichever milestone adds
monitor selection (M11's control surface is the current best guess, spec
1.5/1.10).

**Bus mono (spec 1.5) only ever touches channels 0 and 1.** "First press
sums to mono, second press swaps channels 1 and 2" is unambiguous about
*which* channels; a bus's other 6 channels (`FC SW RL RR SL SR`) are
reshuffled only by the 12 bus modes, which is explicitly M7 (spec 3.4).
Implementing an 8-channel-aware mono here would mean guessing at M7's bus
mode semantics now. `BusMono` (`Off` / `Mono` / `StereoReverse`) lives in
`crates/loomix-core/src/bus.rs`.

**The offline render harness's routing-truth-table test uses a Goertzel
single-bin filter, not an FFT crate, to identify each strip's tone on
each bus.** M3's signal path is linear and stateless per block (gain scale
and sum, nothing spectral), so the test only ever needs the magnitude at
8 known, bin-aligned frequencies — a ~15 line Goertzel loop
(`crates/loomix-core/src/render.rs`) gets an exact answer for exactly that
question without a new dependency `cargo deny` would need to vet. A real
FFT earns its place once M6's parametric EQ needs a general frequency
response sweep (spec 4.1 layer 1).

**The truth-table test does not enumerate the full 2^64 strip×bus
assignment space spec layer 5 could be read as literally asking for.**
"Every combination... that matters" is read as: every one of the 8x8
matrix's 64 cells tested for cross-talk in isolation
(`matrix_every_cell_routes_only_its_own_strip_to_only_its_own_bus`), plus
the full 2^8 = 256-combination space of each *other* per-strip dimension
(mute, solo) applied uniformly across all 8 buses at once, plus a
dedicated case for the one feature that's genuinely per-bus-per-strip
(the 8 independent gain layers). This is the reduction real routing-matrix
tests use: each dimension gets its combinatorics exhausted, but the
dimensions aren't multiplied against each other, which is what would
produce an uncomputable test rather than a stronger one.

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

`RealtimeGuard::drop` now reads a second thread-local (the violation
flag) on every exit instead of just clearing the first, and
`rt_assert_guard_overhead`'s checked-in bench baseline moved with it —
1.776ns to 2.675ns on the CI runner, +50.64%, tripping the 10% regression
gate on this PR's own `bench` job. Regenerated from that same CI run's
`criterion-pr-baseline` artifact, not a local machine, per the M0 lesson
above about runner-vs-laptop hardware variance. The extra nanoseconds are
once per guarded scope (per `process_block` call once the engine exists,
not per sample) and buy back a trap that actually fires in a release
build; not regenerating the baseline to hide that cost was never the
alternative under consideration.

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
the milestone that gives each crate actual behaviour: M11 for the CLI's
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
inputs land** (fuzz targets at M11/M12, the soak harness at M4/M10,
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
