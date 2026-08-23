# DSP reference

Every filter's transfer function/algorithm and where its reference test
lives (spec 3.2, 4.4). Starts at M5 (spec 3.4) — the first milestone with
real filters; M0-M4 had no signal processing beyond gain and routing.

Sample rate in every formula below is the engine's current rate
(`Engine::sample_rate`, default 48kHz, spec 1.11), not a fixed constant.

## Shared biquad primitive (`crates/loomix-core/src/biquad.rs`)

One RBJ/Audio EQ Cookbook biquad (direct form I), reused by the gate
sidechain, the virtual-strip 3-band EQ, Color pad tonal shaping, and
Karaoke. `Biquad`'s `None`/bypassed state is a guaranteed bit-exact
passthrough — a 0dB peaking filter's coefficients are *not* literally
`b1=b2=a1=a2=0` (they're a pole-zero cancellation, mathematically unity
gain but not bit-exact over real floating-point state), which every block
built on this primitive works around by bypassing outright at its own
neutral setting rather than trusting the math.

- `BiquadCoeffs::peaking(fs, f0, Q, gain_db)` — standard RBJ peaking EQ.
- `BiquadCoeffs::band_pass(fs, f0, Q)` — constant 0dB-peak-gain BPF.
- `BiquadCoeffs::notch_from_bandwidth(fs, f0, bandwidth_octaves)` — notch,
  alpha derived from bandwidth rather than Q directly.
- `q_from_bandwidth_octaves(bw)` — `1 / (2 * sinh(ln(2)/2 * bw))`, the
  Cookbook's octave-to-Q relation.

Reference test: `biquad::tests::known_answer_matches_the_cookbook_formula`
checks literal coefficient values (fs=48000, f0=1000, Q=1, gain=+6dB)
computed independently in Python against the same formula, tolerance
`1e-6` (spec 4.1 layer 1). Frequency-response tests use a Goertzel
single-bin magnitude probe (`biquad::test_support`, same technique as
`render.rs`'s routing-truth-table tone probe) rather than a full FFT.

## Macro-knob curve (`crates/loomix-core/src/knob_curve.rs`)

Gate, Compressor and Denoiser each expose a spec 1.3 "0..10 rotary, macro
over the full detail parameters" knob. **Voicemeeter's own internal curve
is not published anywhere in the reference manual this project verifies
against — the tables below are Loomix's own curve, not a reproduction of
theirs.** `knob <= 0.0` always means fully bypassed (a true bit-exact
neutral, spec 4.1). Above that, `fraction(knob) = knob / 10` interpolates
linearly between each parameter's "just engaged" and "maximum" value —
tested for monotonicity and the two endpoints
(`knob_curve::tests`), not for matching an external reference, since none
exists.

### Gate (`gate.rs`)

| knob | threshold (dB) | damping max (dB) | attack (ms) | hold (ms) | release (ms) |
|---|---|---|---|---|---|
| 0 | *bypassed* | *bypassed* | *bypassed* | *bypassed* | *bypassed* |
| 2 | -50 | -20 | 41 | 410 | 256 |
| 4 | -40 | -30 | 32 | 320 | 212 |
| 6 | -30 | -40 | 23 | 230 | 168 |
| 7 | -25 | -45 | 18.5 | 185 | 146 |
| 8 | -20 | -50 | 14 | 140 | 124 |
| 10 | -10 | OFF (unlimited) | 5 | 50 | 80 |

The band-pass sidechain center frequency (spec 1.3: 100-4000Hz, "1.5
octave band pass on the detector") is a separate detail control
(`Gate::set_bp_sidechain_hz`), not driven by the knob — default 1000Hz.
The detector itself RMS-smooths (5ms, fixed) ahead of the threshold
comparison; without that stage a fast attack chases the input waveform's
own ripple instead of its level (found by
`gate::tests::sidechain_filter_ignores_energy_far_outside_the_band`
initially failing when both an in-band and out-of-band tone opened the
gate identically).

### Compressor (`compressor.rs`)

Soft-knee characteristic curve (Giannoulis/Massberg/Reiss-style), knee
width in dB = `knee * 24`. Auto-makeup (default on) approximates
`-(threshold_db * (1/ratio - 1)) / 2` — a common heuristic, not a claim
about matching any specific commercial compressor's makeup gain.

| knob | threshold (dB) | ratio | attack (ms) | release (ms) | knee (0=hard..1=soft) |
|---|---|---|---|---|---|
| 0 | *bypassed* | *bypassed* | *bypassed* | *bypassed* | *bypassed* |
| 2 | -10.4 | 2.4:1 | 41 | 336 | 0.8 |
| 4 | -17.8 | 3.8:1 | 32 | 272 | 0.6 |
| 6 | -25.2 | 5.2:1 | 23 | 208 | 0.4 |
| 7 | -28.9 | 5.9:1 | 18.5 | 176 | 0.3 |
| 8 | -32.6 | 6.6:1 | 14 | 144 | 0.2 |
| 10 | -40 | 8:1 | 5 | 80 | 0 |

Input/output gain (spec 1.3: -24..+24dB each) are independent detail
fields, not driven by the knob. The detector RMS-smooths (5ms fixed)
ahead of attack/release for the same reason as the gate's — an early
version fed the attack/release follower a raw rectified sample directly,
which ratchets toward the signal's peak instead of its RMS level on any
audio-rate waveform (`compressor::tests::a_steady_tone_above_threshold_settles_to_the_static_curve`
caught this: measured reduction was ~2.7dB off from the static-curve
prediction before the fix).

### Denoiser (`denoiser.rs`)

Not spectral/multi-band noise reduction — a single-band downward expander
whose threshold tracks the strip's own ambient floor (fast to fall when
the signal goes quiet, slow to rise so a loud passage doesn't get
mistaken for a new floor). `ponytail: single-band expander, upgrade to
spectral subtraction if a real noisy-mic signal shows artefacts this
can't fix.`

| knob | expansion ratio |
|---|---|
| 0 | *bypassed* |
| 2 | 2:1 |
| 4 | 3:1 |
| 6 | 4:1 |
| 7 | 4.5:1 |
| 8 | 5:1 |
| 10 | 6:1 |

Floor tracker: fall time constant 50ms, rise time constant 1000ms
(20x slower — deliberately asymmetric so a sustained loud passage takes
noticeably longer to be relearned as "normal" than a genuine quiet gap
takes to be recognised as noise), margin 6dB above the tracked floor.
Initial floor is -20dB, not silence — starting from an assumed-silent
floor makes the very first sound of *any* kind (quiet or loud) look
identical to the tracker, since both would be "louder than floor" and
take the slow rise path; starting from a moderate assumption means real
quiet noise (below it) is learned via the fast fall path immediately.

## Limiter (`limiter.rs`)

Sample-accurate brickwall, no lookahead, no attack/release — spec 1.3's
Limiter row has no parameters beyond the threshold itself (-40..+12dB,
default +12). Linked across every channel in the frame (the loudest one
sets the gain, applied to all), so limiting never shifts a multichannel
signal's image. `ponytail: no lookahead, so a hard-clamped transient can
sound abrupt; upgrade to a short lookahead buffer if that's audible.`

## Pan laws (`pan.rs`)

**Balance law, not constant-power.** spec 4.1 also requires a true
bit-exact neutral setting for every effect; a constant-power law (the
usual sine/cosine taper) can't give both, since it's constant-power
*because* both channels sit at -3dB at center — trading a neutral center
for constant total power at the extremes. Direct instruction was to keep
center exactly unity, so both the hardware pan pot and the virtual 5.1
pad use a balance law instead: the favoured channel stays at exactly 1.0
across the whole sweep, only the other channel is attenuated.

`StereoBalance::gains(pan)`, `pan` in `-1.0..=1.0`:

```
pan <= 0:  (left, right) = (1.0, 1.0 + pan)
pan >  0:  (left, right) = (1.0 - pan, 1.0)
```

`PositionPad5_1` (virtual strips, spec 1.4's 5.1 pad, `x: -0.5..0.5`,
`y: 0..1`) reuses `StereoBalance` for the left/right split and crossfades
front/rear by `y`; `(0, 0)` leaves the frame untouched (front unity, rear
silent, center channel untouched, not synthesised). `ponytail: reuses the
balance law rather than a full VBAP amplitude panner across all 5
anchors, and never synthesises center-channel content — upgrade to real
VBAP if the pad needs to hit its labelled FC anchor distinctly.`

Property tests (`pan::tests`) check the balance law's actual invariant —
the favoured channel never leaves unity, the sweep is monotonic and
continuous — not spec 4.1 layer 2's literal constant-power claim, which
doesn't hold for a balance law by construction.

## Intellipan (`intellipan.rs`, hardware strips only)

Three mutually exclusive pad modes (spec 1.3), one active at a time,
modelled as separate structs behind an enum so switching modes can never
leak state between them.

**Reverb/room-effect scope, decided explicitly:** spec 1.3 describes Color
as "3 band tonal shaping plus a small reverb on the upper half" and
Position as "binaural placement with a small room effect". Building a
strip-local reverb ahead of M8's real send/return reverb engine would mean
two implementations and a migration — skipped for Color on direct
instruction, and the same reasoning applies identically to Position's room
effect, so both are skipped and logged here. **Both pads' `y` axis is
accepted and stored but does not affect audio this milestone** — `y==0`
neutrality does not exercise the reverb/room path, because there is no
reverb/room path yet. Both land when M8 exists.

- **Color**: a bass/treble tilt driven by `x` alone (spec's "3 band"
  wording, but only one control axis is actually available once `y` is
  reserved for the deferred reverb) — two peaking bands at 200Hz/5000Hz,
  gain `±12dB * (x / 0.5)`, opposite sign each side. `x == 0` bypasses
  both filters outright (the guaranteed-bit-exact pattern from the
  biquad primitive above).
- **Position**: binaural placement via interaural level difference
  (reusing `StereoBalance` again) plus interaural time difference — a
  fixed-size delay line per channel, max 1ms (not a physiologically
  literal ITD claim, just a placement control), `x == 0` uses zero delay
  exactly rather than a near-zero fractional read.
- **Modulation**: one modulated (chorus-style) delay line stands in for
  spec's "chorus, phasing, feedback modulation" as a single 2D-pad effect.
  `fx_x < 0` (left half, per spec) enables feedback up to 0.5; `fx_y` is
  depth, `0` bypasses entirely. LFO rate 0.8Hz, base delay 15ms, max depth
  8ms — all fixed, not spec parameters. `ponytail: one algorithm covers
  three named characters; a genuinely separate allpass-cascade phaser is
  the upgrade path if the pad's range reads as too similar throughout.`

`Position`/`Modulation` are `Box`ed inside the `Intellipan` enum: their
delay buffers (2048 and 8192 `f32` samples respectively, sized for high
sample rates) would otherwise set the enum's size to its largest variant
regardless of which mode is active. This wasn't a style choice — building
all 8 strips unboxed overflowed the test thread's stack before the fix.

## 3-band EQ (`eq3.rs`, virtual strips only)

Three peaking (not true shelving) bands, spec 1.4's -12..+12dB range, at
fixed centers: bass 200Hz, mid 1000Hz, treble 5000Hz (Loomix's own choice
— spec gives the gain range only). Peaking rather than shelving reuses the
same biquad type M5's other blocks already need, instead of adding shelf
coefficient formulas with no other user yet — M6's full parametric EQ
engine is where shelving types (spec 1.7's 7 filter types) earn their
place. Each band bypasses outright at `0.0`, all three flat is a
guaranteed bit-exact null.

## Karaoke (`karaoke.rs`, virtual AUX strip only)

- **K-m**: exact phase-cancellation, `(L-R)/2` to both channels — spec's
  own wording ("removes the common mono content") names the algorithm
  directly, no interpretation needed.
- **K-1 / K-2 / K-v**: genuinely ambiguous in spec 1.4 ("K-1 keeps some
  bass and treble, K-2 keeps more of both... K-v filters the 200 to
  4000Hz vocal band" — no formula, no depth numbers given). Read as: all
  three apply the same wide notch (center `sqrt(200*4000) ≈ 894.4Hz`,
  bandwidth `log2(4000/200) ≈ 4.32` octaves) mixed with the dry signal at
  increasing depth:

  | mode | dry/notch mix depth |
  |---|---|
  | K-2 | 0.5 (lightest, keeps the most of the original) |
  | K-1 | 0.85 |
  | K-v | 1.0 (full depth — the dedicated vocal-band filter spec names outright) |

  This is Loomix's own interpretation, not Voicemeeter's — there is no
  published formula to reproduce.

## Per-strip chain order (`strip_dsp.rs`)

Hardware strips follow spec 1.2's explicit order: denoiser → gate →
compressor → strip parametric EQ (M6, below) → Intellipan pad → pan pot →
limiter (the FX-send step is M8, not yet in scope). This is a direct
reading of spec text, not an inferred placement.

Virtual strips' order is **not** given by spec — spec 1.2 only orders the
hardware chain; spec 1.4's virtual-strip control list (EQ, pan pad, M.C.,
Karaoke, limiter) is unordered. Implemented order: 3-band EQ → M.C. →
Karaoke (AUX only) → 5.1 position pad → limiter. M.C./Karaoke sit before
the pad because they're channel-content operations (muting a channel,
removing vocal content) that read most sensibly as acting on the strip's
original multichannel material before the pad repositions it — a
judgement call, not a spec requirement, logged with its date in
`docs/ARCHITECTURE.md` and checked by an order-*proving* test
(`strip_dsp::tests::virtual_chain_eq_runs_before_the_pan_pad_provably`):
it drives both the declared order and the alternative order through the
same public block APIs independently, and asserts the real `VirtualChain`
matches only the declared one — not an assertion that assumes its own
implementation's order is correct.

## Parametric EQ (`parametric_eq.rs`, M6)

The shared 6-cell engine spec 1.7 calls for, generic over channel count:
`ParametricEq<2>` for the hardware strip EQ (stereo, inserted per spec
1.2's order above), `ParametricEq<8>` for the bus EQ (independent per
channel, spec 1.2 step 4 — after summing/FX-returns, before the mono
transform, `engine.rs`'s bus loop). One implementation serves both, per
spec 1.7's own text.

**Cell types → `biquad.rs` constructors.** Spec 1.7's 7 types, index 0..6:
Peak (`BiquadCoeffs::peaking`, already existed for M5), LowPass, HighPass,
LowShelf, HighShelf, BandPass (already existed), Notch. The four new
constructors are standard RBJ/Cookbook forms, known-answer tested the same
way as the existing ones (independently in Python, `1e-6` tolerance).
Shelving cells use `alpha = sin(w0)/(2Q)` — the same alpha every other
cell type in this file uses — rather than RBJ's canonical `S`-parameterised
shelf form, since spec 1.7 gives one shared `q` range (1..100) across all
7 types and there is no spec-given `Q`-to-`S` conversion; Loomix's own
choice where the spec underspecifies, same category as Karaoke's/the
macro-knob curves' entries above.

**Coefficient changes are smoothed, not instantaneous** (`biquad.rs`,
`Biquad::set_coeffs`). M6's cells are the first callers that sweep a
biquad's parameters live — a user dragging frequency/gain/Q, or switching
a cell's type while it's engaged. Applying new coefficients to a filter's
existing `(x1,x2,y1,y2)` state in one step computes that sample as if the
filter's entire input history had always run through the new
coefficients, a real, audible discontinuity — worst on a cell-type switch,
where the whole transfer function changes at once, not just one
parameter. `Biquad::set_coeffs` ramps linearly from the current
coefficient set to the new one over a fixed 64-sample window (applied
inside `process()`, so no caller needs to know a ramp is in flight) rather
than swapping in one step. This only smooths *engaged→engaged* changes:
entering/leaving bypass stays instantaneous, matching every block's
established true-neutral convention (spec 4.1) and the real UI gesture
(a cell's on/off toggle is a discrete click, not something dragged).
`ponytail:` the ramp interpolates raw `b0/b1/b2/a1/a2` linearly, not a
provably-stable parameter domain (pole radius/angle, or RBJ's `S`); fine
for two nearby, well-formed coefficient sets (a knob sweep, a switch
between this cell's own 7 types), not proven for arbitrarily divergent
endpoints — upgrade path is interpolating in a parameter domain instead,
if an extreme jump is ever found to click or destabilise mid-ramp.
Proven, not just asserted, in two ways: an "instantaneous swap would click,
the smoothed one doesn't" A/B test on identical accumulated filter state
(`biquad::tests::coefficient_sweeps_do_not_click_...`, same technique
`drift.rs` uses — a wrong implementation actually fails the same
scenario), and a fixed-cell-count random continuous sweep across all 7
types including hard type switches, bounding the smoothed output's
sample-to-sample delta against the naive one's.

**A/B memory doubles params, not live DSP state.** `ParametricEq<N>`
stores two full `[EqChannelParams; N]` (spec 1.7's A/B memories) but only
one live `[EqChannel; N]` (the actual biquads/trim/delay-line state),
rebuilt from whichever memory is active whenever a param changes or the
active memory switches. Cheaper than two full sets of live filter/delay
state, and the same "recompute from params on every change" shape every
other block in this crate already uses (`ThreeBandEq::recompute`, the
macro-knob blocks) — not a new pattern invented for this file.

**Per-channel delay (`DelayLine`, 0..500ms per spec 1.7) is sized to the
current sample rate, not always pre-sized for the 192kHz worst case.**
Reallocated only in `set_sample_rate`, matching the "reallocate outside
`process()`, never inside it" rule every sample-rate-dependent block in
this crate already follows (spec 3.3). `delay_ms == 0.0` is a true
bypass — the buffer isn't touched at all, the same guaranteed-bit-exact
pattern every other neutral setting in this crate uses, not "reading a
zero-sample delay" through the general code path.

**`DelayLine::set_sample_rate`'s concurrency contract, and what a
`debug_assert!` there does and does not prove:** it must never run while
`process()` could be in flight for the same instance — it reallocates,
which is a spec 3.3 violation on the audio thread regardless, and also
corrupts whatever's currently buffered. A same-thread reentrant violation
of that (a `set_sample_rate` call nested inside `process()` on the one
thread that owns the instance) is real-time-unsafe and audio-corrupting
but *not* a memory-safety violation — Rust's own `&mut self` aliasing
rules already rule out true concurrent mutation of one instance in safe
code. A cross-thread violation — reachable only if a caller uses `unsafe`
to alias the instance across threads, exactly the shape `loomix-hal`'s
CoreAudio FFI trampolines take reaching into `Engine` — is a genuine data
race on the buffer's backing allocation: Undefined Behavior, not merely a
wrong sample. The type's own doc comment states this distinction
explicitly, because it changes how carefully a caller has to treat the
contract. A `debug_assert!` catches the same-thread case, compiled out in
release builds — proven to actually fire, in a debug/test build, by
`tests::set_sample_rate_panics_if_called_while_process_is_in_flight`
(the same technique `rt_assert.rs` uses: set the private flag directly,
simulating the reentrant call, then `catch_unwind`), with a companion
test proving that assertion isn't just unconditionally panicking. It
verifies the contract in tests; it does not enforce it in a shipped
release binary — the real protection against the cross-thread case is
architectural (spec 3.3's SPSC/triple-buffer parameter crossing, never a
directly shared `&mut`), not this flag.

**Bus EQ runs before the mono/stereo-reverse transform (spec 1.2 step 4
before step 5), proven with a genuinely non-commutative case, not just
"EQ changed the output."** A per-channel EQ setting and a channel *swap*
only disagree about which channel ends up affected if the two operations
don't commute, so
`engine::tests::bus_eq_runs_before_stereo_reverse_provably` boosts one
channel, sets `StereoReverse`, and checks which channel the boost lands
in against two independently-built hypotheses — the same drives-both-
hypotheses technique `virtual_chain_eq_runs_before_the_pan_pad_provably`
(M5) uses. Checked further, on request, the same way the M1 ring-buffer
test and the M5 gate-chatter test were: the real order was temporarily
inverted in `engine.rs`, the test was confirmed to fail against that
inverted order, then reverted — so the test is confirmed to actually
distinguish the two orders, not just pass regardless of which one ships.
The equivalent strip-side check
(`strip_dsp::tests::hardware_chain_compressor_does_not_see_the_eq_boosted_signal`)
was validated the same way.

**Cross-language verification for the UI's EQ graph.** Per direct
instruction: the graph's TypeScript math (`ui/src/eqResponse.ts`) is a
second, independent implementation of the same Cookbook formulas, checked
against a fixture generated *from* the real Rust engine
(`crates/loomix-core/tests/eq_response_fixture.rs` →
`testdata/fixtures/eq_response_reference.json`, regenerated deliberately,
reviewed in the diff — the same convention as `testdata/golden/`), not
against numbers copied by hand and not trusted as ground truth by either
side without the other agreeing. Getting the two sides to actually agree
at a tight tolerance surfaced two real bugs in the fixture generator, not
in the EQ math itself: probe frequencies that weren't exact bins of the
analysis window leaked energy across the spectrum in a way that a steep
filter (a notch's own center, a stopband) amplified into >1dB of spurious
error — fixed by snapping every probe frequency to `k * sample_rate /
NUM_SAMPLES`; and `render::goertzel_magnitude`'s `f32` accumulation over
a long (32768-sample) buffer wasn't precise enough at sub-0.1dB
tolerance in deep attenuation — fixed with a local `f64`-accumulating
Goertzel in the fixture generator only, the same class of fix as the M4
drift-simulation counters (`docs/ARCHITECTURE.md`). The TS side's own
formulas are otherwise a direct line-for-line port of `biquad.rs`'s.
