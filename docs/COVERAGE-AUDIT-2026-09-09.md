# Voicemeeter manual coverage audit — 2026-09-09

Triggered by a real gap found the hard way: the hardware strip pan pot had
been implemented in the engine since M5 but was never wired to the UI,
unnoticed for three milestones. Rather than wait for the next one to
surface by accident, this audit reads all three vendor manuals in full and
cross-checks every control against `docs/SPEC.md` and the actual code.

**Methodology.** The three manuals (Standard 99 pages, Banana 99 pages,
Potato 112 pages — 310 pages total, confirmed via `pdfinfo`) were each
read in exact, non-overlapping 15-page chunks, extraction only, no
classification, into 22 files under `docs/audit/` (`standard-*.md`,
`banana-*.md`, `potato-*.md`), each entry citing its exact source page.
Classification against `docs/SPEC.md` and the codebase happened only
after every chunk file existed, as a separate pass, per direct
instruction — an earlier single-pass attempt at this same audit mixed
reading and classifying in one step and was rejected and redone this way.
Potato's manual is `SPEC.md`'s own baseline; Standard's and Banana's were
read in full for anything they cover in more detail, or that's specific to
the smaller editions, not merely skimmed for deltas.

**State-1 verification.** A control is only marked reachable (state 1) if
it is actually wired end to end today: an `EngineCommand` variant in
`loomix-app::control.rs`, a `#[tauri::command]` in `main.rs`, and a live
control in `ui/src/App.tsx`. Verified directly against the current code,
not assumed from `SPEC.md`'s text. As of this audit, the *entire*
UI-reachable surface is: strip mute, solo, mono, bus assignment, per-bus
gain layer (fader), the strip pan pot (added today), bus mute, bus mono,
bus mode, bus gain, and device selection/connect. Nothing else — no EQ
(strip or bus), no gate/compressor/denoiser, no Intellipan pads, no M.C.,
no Karaoke, no limiter threshold, no bus modes' patch config, no recorder,
no MIDI, no network audio, no macro buttons, no presets. This matches
`App.tsx`'s own footer text ("the EQ graph lands next") and M8's spec'd
scope (spec 3.4 M8 explicitly excludes macro buttons, recorder transport,
Intellipan pads, MIDI mapping, network audio, and preset scenes from this
milestone).

## Summary

| State | Count | Meaning |
|---|---|---|
| 1 — in spec, implemented, UI-reachable | ~14 control groups | faders, mute/solo/mono, bus assign, pan pot, bus mode, bus mute/gain, device select |
| 2 — in spec, implemented in the engine, **not** UI-reachable | 14 control groups | see highlighted list below |
| 3 — in spec, not implemented, milestone already assigned | ~30 control groups | M6/M7/M9/M12/M13/M14/M15 work not yet started or UI-only |
| 4 — not in `SPEC.md` at all | 4 items, added below | Out Limiter, AutoUpMixMode, DMX-512, System Settings Slider Mode |

`SPEC.md` turned out to already be unusually thorough — it was written
directly from the Remote API parameter tables, and this audit's own
extraction of those same tables (all three editions) confirms nearly
everything in them is already present in section 1.15's parameter
namespace list, including detail this audit expected to find missing
(Voice Modeler/Pitch, the extended -36..+18dB strip EQ range, per-app
`AppGain`/`AppMute`). The state-4 list is short because of that, not
because the search was shallow — see "Manual disagreements and
inconsistencies" below for what full-text reading turned up beyond simple
gaps.

## State 2 — implemented, not reachable (read this list first)

Everything below exists in `loomix-core` today (a public, mutable struct
field or a working `process()` call) but has no `EngineCommand`, no
Tauri command, and no UI control:

1. **Strip and bus parametric EQ** (M6, `parametric_eq.rs`) — fully
   implemented and tested; `App.tsx`'s own footer says "the EQ graph
   lands next." `SetStripEqCell`/`SetBusEqCell` commands already exist in
   `control.rs` (proven by their own tests) but nothing calls them from
   the UI.
2. **Hardware strip gate** (`gate.rs`) — macro knob 0..10, band-pass
   sidechain, full detail parameters (spec 1.3).
3. **Hardware strip compressor** (`compressor.rs`) — macro knob, auto
   make-up, full detail parameters.
4. **Hardware strip denoiser** (`denoiser.rs`) — macro knob, Voice
   Modeler (pitch/formant shift) when engaged.
5. **Hardware strip limiter threshold** (`limiter.rs`, `pub threshold_db:
   f32`) — the limiter itself runs (M5); its threshold is a public,
   settable field with no command wired to it. Currently frozen at
   whatever `HardwareChain::new` defaults it to.
6. **Intellipan** (`intellipan.rs`) — all three pad modes (Color,
   Position, Modulation), each a working `process()` call with public
   x/y-equivalent state; explicitly scoped out of this pass per direct
   instruction ("the 2D Intellipan pads... belong to a later UI pass").
7. **Virtual strip 3-band EQ** (`eq3.rs`, `ThreeBandEq`) — bass/mid/treble,
   fully implemented.
8. **Virtual strip 5.1 position pad** (`pan.rs::PositionPad5_1`) — the
   virtual-strip equivalent of the pan pot just wired for hardware
   strips; same Intellipan-family scoping exclusion as #6.
9. **M.C. (mute center)** (`strip_dsp.rs::VirtualChain.mc: bool`) —
   implemented, public field, no command.
10. **Karaoke** (`karaoke.rs`) — all 4 modes (K-m/K-1/K-2/K-v) plus off,
    implemented on the AUX virtual strip per spec 1.4.
11. **Bus EQ on/off toggle and A/B memory** — `EQ.on`/`EQ.AB` exist at
    the `SetBusEqCell`-adjacent level in the engine (M6) but the toggle
    itself (as opposed to editing individual cells) has no UI control.
12. **Strip FX sends and their pre/post buttons** (Reverb/Delay/Fx1/Fx2
    send levels, spec 1.2 step 11) — not yet implemented at all (M9 owns
    this; listed here only because the *pan pot's own neighbours in the
    signal chain* are worth flagging together — see state 3 below for
    the accurate classification; corrected from an initial
    over-inclusion during drafting).
13. **`pack_channels`'s master-device-as-strip-source path**
    (`loomix-app::engine_io.rs`) — not a Voicemeeter feature at all, an
    internal wiring path proven correct by its own test but unreachable
    given `connect_audio` always passes `master_strip: None`. Noted here
    only because it's the exact same *shape* of gap (implemented,
    provably correct, silently unused) as the rest of this list — see
    `docs/ARCHITECTURE.md`'s 2026-09-09 interleaving-fix entry.
14. **Bus SEL's multi-select** (Ctrl+Click selects several buses at
    once, spec 1.5) — the engine has no concept of "currently selected
    buses" beyond the UI's own single `selectedBus` state; this is
    UI-only work once EQ/gate/etc. wiring makes multi-bus editing useful,
    not an engine gap.

(Items 12-14 are noted for completeness but are not the same clean
"implemented, unreachable" shape as 1-11; the real state-2 list a
reader should act on is 1-11.)

## 1.1 Editions and I/O topology

State 1. Implemented (`loomix-core::{NUM_STRIPS, NUM_BUSES}` = 8/8, spec
1.1's fixed Potato topology in `strip::topology_is_hardware`/
`topology_is_aux`) and reachable (the UI renders all 8 strips and 8
buses). `--layout compact|mid|full` (Standard/Banana presets) is not
implemented — state 3, no milestone currently owns a "layout preset"
concept explicitly; folding it into M15 (polish) is the natural home, not
a fresh milestone.

## 1.2 Exact signal flow

Mixed states, item by item (hardware strip order): source (state 1) →
pre-fader tap (state 3, M12/M7) → insert point (state 3, M9/2.3's AUv3
upgrade) → denoiser/Voice Modeler (state 2) → gate (state 2) → compressor
(state 2) → strip EQ (state 2) → Intellipan (state 2) → **pan pot (state
1, fixed today)** → limiter (state 2, engine runs it, threshold not
settable) → FX sends (state 3, M9) → fader/gain layers (state 1) →
mute/solo (state 1) → bus assignment (state 1).

Bus chain: sum (state 1) → FX returns (state 3, M9) → bus mode (state 1)
→ bus EQ (state 2) → mono (state 1) → mute (state 1) → gain (state 1) →
**bus output limiter/peak-remover (state 4 — see below)** → per-bus
output delay (state 3, M13/M4-adjacent, no dedicated owner yet, folding
into M4's clocking work or M15 is reasonable) → device output (state 1).

## 1.3 Hardware input strip: every control

Device selector: state 1 (device picker, though scoped to session
connect/disconnect rather than per-strip live reassignment — a real,
minor gap: `main.rs::connect_audio` wires exactly one input device into
strip 0 for the whole session; there is no per-strip device picker for
strips 1-4 at all yet). Strip label: state 3 (no UI). Intellipan
Color/Position/Modulation: state 2. Comp/Gate/Denoiser knobs and every
listed detail parameter: state 2 (macro knob) / state 3 (the detail-view
escape hatch itself — right-click to edit raw sub-parameters — is a
**deliberate, already-logged Loomix design decision not to reproduce**,
per `docs/DSP.md`'s "Loomix's own documented mapping, not a reproduction
of Voicemeeter's unpublished one"; not a gap). Strip parametric EQ: state
2. Limiter: state 2. Mono/Solo/Mute: state 1. Gain fader (gain layers):
state 1. Reverb/Delay/Fx1/Fx2 sends and their post buttons: state 3 (M9).
Bus assign: state 1. Input meter: state 1 (meters render for strips
today). Standard-edition `Audibility` knob: state 3, correctly scoped —
`SPEC.md` itself says "implement it" but no milestone has yet; natural
home is M5 (already merged) retroactively, or M15 as a small addendum —
flagged, not resolved, per the "doesn't fit an existing milestone
cleanly" instruction.

## 1.4 Virtual input strip: every control

3-band EQ: state 2. 5.1 pan pad: state 2. M.C.: state 2. Karaoke: state 2.
Limiter: state 2. Mono/Solo/Mute/fader/bus-assign: state 1 (shared code
path with hardware strips). Connected application list: state 3 — spec
2.2/2.3 already correctly scope this behind macOS 14.4's process-tap API
with an explicit availability gate; not implemented yet, no explicit
milestone owner (M15 polish is the natural home given its
platform-conditional nature, or a small M8-follow-up — flagged, not
resolved).

## 1.5 Master section: every bus control

Device selector, SEL (already captures Ctrl+Click multi-select in spec
text), bus mode, mono, mute, gain fader: all state 1 or already
correctly captured. EQ toggle: state 2. Reverb/Delay/Fx1/Fx2 return
knobs: state 3 (M9). Monitor select: state 3 (M8/M15-adjacent UI work, no
engine concept of a monitoring bus yet). Output meter: state 1.

## 1.6 The 12 bus modes

State 1 for the engine (all 12 modes implemented and tested, M7) and the
UI (bus mode selector wired). The Mix-Down `RL`-vs-`FR` vendor-manual
typo is already correctly identified and worked around per `SPEC.md`'s
own text — independently reconfirmed in this audit from all three
manuals' own Mix Down A/B formula tables (Standard p.26, Banana pp.26-30,
Potato pp.31/34), verbatim, in every one, not a one-edition
transcription slip. **AutoUpMixMode auto-detection is state 4** — see
below.

## 1.7 Parametric EQ engine

State 2 across the board (M6 fully implemented, zero UI path) — see the
highlighted list.

## 1.8 Internal FX

State 3, M9, not started. `SPEC.md`'s existing text for Reverb, Multitap
Delay, and the C5 multiband compressor already matches this audit's
extraction in detail (preset counts, DRY/WET/DECAY/E.Ref, the 8-tap
timeline mechanics, TAP/Scale-Fit/Fix-Delay/Auto-Fit, the 5-band
compressor's per-band parameters and LINK type) — no additions needed
here, M9's future implementer already has everything the manuals specify.

## 1.9 Recorder / tape deck

State 3, M12, not started. `SPEC.md`'s "one or all inputs" phrasing
already implies the per-source arming this audit found in the Remote
API's `Recorder.ArmStrip(i)`/`ArmBus(i)` (confirmed present in all three
manuals, most explicitly in Banana p.59 and Potato p.71) — not a gap
requiring a wording change, just confirmed accurate.

## 1.10 Main menu, every item

State 3, spread across several milestones (M13 for MIDI/network dialogs,
M15 for the rest) — not implemented, correctly scoped, no changes needed.

## 1.11 System settings dialog, every field

Mostly state 3 (M4/M13/M15, not yet built as a dialog). Two additions
needed:

- **Out Limiter toggle is missing entirely — state 4.** See below.
- **`Option.SliderMode`** (the *main* System Settings dialog's own
  Absolute/Relative fader-linking behavior) **is missing — state 4,
  distinct from the Streamer View app's own separate slider-link mode
  `SPEC.md` 1.17 already documents.** See below.

## 1.12 MIDI mapping

State 3, M13, not started. Fully matches this audit's extraction
(Learn/F/FF/Advanced Feedback/MIDI Forward all already named in
`SPEC.md`'s text) — no additions needed.

## 1.13 Network audio

State 3, M14, not started. Matches extraction exactly, including the
explicit, correct exclusion of VBAN-Frame screen sharing.

## 1.14 Macro buttons application

State 3, M13, not started, with one addition:

- **DMX-512 lighting control is missing entirely — state 4.** See below.

## 1.15 Remote control API and request script

State 3, M13, not started. This section's own parameter namespace list is
the most thoroughly cross-checked part of this audit (every table in all
three manuals was read against it) and is already accurate and complete
— no additions found.

## 1.16 Preset scenes

State 3, M15, not started. Matches extraction exactly (64 slots, F1-F24,
the explicit "not device selection/system settings/MIDI/VBAN" scope
boundary appears identically worded in all three manuals and in
`SPEC.md`).

## 1.17 Bundled companion tools

State 3 across the board (M15 for most, M4's own virtual driver control
panel equivalent for the last item). Matches extraction; Streamer View's
own separate slider-link mode is already correctly captured here,
distinct from the System Settings-level one flagged as state 4 above.

## 1.18 Interaction conventions to reproduce exactly

State 3, cuts across every milestone that adds an interactive control
(most gestures apply once EQ/gate/etc. get a UI, i.e. depend on the
state-2 items above getting wired first). No additions — this audit's
extraction of double-click-reset, right-click detail views, Ctrl+right-click
undo, Shift+click precision slider, etc. matches `SPEC.md`'s table
exactly across all three manuals.

## 1.19 Latency, clocking and known failure modes

State 1/3 mixed (drift correction is implemented per M4, per-device
non-sync warnings and loopback warnings are UI text not yet written).
Matches extraction; the manuals' own explicit naming of "robotic
voice"/audio-cut symptoms from buffer misconfiguration (Standard p.81,
Banana p.80, Potato p.92, each in bold red vendor text) is a useful,
independent confirmation that this project's own M8 distortion
investigation (frames-vs-samples bug, `docs/ARCHITECTURE.md`) found a
real, industry-recognized failure class — not evidence the two specific
bugs are the same defect, just the same symptom family.

## State 4 — not in `SPEC.md` at all

### 1. Out Limiter (bus-level brickwall/peak-remover system toggle)

Confirmed identically in all three manuals: Standard p.79-80, Banana
p.77-78, Potato p.89-90 — a single System Settings on/off toggle,
enabled by default, switching *every* output bus between a brickwall
limiter ("VB-Audio C-Limiter") and a simple peak remover when off.
`SPEC.md`'s bus signal chain (1.2) has no limiter step for buses at all
(only hardware strips get one, 1.2 step 10); 1.5's bus control table and
1.11's system settings field list are both missing it too. Does not
cleanly fit an existing milestone's own description — M5 built the
*strip* limiter this parallels, M7 is the last bus-focused milestone
already merged; flagged for the user's judgment on milestone number
rather than assigned unilaterally, added to `SPEC.md` tagged **M8** as
the nearest currently-open, bus-signal-chain-adjacent milestone.

### 2. AutoUpMixMode (auto stereo/multichannel detection for Up Mix modes)

Confirmed identically in all three manuals' registry-parameters
appendix: Standard p.99, Banana p.99, Potato p.112 — exact wording and
threshold in all three: if enabled (off by default), an Up Mix bus mode
skips its own transform when material above **-80 dBFS is detected on
channels 3, 4, or 5**, i.e. the incoming signal is already multichannel
rather than genuinely stereo. Not a UI-exposed mixer control in the
reference product either (registry-only) — added to `SPEC.md` section
1.6 tagged **M7** (the milestone that already built Up Mix itself; a
completeness addition to already-shipped work, not a new milestone).

### 3. DMX-512 lighting control

Confirmed identically in all three manuals' Macro Buttons section:
Standard p.76, Banana p.74, Potato p.86 — `System.DMXSetValue(addr,
channel, value...)` and `System.DMXCommit()`, driven through a DMX
serial (COM) interface selected in a dedicated "DMX Configuration..."
dialog in the Macro Buttons app's own system menu (also confirmed present
in that menu's exact item list in all three manuals). Niche and
hardware-dependent (needs a USB DMX interface, e.g. the vendor's tested
"Enttec Open DMX USB") but a genuine reference-product capability, not
one of `SPEC.md`'s deliberate exclusions. Added to section 1.14 tagged
**M13** (the milestone that already owns macro buttons' system actions).

### 4. System Settings' own Slider Mode (Absolute/Relative fader linking)

Confirmed in Potato p.89-90 (Banana and Standard don't have per-bus
sub-mixing to make this meaningful, consistent with `SPEC.md` 1.1's own
observation that they're layout presets over the same 8-bus engine): a
main System Settings dialog toggle, Absolute (default, every one of a
strip's per-bus gain-layer values jumps together when one is dragged) or
Relative (preserves their existing offsets). Distinct from Streamer
View's own, separately-configured slider-link mode, which `SPEC.md` 1.17
already documents correctly. Added to section 1.11 tagged **M8** (the
milestone that already built the gain-layer fader UI this toggle would
govern).

## Manual disagreements and inconsistencies found

None of the three manuals contradict each other on any control's actual
behavior, range, or default. What was found instead, reading in full
rather than sampling:

- **The manuals' own internal inconsistencies** (not manual-vs-manual):
  Potato p.90 says Swift mode "has been disabled because generating too
  much support," while Potato p.92 says it's "not recommended because
  might be unstable" — the same feature described two different ways
  two pages apart in the *same* manual. Also Potato's own Specifications
  table (p.97) lists a narrower MIDI-remoting scope than its own MIDI
  Mapping dialog screenshot two pages earlier (p.94) actually shows
  (missing Pan, Karaoke, and Compression from the summary table) — the
  same pattern appears in Standard's manual (p.85 vs p.82) and Banana's.
  In every case the detailed dialog page is the more reliable source,
  the specifications-table recap is not.
- **A real vendor-table duplication**: Potato p.68's Remote API table
  lists `Strip[i].Denoiser.Threshold` twice under the identical name —
  transcribed as printed, not corrected, since it doesn't imply a second
  distinct parameter.
- **A vendor naming reuse, not a bug**: the 4 VBAN-TEXT output stream
  slots and the 2 VBAN-MIDI output stream slots both use the literal
  names `"vban1"`/`"vban2"` for two different purposes (Potato pp.83-85)
  — disambiguated only by which script function targets them
  (`SendMidi` vs `SendText`/`BEGIN_SECTION`), not by the string itself.
  Noted for anyone implementing M13/M14 against these names later, not a
  `SPEC.md` change.
- **One elaboration, not a conflict**: Standard's manual gives a
  concrete, worked Composite-mode channel layout for its own 3-strip
  topology (pp.26, 42-43); Potato's manual only describes the general
  mechanism. Both describe the same mechanism at different levels of
  concreteness, matching `SPEC.md` 1.6's own Composite description
  already.
- **`Pan_x`/`Pan_y` API resolution**: initially flagged in this audit's
  own extraction as ambiguous — un-scoped to "Physical Strip Only"
  unlike `Color_x`/`Color_y`, raising the question of whether a hidden
  generic pan parameter existed on virtual strips too. Resolved by
  Banana's own Remote API table (p.55-56): `Pan_y`'s remark reads
  "-0.5 to +0.5 for 5.1 pan pot," confirming `Pan_x`/`Pan_y` is a shared
  API surface addressing the hardware pan pot on hardware strips and the
  5.1 position pad's X/Y coordinate on virtual strips — exactly
  `SPEC.md` 1.2 step 9's own framing ("stereo pan on hardware strips, 5.1
  position pad on virtual strips"), not a hidden extra control. The
  "L/R PAN Virtual Strip #3/#6/#8" rows seen in every manual's MIDI
  Mapping dialog screenshot are this same pad's axis, MIDI-mappable like
  any other parameter, not evidence of a second pan control.

## Files

- 22 extraction chunk files under `docs/audit/{standard,banana,potato}-*.md`
  (kept — they're the citable source for every page reference above).
- `docs/audit/_manuals/*.pdf` — working copies of the three manuals,
  needed because this session's sandbox could not read the originals at
  `~/Documents/loomix-refs/` directly; git-ignored, not part of the
  deliverable.
- This report.
- `docs/SPEC.md` — 4 additions (sections 1.2/1.5/1.11 for Out Limiter,
  1.6 for AutoUpMixMode, 1.14 for DMX-512, 1.11 for Slider Mode).
- `docs/ARCHITECTURE.md` — a new dated entry summarizing this audit.
