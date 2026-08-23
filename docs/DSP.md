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
compressor → Intellipan pad → pan pot → limiter (strip EQ and FX-send
steps aren't in scope until M6/M8). This is a direct reading of spec
text, not an inferred placement.

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
