# LOOMIX — a Voicemeeter class virtual audio mixer for macOS

> **Scope of this document**
> This is the engineering specification and single source of truth for the project. Part 1 is the feature inventory that defines parity. Part 2 records the platform decisions and the reasoning behind them. Part 3 defines the architecture and the milestone order. Part 4 defines the test strategy and the CI pipeline. Nothing ships that is not covered here, and anything that changes in the product changes here first.
>
> * **Part 1** Reference mixer feature inventory, control by control.
> * **Part 2** macOS platform mapping: what translates directly, what is re-engineered, what is out of scope.
> * **Part 3** Architecture, crate layout, milestones, acceptance criteria.
> * **Part 4** Test strategy and CI/CD, which are part of the definition of done.

**Project codename:** `loomix`. Do not use the name Voicemeeter, VB-Audio, VBAN, Banana or Potato in the product name, bundle identifier, UI strings or marketing copy. Voicemeeter is a VB-Audio trademark. Cloning behaviour is fine, borrowing identity is not. The one exception is protocol interoperability, where the wire protocol name may appear inside technical documentation and code comments only.

---

# PART 1 — COMPLETE VOICEMEETER FEATURE INVENTORY

Verified against the official VB-Audio user manuals for Voicemeeter Standard, Banana and Potato (Potato manual version 3.1.2.2, December 2025). Every parameter range below comes from the published Remote API parameter tables.

## 1.1 Editions and I/O topology

| Edition | Hardware input strips | Virtual input strips | Total strips | Physical buses | Virtual buses | Total buses | Channels per bus |
|---|---|---|---|---|---|---|---|
| Standard (Voicemeeter 1) | 2 | 1 | 3 | 1 (A1) | 1 (B1) | 2 | 2 |
| Banana (Voicemeeter 2) | 3 | 2 | 5 | 3 (A1 A2 A3) | 2 (B1 B2) | 5 | 8 |
| Potato (Voicemeeter 8) | 5 | 3 | 8 | 5 (A1..A5) | 3 (B1 B2 B3) | 8 | 8 |

**Loomix targets Potato level parity: an 8 in / 8 out matrix with 8 channel buses.** Standard and Banana equivalents are just layout presets over the same engine, expose them as `--layout compact|mid|full`.

**Virtual device wiring in Voicemeeter Potato**

| Virtual playback device (OS sees an output) | Feeds | Virtual capture device (OS sees an input) | Fed from |
|---|---|---|---|
| Voicemeeter Input | Virtual strip 1 (strip index 5) | Voicemeeter Output | BUS B1 |
| Voicemeeter Aux Input | Virtual strip 2 (strip index 6) | Voicemeeter Aux Output | BUS B2 |
| Voicemeeter VAIO3 Input | Virtual strip 3 (strip index 7) | Voicemeeter VAIO3 Output | BUS B3 |

The newer driver exposes 8 virtual I/O pairs. Pairs 1 to 5 are the licensed "VAIO Extension", which route into hardware strips 1 to 5 and out of buses A1 to A5. Loomix ships all 8 pairs with no licensing gate.

**Loomix virtual device naming** (must be stable, users select these in other apps):

* Inputs to the mixer: `Loomix In 1` … `Loomix In 8`
* Outputs from the mixer: `Loomix Out A1` … `Loomix Out A5`, `Loomix Out B1` … `Loomix Out B3`

## 1.2 Exact signal flow

This ordering is the most important part of the whole document. Get it wrong and nothing sounds right.

### Per input strip (hardware strip, in order)

1. Source acquisition: hardware capture device, or network stream, or virtual device feed. If no device is selected the strip can still receive network audio.
2. **Pre-fader tap** is taken here. This tap feeds: the recorder in "pre-fader inputs" mode, and the composite bus patch when the patch is in PRE mode.
3. **Insert point** (pre-FX or post-FX, selectable globally, see Patch Insert).
4. **Denoiser** / noise floor remover, and when enabled the **Voice Modeler** (pitch and formant shifter).
5. **Gate** (with band pass sidechain).
6. **Compressor** (with optional auto make up).
7. **Strip parametric EQ**, 6 cells, stereo, hardware strips only.
8. **Intellipan panel**, which is one of three mutually exclusive right click modes: Color (3 band tonal shaping plus a small reverb on the upper half), Position (binaural placement with a small room effect, stereo only), Modulation (chorus, phasing, feedback modulation).
9. **Pan pot** (stereo pan on hardware strips, 5.1 position pad on virtual strips).
10. **Limiter** (brickwall, threshold set by clicking the meter bar).
11. **FX sends**: reverb send, delay send, external FX 1 send, external FX 2 send. Each send has its own PRE/POST fader button. In POST the send follows the fader and the mute.
12. **Fader gain**, which in Potato is actually 8 independent gain layers, one per bus. The SEL buttons on the master section choose which layer the strip faders display and edit.
13. **Mute** and **Solo**.
14. **Bus assignment matrix**: 8 toggle buttons A1 A2 A3 A4 A5 B1 B2 B3.

### Per virtual input strip

Same chain, except: no Comp / Gate / Denoiser knobs, no strip parametric EQ, no Intellipan. Instead they get a simple 3 band EQ (bass, mid, treble), a 5.1 position pad, the M.C. button, the Karaoke button (on the AUX strip), the limiter, and the connected application list.

### Per output bus (in order)

1. Sum of all strips assigned to this bus, using each strip's gain layer for this bus.
2. Plus the FX returns: reverb return, delay return, external FX 1 and 2 returns.
3. **Bus mode** transform (12 modes, section 1.6).
4. **Bus parametric EQ**: 6 cells per channel, for all 8 channels independently, plus per channel trim and per channel delay.
5. **Mono button**: first press sums to mono, second press swaps channels 1 and 2 (stereo reverse), third press returns to off.
6. **Mute**.
7. **Bus gain fader**.
8. **Global per bus output delay** from system settings, 0 to 500 ms.
9. Output to hardware device or virtual capture device.

## 1.3 Hardware input strip: every control

| Control | Type | Range | Default | Notes |
|---|---|---|---|---|
| Device selector | Click on strip header | List of system capture devices grouped by API | none | Clearing it leaves the strip fed only by network audio |
| Strip label | Right click on the slider base | String | device name | Empty label hides the strip in the simplified remote view |
| Intellipan / Color pad | 2D pad, right click cycles mode | `Color_x` -0.5..+0.5, `Color_y` 0..1.0 | 0, 0 | Color mode |
| Position pad | same pad, mode 2 | `Pan_x` -0.5..+0.5, `Pan_y` 0..1.0 | 0, 0 | Binaural, stereo only |
| Modulation pad | same pad, mode 3 | `fx_x` -0.5..+0.5, `fx_y` 0..1.0 | 0, 0 | Chorus / phaser / modulation, feedback on the left half |
| Comp knob | Rotary, right click opens detail view | 0..10 | 0 | Simplified macro over the full compressor |
| Comp: input gain | detail | -24..+24 dB | 0 | |
| Comp: ratio | detail | 1..8 | 1 | |
| Comp: threshold | detail | -40..-3 dB | -3 | |
| Comp: attack | detail | 0..200 ms | | |
| Comp: release | detail | 0..5000 ms | | |
| Comp: knee | detail | 0..1 | | Soft to hard transition |
| Comp: output gain | detail | -24..+24 dB | 0 | |
| Comp: auto make up | detail toggle | on / off | on | |
| Gate knob | Rotary, right click opens detail view | 0..10 | 0 | |
| Gate: threshold | detail | -60..-10 dB | | |
| Gate: damping max | detail | -60..-10 dB, plus OFF meaning minus infinity | OFF | Limits gain reduction when closed |
| Gate: BP sidechain | detail | 100..4000 Hz | | 1.5 octave band pass on the detector |
| Gate: attack | detail | 0..1000 ms | | |
| Gate: hold | detail | 0..5000 ms | | |
| Gate: release | detail | 0..5000 ms | | |
| Denoiser knob | Rotary, right click opens detail view | 0..10 | 0 | Non zero adds latency to that strip |
| Denoiser: threshold | detail | 0..10 | | Noise floor remover amount |
| Denoiser: bypass | detail toggle | on / off | off | |
| Voice modeler: enable | detail toggle | on / off | off | |
| Voice modeler: dry/wet | detail | -100..+100 % | | -100 is fully dry |
| Voice modeler: pitch | detail | -12..+12 semitones | 0 | |
| Voice modeler: low formant | detail | -12..+12 semitones | 0 | |
| Voice modeler: med formant | detail | -12..+12 semitones | 0 | |
| Voice modeler: high formant | detail | -12..+12 semitones | 0 | |
| Voice modeler: formant group | detail | -12..+12 | 0 | Moves all three formants together |
| Voice modeler presets | 8 user slots | recall on left click, context menu on right click | | |
| Strip parametric EQ | Button, right click opens dialog | 6 cells x 2 channels | off | Peak gain range extended to -36..+18 dB, cell gain in the API is -12..+12 |
| EQ A/B memory | toggle | A or B | A | |
| Limiter | Click or right click on the meter bar | -40..+12 dB | +12 | |
| Mono | toggle | on / off | off | |
| Solo | toggle | on / off | off | Soloing mutes non soloed strips on the monitored bus |
| Mute | toggle | on / off | off | |
| Gain fader | vertical slider | -60..+12 dB | 0 | 8 independent layers, one per bus |
| Reverb send | rotary | 0..10 | 0 | |
| Delay send | rotary | 0..10 | 0 | |
| FX1 send | rotary | 0..10 | 0 | External FX, needs hardware channel routing |
| FX2 send | rotary | 0..10 | 0 | |
| Post buttons | 4 toggles, one per send | pre / post fader | pre | |
| Bus assign A1..A5, B1..B3 | 8 toggles | on / off | A1 on | |
| Input meter | display | pre fader level, per channel, with peak hold | | |

**Standard edition only:** an `Audibility` knob, 0..10, a loudness style presence control. Implement it, it is cheap and users of the compact layout expect it.

## 1.4 Virtual input strip: every control

| Control | Type | Range | Notes |
|---|---|---|---|
| 3 band EQ | 3 rotaries | -12..+12 dB each | Bass, mid, treble |
| 5.1 pan pad | 2D pad | -0.5..+0.5 both axes | Positions between FL, FC, FR, RL, RR |
| M.C. button | toggle | on / off | Mutes the centre channel (channel 3) of multichannel material, used for real time dubbing |
| Karaoke button | 5 states | off, K-m, K-1, K-2, K-v | K-m removes the common mono content, K-1 keeps some bass and treble, K-2 keeps more of both, K-v filters the 200 to 4000 Hz vocal band. Present on the AUX virtual strip |
| Limiter | meter bar | -40..+12 dB | |
| Mono, Solo, Mute, fader, bus assigns | same as hardware | | |
| Connected application list | display and control | 4 apps visible, expandable to 11 by right click | Shows each app's volume, mute button, and live level meter, bidirectionally synced with the OS volume mixer |

## 1.5 Master section: every bus control

| Control | Type | Range | Notes |
|---|---|---|---|
| Device selector | click | system playback devices, A buses only | B buses are hardwired to virtual capture devices |
| Bus label | right click on slider base | string | Empty label hides the bus from the simplified remote view |
| SEL | toggle | on / off | Selects which per bus mix the strip faders display. Ctrl click selects several at once |
| Bus mode | cycling selector | 12 modes | Section 1.6 |
| Mono | 3 state | off, mono, stereo reverse | Click twice for the channel swap |
| EQ | toggle, right click opens dialog | on / off | Button colour encodes state per channel: green equalized or gain changed, red delayed, yellow both, black untouched |
| Mute | toggle | on / off | |
| Gain fader | slider | -60..+12 dB | |
| Reverb return | rotary | 0..10 | |
| Delay return | rotary | 0..10 | |
| FX1 return, FX2 return | rotary | 0..10 | |
| Monitor select | toggle | exclusive selection | Chooses which bus is heard on the monitoring bus |
| Output meter | display | per channel with peak hold | |

## 1.6 The 12 bus modes

Given the 8 channel layout `FL FR FC SW RL RR SL SR`:

| Mode | Behaviour |
|---|---|
| Normal | All channels passed unchanged |
| Mix Down A | `L = FL + 0.7*FC + SW + RL - SL`, `R = RL + 0.7*FC + SW - RR + SR`. Rear and side mixed out of phase to fake surround |
| Mix Down B | `L = FL + 0.7*FC + SW + RL + SL`, `R = RL + 0.7*FC + SW + RR + SR`. Rear and side in phase |
| Stereo Repeat | `ch1 ch3 ch5 ch7 = FL`, `ch2 ch4 ch6 ch8 = FR`. Combined with the bus EQ this becomes a 2, 3 or 4 way active crossover |
| Composite | The 8 channels are filled from the composite patch, taking any strip pre or post fader |
| Up Mix TV | 7.1 from stereo: `FL=L`, `FR=R`, `FC=0.2*(L+R)`, `SW=0.5*(L+R)`, `RL=SL=0.7*(L-R)`, `RR=SR=0.7*(R-L)` |
| Up Mix 2.1 | `FL=L`, `FR=R`, `SW=0.5*(L+R)` |
| Up Mix 4.1 | 2.1 plus `RL=L`, `RR=R` |
| Up Mix 6.1 | 4.1 plus `SL=L`, `SR=R` |
| Center Only | `L=R=FC` |
| LFE Only | `L=R=SW` |
| Rear Only | `L=RL`, `R=RR` |

Note that the published Mix Down formulas literally use `RL` on the right hand side of the RIGHT channel, which looks like a typo for `FR` in the vendor documentation. Implement `RIGHT = FR + 0.7*FC + SW ∓ RR ± SR` and cover it with a unit test, then leave a code comment about the discrepancy.

## 1.7 Parametric EQ engine

One shared implementation serves both the strip EQ and the bus EQ.

* 6 cells per channel.
* Bus EQ: independent settings for each of the 8 channels, plus a channel selector that can edit all channels at once.
* Strip EQ: stereo, 2 channels.
* Cell parameters: `on` (bool), `type` (7 types, index 0 to 6: typically peak, low pass, high pass, low shelf, high shelf, band pass, notch), `f` 20 Hz to 20 kHz, `gain` -12 to +12 dB in the API and up to -36 to +18 dB in the extended UI scale, `q` 1 to 100.
* Per channel `Trim` -24 to +24 dB and per channel `Delay` 0 to 500 ms.
* `FLAT` button resets according to the current channel selection.
* `A / B` memories for instant comparison, edits always land in the currently selected memory.
* `CH COPY` copies the current channel to another channel, `COPY ALL` copies every channel to another bus.
* Right click a gain, Q or frequency control to type an exact value.
* Right click the graph to change the dB scale of the display.
* Load and save the whole EQ set as a file, and copy settings between strip EQs and bus EQs since the parameter model is shared.

## 1.8 Internal FX

### Reverb (send / return)
Controls: `DRY` amount, `WET` amount, `DELAY` pre delay up to 1000 ms, `E.Ref` early reflection amount, `DECAY` scaling from 50 % to 200 % of the preset, a 3 band full parametric EQ inside the reverb, a bypass button that can bypass either the whole effect or only its EQ, and a preset grid of 10 x 3 presets. Plus an A/B memory and a global on/off.

### Multitap delay (send / return)
Controls: 8 stereo taps on a musical timeline. Per tap: gain, left/right pan, low pass and high pass filter. Two timeline views, one for gain and delay position, one for pan. A tempo in BPM with a 4/4 grid on top and a 3/4 grid at the bottom. `Time Ratio` knob to rescale the pattern to another tempo. `Set Note` dialog to enter a delay as a note duration relative to the tempo. `TAP` tempo button that shows a candidate tempo in blinking red after four taps, `Scale Fit` to accept it, `FIX DELAY` to snap all taps to the new tempo, `AUTO FIT` to do that automatically. Input balance knobs for left and right that allow mono or reversed stereo input. Feedback knob that returns the output into the input. `DRY`, `WET`, pre delay up to 1000 ms, soft bypass, A/B memory, global on/off.

### Multiband compressor (occupies the delay slot when selected)
5 bands. Band split frequencies adjustable by 4 controls sitting between the bands. Per band: input gain, threshold, ratio, attack, release, knee, output gain, brickwall limit, distortion (limiter reaction rate), enable, solo, mute. Global: link type (absolute, relative, independent) controlling how bands move together, output gain, auto make up, bypass, and settings load / save. Per band metering shows input, output and gain reduction.

Routing warning to reproduce in the UI: when a strip feeds the multiband compressor and the buses take the FX return, that strip's direct bus assignments must be off or the signal is counted twice.

### External FX
Two true aux paths with send and return knobs, routed to physical hardware channels. On Windows this requires ASIO. On macOS this is an aggregate device channel pair. See Part 2.

## 1.9 Recorder / tape deck

* Playback of common audio and video container formats, with a transport, a clickable progress bar, playback gain, `PLAY ON LOAD` and `LOOP` options.
* Recording sources: either "pre fader inputs", meaning one or all inputs summed to stereo at their original gain and ignoring faders, FX, mute and solo, or "post fader outputs", meaning the output of a chosen bus, 2 to 8 channels, and it works with composite mode so you can build an arbitrary multitrack layout on an unconnected bus.
* File type: WAV, BWF, AIFF, or MP3 at 32 to 320 kbps. MP3 is stereo only, the others take up to 8 channels.
* Recording sample rate is independent of the engine sample rate.
* Multitrack option writes one file per channel with `_Track1`, `_Track2` suffixes in addition to the main file.
* Pre-recording buffer: the engine records continuously into a rolling buffer, 20 seconds by default, and that buffer is prepended to the file when you hit record. Configurable, 0 disables it.
* Target directory and filename prefix are configurable. Default naming pattern is `Prefix YYYY-MM-DD at HHhMMmSSs.ext`.
* Stop record after a duration timer, `00:00:00` disables it.
* Recorder state changes (play, stop, rec, end of file) are broadcast as events that macro buttons can react to.

## 1.10 Main menu, every item

* Restart audio engine.
* Auto restart audio engine when the A1 device disconnects.
* Auto restart audio engine when any device disconnects.
* Release the audio file held by the tape recorder.
* Load settings from file, save settings to file.
* Load a specific settings file on startup.
* Reset all settings.
* Run in the system tray.
* Run at system startup.
* Show the window on launch.
* Always on top.
* Lock the interface to prevent accidental changes.
* Launch the companion apps (macro buttons, simplified remote view) at startup, and open the other bundled tools.
* Hook keyboard keys to control volumes (bus A1, bus A2, strip 1).
* Limit remote gain from MIDI to 0 dB instead of +12 dB.
* Show contextual help in the caption bar, on by default.
* Show the preset scene dialog on startup.
* Open the system settings dialog, the recorder options dialog, the MIDI mapping dialog, the network dialog.
* Driver installation check.
* About box.
* Quit.

## 1.11 System settings dialog, every field

* Device selection for the main output, which becomes the clock master and defines the engine sample rate.
* Preferred sample rate: 44.1, 48, 88.2, 96, 176.4 or 192 kHz. The device itself may run at 32 kHz as well.
* Buffer size per audio API, 128 to 2048 samples, one value per API family.
* Exclusive mode toggle for inputs.
* Per bus output delay, 0 to 500 ms, one per bus.
* Monitoring bus selection, used by the MON button in the simplified remote view.
* Hardware channel patch for physical inputs (which device channels feed which strip).
* Hardware channel patch for buses A2 to A5 (A1 always occupies the first 8 output channels).
* External FX patch: send and return channel assignments.
* Composite patch: 8 slots, each selecting one of the possible input channels (index 0 means the default bus channel, 1 to 22 select an input channel).
* Composite pre fader or post fader switch.
* Insert patch: an on/off toggle for each of the 22 input channels, sending it out to an external processor and back.
* Insert point pre FX or post FX switch.
* Engine mode and internal clock behaviour, including running with no output device at all on an internal clock.

## 1.12 MIDI mapping

* Select a MIDI input device and an optional MIDI output device.
* Learn mode: touch a control in the UI, move a knob on the controller, mapping is stored.
* Any mixer parameter can be a mapping target, including the four visible application volumes on virtual strips.
* MIDI feedback: send values back to the controller so motorised faders and LED rings stay in sync.
* MIDI forward: pass incoming MIDI through to another destination.
* Advanced feedback rules for controllers that need explicit refresh messages.
* Incoming MIDI can also arrive over the network protocol rather than from a physical port.

## 1.13 Network audio (VBAN class functionality)

The protocol is public and free to implement, and interoperating with it is the single highest value feature for a musician, because phone apps and other machines can then send and receive audio to Loomix.

* UDP based, default port 6980.
* 8 incoming audio streams, each landing on any chosen input strip.
* 8 outgoing audio streams, each sourced from any chosen bus.
* Stream parameters: name, source IP address, port, sample rate from 11025 Hz to 96 kHz, 16 or 24 bit, 1 to 8 channels.
* A stream is identified by name plus source IP plus port, and all three must match on the receiver.
* Broadcast to `x.x.x.255` on wired networks.
* Auto discovery: right click the stream name field to list detected incoming streams.
* Handshake ping that validates a connection, with an info indicator and remote unit details.
* Network quality setting from fast to slow, which sizes the jitter buffer.
* Five error indicators per stream: overload (too many packets), corrupt, disorder (late packets), missing (lost packets), underrun (too few packets).
* Text and MIDI sub protocols carried over the same transport, for remote control and for MIDI over network.
* User name and colour for identification, and a simple chat service between units.
* Optional screen frame streaming for remote control of the interface. **Skip this in Loomix.** It is a video product bolted onto an audio product and it will eat months. Explicitly out of scope.

## 1.14 Macro buttons application

* A grid of 4 to 80 buttons, each either push type or two position (toggle) type.
* Each button holds three scripts: an initialisation script run at startup, a script on press, and a script on release.
* Per button: title, subtitle, one of 9 background colours, an image for the off state and an image for the on state.
* Trigger sources per button: keyboard shortcut including mouse button combinations, MIDI message with a learn checkbox and a reset button, game controller input for up to 4 controllers, raw HID device input, and an audio level trigger.
* Audio level trigger: choose an input strip, set an IN threshold that presses the button when the level rises above it, an OUT threshold that releases it when the level falls below it, and a HOLD time that keeps it engaged for a minimum period. This is how auto ducking and push to talk are built.
* React to mixer events, in particular the recorder transport events.
* System actions: execute a program with a command line, send keyboard events to the OS, send MIDI messages to up to 2 devices, send network text or MIDI requests to remote instances.

## 1.15 Remote control API and request script

A tiny scripting language drives everything. Reproduce the exact grammar so existing user scripts port over.

```
Strip(0).mute = 1;          // absolute set
Strip(0).mute += 1;         // toggle by relative change
Bus(0).gain = -10.0;
Strip(0).gain -= 3;         // relative
Command.Restart = 1;
Command.Load = "/path/to/config.json";
Strip(0).FadeTo = (-10.0, 500);   // reach -10 dB over 500 ms
Strip(0).FadeBy = (-3.0, 2000);   // change by -3 dB over 2 s
```

Rules: statements separated by semicolons, `//` line comments, zero based indices, parameter paths are case insensitive in practice but documented in PascalCase, fade times range from 0 to 120000 ms.

**Full parameter namespace to implement:**

*Strip:* `Mono`, `Mute`, `Solo`, `MC`, `Gain`, `GainLayer[j]`, `Pan_x`, `Pan_y`, `Color_x`, `Color_y`, `fx_x`, `fx_y`, `Audibility`, `Comp`, `Comp.GainIn`, `Comp.Ratio`, `Comp.Threshold`, `Comp.Attack`, `Comp.Release`, `Comp.Knee`, `Comp.GainOut`, `Comp.MakeUp`, `Gate`, `Gate.Threshold`, `Gate.Damping`, `Gate.BPSidechain`, `Gate.Attack`, `Gate.Hold`, `Gate.Release`, `Denoiser`, `Denoiser.Threshold`, `Denoiser.Bypass`, `Pitch.On`, `Pitch.DryWet`, `Pitch.PitchValue`, `Pitch.LoFormant`, `Pitch.MedFormant`, `Pitch.HiFormant`, `Pitch.RecallPreset`, `Karaoke`, `Limit`, `EQGain1..3`, `Label`, `A1..A5`, `B1..B3`, `FadeTo`, `FadeBy`, `Reverb`, `Delay`, `Fx1`, `Fx2`, `PostReverb`, `PostDelay`, `PostFx1`, `PostFx2`, `EQ.on`, `EQ.AB`, `EQ.channel[j].cell[k].{on,type,f,gain,q}`, `App[k].Gain`, `App[k].Mute`, `AppGain`, `AppMute`, `device.name`, `device.sr`.

*Bus:* `Mono`, `Mute`, `EQ.on`, `EQ.AB`, `Gain`, `Label`, `mode.{normal,Amix,Bmix,Repeat,Composite,TVMix,UpMix21,UpMix41,UpMix61,CenterOnly,LFEOnly,RearOnly}`, `EQ.channel[j].cell[k].{on,type,f,gain,q}`, `EQ.channel[j].Trim`, `EQ.channel[j].Delay`, `FadeTo`, `FadeBy`, `Sel`, `ReturnReverb`, `ReturnDelay`, `ReturnFx1`, `ReturnFx2`, `Monitor`, `device.*`.

*FX:* `Fx.Reverb.On`, `Fx.Reverb.AB`, `Fx.Delay.On`, `Fx.Delay.AB`.

*Patch:* `patch.hw[i]`, `patch.OutA2[i]`..`patch.OutA5[i]`, `Patch.composite[j]` (0 to 22), `Patch.insert[k]` (0 to 21), `Patch.PostFaderComposite`, `Patch.PostFxInsert`.

*Options:* `Option.sr`, `Option.delay[i]`, `Option.buffer.*`, `Option.mode.exclusif`, plus the recorder and network options.

Expose this over three transports: a local unix domain socket, the network text protocol, and a C ABI shared library so third party controllers (stream decks, custom hardware) can bind to it.

## 1.16 Preset scenes

64 preset slots stored in a dedicated folder. Recall by click, by Enter, or by function keys F1 to F24 for the first 24. Right click for the context menu to store into an empty slot or overwrite an existing one. Each preset carries a name and a comment, defaulting to the creation timestamp. Import, export, copy and paste. **Presets contain mixing state and audio FX only. They must not contain device selections, system settings, MIDI mappings or network configuration.** That separation is what makes presets portable, do not blur it.

## 1.17 Bundled companion tools

* **Simplified remote view.** A resizable, minimal interface showing only labelled strips and labelled buses. Its main value on an 8 bus system is direct access to every per bus sub mix, so each strip shows one fader per bus. A MON button per bus routes that bus to the monitoring bus. A slider link mode: no link, absolute, or relative, so a controller can move sub mixes together while preserving their offsets. Connects either directly to the local engine or to up to 4 remote instances over the network.
* **8x8 gain matrix.** A plain 8 by 8 gain matrix applied to a selected bus, for redistributing channels to odd speaker layouts.
* **15 band graphic EQ.** A simple stereo graphic EQ applied to a selected bus.
* **MIDI to network bridge.** Converts a physical MIDI port into a network MIDI stream and back.
* **Virtual driver control panel.** Shows every virtual I/O pair, its buffer statistics, and lets you change the driver side latency for testing. Warns in red when the configured latency is too small for the running streams.
* **Device checker.** Diagnostic tool that lists installed virtual devices and reports installation problems.

## 1.18 Interaction conventions to reproduce exactly

These micro behaviours are what make the original feel fast. Users notice their absence immediately.

| Gesture | Effect |
|---|---|
| Double click any control | Reset it to its default value |
| Right click a knob with a detail view (comp, gate, denoiser) | Toggle the detail panel in place of the pad |
| Right click a numeric control | Open a small inline edit box to type an exact value |
| Ctrl + right click a parameter | Edit the value |
| Ctrl + right click the comp / gate / denoiser knob | Undo, restoring the detailed parameters after the macro knob overwrote them |
| Shift + click a parameter | Open a high precision slider |
| Right click the slider base | Rename the strip or bus |
| Right click the 2D pad | Cycle Color, Position, Modulation |
| Right click the meter bar | Type the limiter threshold |
| Right click the strip header area | Open the strip menu: reset, copy, paste, load, save, either for the whole strip or for one effect |
| Right click the tape deck | Open recorder options |
| Ctrl + click SEL | Multi select buses |
| A "PRG" marker on a macro knob | Indicates detailed parameters diverge from the knob position |

## 1.19 Latency, clocking and known failure modes

Reproduce these behaviours and the accompanying warnings.

* The main output device is the clock master. Everything else is resampled to it.
* The engine can run on an internal clock with no output device selected.
* Outputs A1 through A5 are not sample synchronous with each other when they run on different physical devices, and the UI should say so rather than pretend otherwise.
* Virtual device internal latency should be at least 3 times the engine buffer size. Large values are safe, small values are lower latency and riskier.
* Loopback into a recording application will feed back infinitely unless the return path is muted, warn about it in the UI.
* When a device disappears the engine stops. Auto restart options exist for exactly this reason.
* Settings live in a user visible folder, and a command line interface can drive install, launch and configuration.

---

# PART 2 — macOS REALITY MAPPING

Read this section before writing a line of code. Voicemeeter is not portable software, it is a Windows kernel audio product wearing a mixer skin. About 70 % of it translates cleanly, 20 % needs re-engineering, and 10 % has no macOS equivalent at all.

## 2.1 The single hardest requirement

Voicemeeter's whole reason for existing is that it installs virtual audio devices that the rest of the OS can select. On macOS the equivalent is an **AudioServerPlugIn**, a user space CoreAudio HAL plug-in bundle installed to `/Library/Audio/Plug-Ins/HAL/`, loaded by the `coreaudiod` daemon.

Key facts:

* It runs in user space. No kernel extension, no DriverKit, no reduced security mode.
* It must be signed with a Developer ID Application certificate and the containing installer package must be signed with a Developer ID Installer certificate and notarised, otherwise other people cannot install it. For local development an ad-hoc signature plus `sudo killall coreaudiod` is enough.
* `BlackHole` is the canonical open source reference implementation, MIT licensed. **Read it, learn from it, credit it, but write the driver rather than vendoring it,** because Loomix needs 8 independent device pairs with a private control channel, which is a different shape.
* Installing or updating requires restarting `coreaudiod`, which briefly interrupts all audio on the machine. The installer must warn about this.
* The driver and the mixer app are separate processes. The driver just presents ring buffers. The mixer app opens those devices like any other client and does the actual mixing.

## 2.2 API translation table

| Windows / Voicemeeter concept | macOS equivalent | Notes |
|---|---|---|
| MME, DirectSound | none needed | Legacy compatibility layers, drop them |
| WDM / WASAPI shared | CoreAudio HAL, standard mode | Default path |
| WASAPI exclusive, KS, WaveRT | Hog mode via `kAudioDevicePropertyHogMode` | Same idea, exclusive access to the device |
| ASIO | CoreAudio is already low latency | There is no ASIO on macOS and none is needed |
| ASIO multichannel routing to 64 I/O | Aggregate device plus channel maps | Create or consume an aggregate device, address individual channels of a multichannel interface |
| Virtual ASIO ports for DAWs | The same virtual devices | Any macOS DAW takes CoreAudio devices, so no separate driver type is needed |
| Buffer size per API family | `kAudioDevicePropertyBufferFrameSize` per device, plus safety offset | Query the device's allowed range instead of hardcoding 128 to 2048 |
| Exclusive input mode | Hog mode plus `kAudioDevicePropertyDeviceIsRunningSomewhere` checks | |
| Windows Volume Mixer per app control | Core Audio process taps, macOS 14.4 and newer | `AudioHardwareCreateProcessTap`, `CATapDescription`, `kAudioHardwarePropertyProcessObjectList`. This gives per process capture and per process identification. Gate the whole application list feature behind an availability check and hide it gracefully on older systems |
| Registry parameters | `~/Library/Preferences` via `UserDefaults` plus a JSON settings file under `~/Library/Application Support/Loomix/` | |
| Settings folder in My Documents | `~/Library/Application Support/Loomix/` with presets in `Presets/`, recordings default to `~/Music/Loomix/` | |
| Run at startup | `SMAppService.mainApp.register()` | Requires macOS 13 and newer |
| System tray | `NSStatusItem` menu bar extra | |
| Global keyboard hooks | `CGEventTap` | Needs the Accessibility permission, must be requested with a clear explanation |
| XInput game controllers | `GameController` framework | |
| Raw HID devices | `IOHIDManager` | |
| MIDI devices | `CoreMIDI`, including virtual endpoints | |
| Reboot after install | `sudo killall coreaudiod` | No reboot needed, but every audio app on the machine glitches for a moment |
| Device disconnect detection | `kAudioHardwarePropertyDevices` listener plus per device alive listeners | Drives the auto restart options |
| MP3 encoding | `libmp3lame` via a Rust binding, or ship AAC in `.m4a` through `AVAudioFile` | MP3 patents have expired, so either is fine |

## 2.3 Things that must be re-engineered, not ported

**Multi device clock drift.** Windows and macOS both have the problem, but on macOS the correct solution is well defined: pick one device as the clock master (bus A1's device, or the internal clock if none), and run an asynchronous sample rate converter with a drift tracking loop on every other device. Measure drift from the difference between the device's sample time and the master's, filter it with a slow PI controller, and feed the ratio into a polyphase resampler. Never resample with a naive fixed ratio, it will click every few minutes. Alternatively offer an aggregate device mode where CoreAudio does the drift compensation, and let the user choose.

**Per application volume.** On Windows this is a first class OS feature. On macOS you have to build it from process taps, and even then you get level and capture, not a system volume slider you can move. Design the UI so the application list shows level and a Loomix side gain and mute, and be honest in the docs that it is not the same thing as the Windows volume mixer.

**The insert points.** On Windows these hang off ASIO channels. On macOS, model an insert as a send to and return from a channel pair of a chosen hardware or aggregate device, or, better, as an **Audio Unit v3 host slot**, so users can drop a real plug-in on a strip. That is a genuine improvement over the original and it is the feature a guitarist will actually use.

**Screen frame streaming.** Out of scope, do not build it.

## 2.4 Recommended additions beyond parity

Small, cheap, and directly relevant to musicians and home studio users:

1. **Audio Unit v3 host slots** on every strip and bus, 2 slots each, with the plug-in window opened out of process. This replaces the external FX loop for most users.
2. **A tuner and a metronome** on the strip detail panel. The multitap delay already tracks tempo, so a metronome is nearly free.
3. **Loopback safe recording**, where the recorder refuses to arm on a source that would feed back and says why.
4. Keep everything else identical to the original so muscle memory transfers.

---

# PART 3 — ARCHITECTURE AND MILESTONES

## 3.1 Language and stack decisions

| Component | Language | Why |
|---|---|---|
| `loomix-driver` virtual audio driver | C, built as an `AudioServerPlugIn` bundle | The CoreAudio plug-in ABI is C. No runtime, no allocations, no exceptions |
| `loomix-core` DSP and mixing engine | Rust, `#![no_std]` compatible in the hot path where practical | Real time safety, and it is the user's strongest language |
| `loomix-hal` CoreAudio integration | Rust with `coreaudio-sys` bindings, plus a thin Objective-C shim where the API is not C | Device enumeration, IOProc, hog mode, aggregate devices, process taps |
| `loomix-net` network protocol | Rust | Packet parsing is attack surface, memory safety matters |
| `loomix-rpc` control API | Rust, exposes a unix socket, a C ABI `cdylib`, and the text request script | |
| `loomix-cli` | Rust | Scriptable control, and the thing CI actually drives |
| Desktop UI | Tauri v2 with React and TypeScript | Reuses the Rust core in process, ships a small binary, and the user knows TypeScript |
| Installer | `pkgbuild` plus `productbuild`, driven by a shell script | |

**Rejected alternatives:** Electron (bundle size and audio thread separation), Python for the engine (no real time guarantees), SwiftUI for the whole app (would fragment the codebase across two languages for no gain given the Rust core), a kernel extension (unnecessary and unshippable).

## 3.2 Repository layout

```
loomix/
  Cargo.toml                 # workspace
  crates/
    loomix-core/             # pure DSP, no I/O, no OS calls, no allocation in process()
    loomix-hal/              # CoreAudio device I/O, drift correction, aggregate devices
    loomix-net/              # network audio, text and MIDI sub protocols
    loomix-rpc/              # request script parser, unix socket server, C ABI
    loomix-recorder/         # file writers, ring buffer, multitrack
    loomix-config/           # settings and preset serialisation plus migrations
    loomix-cli/              # command line front end
    loomix-app/              # Tauri backend, wires everything together
  driver/
    LoomixAudioDriver/       # AudioServerPlugIn, C sources, Xcode project
    tests/                   # driver level integration tests
  ui/                        # React + TypeScript front end
  packaging/
    build-pkg.sh
    scripts/postinstall
  docs/
    SPEC.md                  # this file
    ARCHITECTURE.md
    DSP.md                   # every filter's transfer function and its reference test
    PROTOCOL.md
    TROUBLESHOOTING.md
  testdata/
    golden/                  # reference WAV renders, generated once, checked in with hashes
    fixtures/
  .github/workflows/
```

## 3.3 Real time safety rules, non negotiable

The audio callback must never: allocate, free, lock a mutex, take an `RwLock`, do file or network I/O, log with a formatting allocator, or panic. Enforce this mechanically, not by discipline:

* Parameters cross into the audio thread through a triple buffer or a lock free SPSC queue (`rtrb`, `triple_buffer`), never through a shared `Mutex`.
* Meters and level data cross back out the same way.
* Under `cfg(test)` install a global allocator that panics when a thread local "in audio callback" flag is set. Every DSP test wraps `process()` in that flag. **This is the single most valuable test in the project.**
* Flush denormals in the process callback, and cover it with a test that feeds a decaying signal to silence and asserts the CPU cost does not blow up.
* All buffers are pre-allocated at engine start based on the maximum block size, and the engine reallocates only on an explicit restart.

## 3.4 Milestones

Work strictly in order. Each milestone ends with a green CI run, a tagged commit, and an updated CHANGELOG. Do not start the next one until the previous one is merged.

**M0 — Skeleton.** Workspace, CI green on an empty test, licences, README, `docs/ARCHITECTURE.md`, conventional commits, `rustfmt` and `clippy` configured with warnings denied.

**M1 — Virtual driver, one pair.** A single virtual device pair appears in Audio MIDI Setup, passes audio through, survives a `coreaudiod` restart, supports 44.1 through 192 kHz and 2 to 8 channels. Local install script and uninstall script. **Acceptance:** play into `Loomix In 1` from any app, capture it with a test client, compare against the source with a bit exact test.

**M2 — Driver, full topology.** All 8 input pairs and 8 output endpoints, stable device UIDs, correct channel layouts, configurable driver side latency, a control channel so the app can query buffer statistics.

**M3 — Engine core, no effects.** 8 strips, 8 buses, the full 8 by 8 matrix, per bus gain layers, mute, solo, mono, fader law, bus assignment, meters. Offline deterministic rendering harness. **Acceptance:** the routing truth table test passes for every combination.

**M4 — Hardware I/O and clocking.** Device enumeration, selection, hog mode, clock master selection, drift corrected resampling, internal clock fallback, hot plug handling, auto restart options. **Acceptance:** a 30 minute soak test on two devices with different clocks shows no dropouts and bounded drift.

*At the end of M4 the product is already usable daily. Ship a `0.1.0` here.*

**M5 — Strip processing.** Gate, compressor with the macro knob mapping, limiter, denoiser, pan laws, the three pad modes, the 3 band EQ on virtual strips, M.C., karaoke modes.

**M6 — Parametric EQ.** The shared 6 cell engine, per channel on buses, trim, delay, A/B memories, copy operations, load and save, and the EQ graph in the UI.

**M7 — Bus modes and patching.** All 12 modes, composite patch, insert patch, pre and post switches.

**M8 — Internal FX.** Reverb, multitap delay with tempo, multiband compressor, sends and returns with pre and post buttons.

**M9 — Recorder.** Playback and recording, all source modes, formats, multitrack, pre-record buffer, timer.

**M10 — Control surface.** Request script parser, unix socket, C ABI, CLI, MIDI mapping with learn and feedback, macro buttons with all trigger types including the audio level trigger.

**M11 — Network audio.** Incoming and outgoing streams, discovery, jitter buffer, error indicators, text and MIDI sub protocols. Fuzz the parser before merging.

**M12 — Polish and release.** Preset scenes, simplified remote view, menu bar mode, login item, first run wizard, signed and notarised installer, uninstaller, documentation site.

---

# PART 4 — TESTING AND CI

This section is a requirement, not a suggestion. A mixer that is "mostly right" is worse than no mixer, because the failures are intermittent and appear during a recording session.

## 4.1 Test layers

**Layer 1, unit tests in `loomix-core`.** Pure functions, no I/O. Every DSP block gets:
* A known answer test against a reference computed independently. For biquads, compare the impulse response against coefficients derived from the Audio EQ Cookbook formulas, tolerance `1e-6`.
* A frequency response test: sweep, FFT, assert gain at the centre frequency is within 0.1 dB of the target and that the skirt matches the requested Q.
* A stability test: random parameter automation at audio rate for 10 seconds, assert no NaN, no infinity, and output bounded.
* A null test where the effect is bypassed or set to neutral, asserting bit exact passthrough. Every effect must have a true neutral setting.

**Layer 2, property tests with `proptest`.**
* Fader law is monotonic and continuous across the whole range, and `-inf` produces digital silence.
* Pan law preserves energy within 0.01 dB across the sweep.
* Any sequence of parameter changes leaves the engine in a valid state.
* `gain_db_to_linear` and its inverse round trip within tolerance.

**Layer 3, the real time safety test.** The custom panicking allocator described in 3.3. Run every effect and the full engine `process()` under it. **A pull request that allocates in the audio callback fails CI.**

**Layer 4, golden file rendering.** A deterministic offline harness renders fixed input WAVs through a fixed configuration and compares the output to a checked in reference, sample by sample with a small tolerance, plus a spectral distance check so tiny denormal differences do not cause false failures. Store references in `testdata/golden/` with a generator script so they can be regenerated deliberately and reviewed in the diff.

**Layer 5, routing matrix truth table.** Enumerate every combination of strip to bus assignment, mute, solo and gain layer that matters, feed a distinct identifiable tone per strip, and assert exactly the expected tones appear at exactly the expected level on each bus. This is generated, not hand written, and it is the test that catches the bugs that make people give up on a mixer.

**Layer 6, protocol tests.**
* `cargo fuzz` targets for the network packet parser and the request script parser, run in CI on every push with a short time budget and nightly with a long one.
* Round trip tests: serialise every settings struct, parse it back, assert equality.
* Migration tests: every historical settings schema version has a fixture file that must still load.
* Golden tests for the request script: a corpus of scripts and their expected resulting state.

**Layer 7, driver integration tests.** These need a real macOS runner with the driver installed.
* Install the driver, assert the devices appear with the expected UIDs, channel counts and supported sample rates.
* Bit exact loopback: write a known buffer to a virtual output, read it from the paired virtual input, assert equality.
* Sample rate switching under load.
* Uninstall cleanly, assert the devices are gone.

**Layer 8, end to end.** Start the app headless with a null audio backend, drive it through the CLI, assert the resulting state and the rendered output. Add a UI test pass with Playwright against the Tauri webview for the critical flows only: device selection, fader move, preset recall.

**Layer 9, performance regression.** `criterion` benchmarks for the full engine at 8 strips by 8 buses with all effects engaged, at 48 kHz and at 96 kHz. CI fails if the mean regresses by more than 10 % against the stored baseline. Also assert absolute CPU headroom: a full configuration at 48 kHz with a 128 sample buffer must use less than 25 % of one performance core on an M1.

**Layer 10, soak.** A nightly job that runs the engine for 2 hours with randomised parameter automation, asserting zero dropouts, no memory growth beyond a small threshold, and bounded clock drift.

## 4.2 Coverage and quality gates

* Line coverage via `cargo llvm-cov`, minimum 80 % overall and 90 % in `loomix-core`. The gate fails the build, it does not just report.
* `cargo clippy --all-targets --all-features -- -D warnings`.
* `cargo fmt --check`, `cargo deny check` for licences and advisories, `cargo audit`.
* TypeScript: `tsc --noEmit`, `eslint --max-warnings 0`, `vitest` with coverage.
* C driver: build with `-Wall -Wextra -Werror`, run `clang-analyze` and a `clang-tidy` pass.
* No `unsafe` outside `loomix-hal` and the driver bindings, enforced with `#![forbid(unsafe_code)]` in every other crate.

## 4.3 GitHub Actions workflows

`.github/workflows/ci.yml` runs on every push and pull request:

```yaml
name: ci
on:
  push: { branches: [main] }
  pull_request:
env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: "-D warnings"
jobs:
  lint:
    runs-on: macos-15
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: rustfmt, clippy }
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets --all-features
      - run: cargo deny check
  test:
    runs-on: macos-15
    strategy:
      matrix: { profile: [debug, release] }
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace --all-features
      - run: cargo test --workspace --release -- --ignored golden
  rt_safety:
    runs-on: macos-15
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test -p loomix-core --features rt-assert -- realtime
  coverage:
    runs-on: macos-15
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: taiki-e/install-action@cargo-llvm-cov
      - run: cargo llvm-cov --workspace --lcov --output-path lcov.info --fail-under-lines 80
      - uses: codecov/codecov-action@v4
  driver:
    runs-on: macos-15
    steps:
      - uses: actions/checkout@v4
      - run: xcodebuild -project driver/LoomixAudioDriver.xcodeproj -scheme LoomixAudioDriver -configuration Release build CODE_SIGNING_ALLOWED=NO
      - run: ./driver/tests/run-static-checks.sh
  ui:
    runs-on: macos-15
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 22, cache: npm, cache-dependency-path: ui/package-lock.json }
      - run: npm ci --prefix ui
      - run: npm run --prefix ui typecheck
      - run: npm run --prefix ui lint
      - run: npm run --prefix ui test -- --coverage
  bench:
    runs-on: macos-15
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo bench -p loomix-core -- --save-baseline pr
      - run: ./scripts/check-bench-regression.sh 10
```

`.github/workflows/nightly.yml`: long fuzzing, the 2 hour soak, and a dependency freshness report.

`.github/workflows/release.yml`, triggered by a `v*` tag: build universal binaries for `aarch64-apple-darwin` and `x86_64-apple-darwin`, sign the driver and the app with the Developer ID certificate from secrets, build the `.pkg`, notarise with `notarytool`, staple, attach to a GitHub release, and generate release notes from conventional commits.

Also add: Dependabot for cargo, npm and actions; a CODEOWNERS file; a pull request template with a checklist that includes "no allocation in the audio thread" and "golden files regenerated deliberately"; and branch protection requiring every job above.

## 4.4 Definition of done for any milestone

1. Code merged to `main` through a pull request, never pushed directly.
2. Every CI job green.
3. New behaviour covered by tests at the appropriate layer, and the coverage gate still passes.
4. `docs/` updated, including `DSP.md` if a filter changed.
5. CHANGELOG entry under Keep a Changelog format.
6. Manual smoke test recorded in the pull request: which macOS version, which audio interface, what was verified.
7. No new `clippy` allow attributes without a comment explaining why.

## 4.5 Non goals, write these into the README

* No Windows or Linux support.
* No screen sharing or video streaming.
* No licence activation, no telemetry, no analytics.
* No cloud accounts.
* No attempt to bypass any macOS security mechanism. The driver is signed and notarised or it is not shipped.
