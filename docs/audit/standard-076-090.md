# Voicemeeter Standard manual, pages 76-90

Source: ~/Documents/loomix-refs/Voicemeeter_UserManual.pdf ("VOICEMEETER Standard", Version 1.1.2.2, Dec 2025). Page 78 is a pure section-divider page ("EXTRA OPTIONS -- Voicemeeter Latency, M.I.D.I. Mapping, Specifications, VAIO Extension, Known issues, Command line Installation"), no controls to extract.

## Manage lights Network in DMX 512 (p.76)

- **`System.DMXSetValue(addr, channel, value)`** (p.76, since MacroButtons 1.0.2.7): sets one DMX value at a given device address and channel, over a DMX serial (COM) interface selected in MacroButtons' own DMX configuration dialog. Can also take several values at once (`System.DMXSetValue(addr, channel, value1, value2, value3, value4...)`), which automatically fill the following channels.
- **`System.DMXCommit()`** (p.76): sends the newly modified DMX frame out over the interface.
- **Worked example** (p.76): a "2 Positions" button ("DMX" / "test red - blue") whose Trigger-IN script sets 4 DMX values, and whose Trigger-OUT script sets a different set plus commits the frame.
- **Tested hardware** (p.76): "Enttec Open DMX USB Interface" named as a confirmed-working DMX serial interface.

## Voicemeeter Remote API, for developers (p.77)

- **`VoicemeeterRemote.dll`** (p.77): the interface any third-party application uses to control Voicemeeter and take advantage of all its features, in any programming language. Named functions shown: `Login()`, `Logout()`, `GetVoicemeeterType()`, `GetVoicemeeterVersion()`, `IsParametersDirty()`, `GetParameterFloat()`, `GetParameterStringA()`, `GetParameterStringW()`, `SetParameterFloat()`, etc.
- **Client capacity** (p.77): up to 4 client applications can remote a Voicemeeter (or Banana) instance simultaneously.
- **Audio Callback API, 3 distinct points** (p.77, since Voicemeeter 1.0.5.0 / 2.0.3.0): an AUDIO API lets a client process audio directly inside Voicemeeter, at 3 different points in the signal path:
  1. **AUDIO CALLBACK IN**: 22 inputs (channel in) / 22 outputs (channel out) -- a "Pre-Strip Input Insert" driver, ahead of the strip chain, described as "2+2+2+8+8 channels."
  2. **AUDIO CALLBACK OUT**: 40 inputs / 40 outputs -- a "Pre-Master BUS Insert" driver, ahead of the master/bus stage, described as "8+8+8+8+8 channels."
  3. **AUDIO CALLBACK MAIN**: 22 inputs / 40 outputs -- a "Main driver with all I/O", covering both stages at once; outputs described as "5x8 channels."

  Flagged precisely for the classification pass: this diagram's own channel-count breakdown (2+2+2+8+8=22 inputs; 5 buses x 8 channels=40 outputs) reads as a **5-strip, 5-bus topology**, not the Standard edition's own 3-strip/2-bus one described everywhere else in this same manual -- the diagram is very likely illustrating Banana's (or a generic/larger) topology as a stand-in example inside the Standard manual, or this Audio Callback API itself may only be meaningfully exercised on Banana/Potato. Not resolved here; corroborate against Banana's and Potato's own manuals before drawing a conclusion.

## System Settings / Options dialog (p.79-81)

- **Opening the dialog** (p.79): click the Master Meter LCD section.
- **Per-device status rows** (p.79): for each of IN1, IN2 (physical inputs) and OUT A1 (Main Device), OUT A2 -- Status (ON/OFF), SR (current samplerate, can differ per device), buf (current buffer size), Ch (channel count, 1-2 on inputs, up to 8 on outputs), r (bit resolution, 16 bits by default), and for WDM devices only, S (share-mode indicator; KS is normally exclusive, MME normally shared).
- **Buffering fields** (p.79): Buffering MME (default 1024), Buffering WDM (default 512), Buffering KS (default 512), Buffering Clock (default 512, used for the internal-clock fallback), Buffering ASIO (Default = driver's own preferred size) with its own SR override field (Default).
- **Monitoring Synchro Delay** (p.79): a per-output field, OUT A1 and OUT A2 each independently, in ms (0.00 shown).
- **Out Limiter** (p.79-80): an ON/OFF toggle. Enabled by default, it "enables brick wall limiters (based on VB-Audio C-Limiter) on every output BUS. Otherwise a simple peak remover will be applied." A genuine, real, system-wide toggle switching every bus between two distinct behaviors.
- **VAIO sync** (p.79-80): a mode field, shown as "STRICT." In this mode "all virtual I/O are strictly synchronized with the Voicemeeter main stream (given by the output A1 device)... expected to bring more stability (especially for small latency)."
- **Virtual ASIO Type** (p.79): a dropdown, e.g. "Float32LSB."
- **Preferred Main SampleRate** (p.79-80): 44.1 / 48 / 88.2 / 96 / 176.4 / 192 kHz -- used as a wish for output device A1's rate, but the actual device's own current configuration can override it.
- **WDM Input Exclusive Mode** (p.79-81): a Yes/No field. Explicitly documented as a **disabled/deprecated option** (p.80, in visually distinct styling from the surrounding text): "DISBALED [sic] OPTION: Activating WDM Input Exclusive mode (and Swift mode) will force audio engine to use smallest buffer with WDM input devices. But this exclusive mode could not be stable and produce various potential problems in the time (from audio crash to system freeze/crash -- pending on audio driver and O/S -- see known issues section)."
- **Engine Mode** (p.79-81): a dropdown, "Normal" shown. Also offers a **SWIFT** mode -- similarly flagged as disabled/deprecated in the same passage: "Engine mode provides a SWIFT mode to possibly improve real time (experimental option)... These both modes have been disabled because generating too much support."
- **Internal clock fallback, mechanics** (p.80): if no device is selected as output A1, Voicemeeter runs on an internal clock at the Preferred SampleRate and the Buffering Clock parameter, allowing operation with no physical audio output device at all.
- **Buffer size ranges by driver type** (p.80): MME supports 512 to 2048 samples; WDM and KS can go down to 256 samples ("that makes audio processing very close to the real time -- practically usable to sing on a song in real time -- karaoke -- or to play digital piano on music in real time"). Default MME=1024, default WDM=512, chosen because "these default settings should work for 100% PC configuration cases," not for lowest latency.
- **ASIO Driver support for output A1** (p.80-81): selecting an ASIO device as output A1 runs Voicemeeter "in audio pro conditions (like any DAW using ASIO device)." A "PATCH ASIO Inputs to Strips" control maps specific ASIO input channels onto Hardware Input #1/#2's L and R; the main output bus automatically uses the ASIO driver's first 8 output channels, with other buses patchable to other ASIO output channel ranges. ASIO Buffering and ASIO Samplerate can each either follow the driver's own default, or be forced to a specific value (the driver may refuse a forced value). The selected ASIO device's own Control Panel can be opened by clicking its name in this dialog.
- **Getting Optimal Latency, guidance** (p.81): output A1's choice is described as critical, since it sets the master samplerate and main buffer size; recommends ASIO first, then WDM or KS. Reducible down to 256-sample buffers on WDM/KS for lower latency. WDM's SWIFT mode can reduce it further "but not recommended because might be unstable" (consistent with the deprecation note above).
- **Virtual I/O latency, separately tunable** (p.81): can also be optimized via the separate VB-CABLE Control Panel application, by decreasing the VAIO driver's own internal latency -- but "can produce discontinued or non-working stream in some cases, pending on different buffering constraints."
- **Virtual ASIO driver's own latency contribution** (p.81): adds exactly one buffer's worth of latency to the global total, sized by output A1's own buffering setting.
- **LATENCY WARNING, verbatim, shown in bold red text** (p.81): "CHANGING DEFAULT LATENCY, BY REDUCING BUFFER SIZE CAN DECAY THE AUDIO STREAM, BRING UNSTABILITY, FREQUENT AUDIO CUT, STATIC, SYNCHRO LOST (ROBOTIC VOICE)." Recommends reverting to the default buffer size if this happens. Noted for context, not as a Loomix-actionable item: this vendor manual independently names the exact symptom class ("robotic voice", audio cuts, sync loss from buffer/timing problems) this project's own M8 investigation traced and fixed earlier this session -- confirming it as a well-known, generic real-time-audio failure mode industry-wide, not something unique to either codebase.

## M.I.D.I. Mapping (p.82-84)

- **Purpose** (p.82): connects a MIDI remote surface to control gain, mute, and solo of every strip and bus (with MIDI Feedback), plus a secondary "MIDI Extra Input Device."
- **Dialog fields** (p.82): M.I.D.I. Input Device, M.I.D.I. Map Name, Reset Map / Load M.I.D.I. Map / Save M.I.D.I. Map buttons, M.I.D.I. Output Device (feedback), M.I.D.I. Extra Input Device, VBAN M.I.D.I. input (ON/OFF), Refresh Controller button.
- **Mapping rows, exact labels visible (page 1 of 2 shown)** (p.82): Gain Fader Strip #1, Gain Fader Strip #2, Gain of Virtual Strip #3, **L/R PAN Strip #1**, **L/R PAN Strip #2**, **L/R PAN Virtual Strip #3**, PTT Mute Strip #1, PTT Mute Strip #2, Mute Virtual Strip #3, Solo Strip #1, Solo Strip #2, Solo Virtual Strip #3, M.C. Virtual Strip #3, Audibility Strip #1, Audibility Strip #2, EQ High Virtual Strip #3, EQ Med Virtual Strip #3, EQ Bass Virtual Strip #3, Gain of BUS A, Gain Virtual BUS B -- each row has a MIDI-code readout/dropdown, a Learn button, and an F/FF feedback-mode toggle.

  **Flagged as one of the most load-bearing findings in this manual for the pan-pot scope question, not resolved here:** "L/R PAN" is listed as a real, separately mappable control on **both hardware strips (#1, #2) and the virtual strip (#3)** in this exact dialog -- direct, primary-source confirmation that some form of pan control exists on the virtual strip too, not only on hardware strips. This corroborates the `Strip[i].Pan_x`/`Pan_y` Remote-API finding flagged on p.59-60 (a 2D X/Y pair, un-scoped to "Physical Strip Only" unlike `Color_x`/`Color_y`), and should be weighed together with it. Whether "L/R PAN" here is the same 2D `Pan_x`/`Pan_y` pair, a separate 1D scalar, or literally the position/3D pad under a different name, is not resolved by this page alone.
  - **"PTT" prefix on the Mute rows** (p.82): appears attached specifically to "Mute Strip #1" and "Mute Strip #2," described in prose as "un-mute the related strip when pushing the button, mute it when release it" -- Push-To-Talk semantics layered onto a mute-control MIDI mapping, though it's ambiguous from this page alone whether "PTT" is a distinct mapping mode/checkbox or just this screenshot's own row-naming choice; flagged, not resolved.
- **Learn workflow** (p.82): click Learn (or use TAB / up-down arrow to move between controls) and move the physical MIDI control; clicking the MIDI Code readout area resets that row's mapping.
- **RESET MAP / LOAD / SAVE** (p.82): reset the entire mapping; recall or save the whole Map to/from an XML file.
- **REFRESH CONTROLLER** (p.82): sends every current Voicemeeter value out as MIDI feedback in one go -- also separately available as its own MIDI-mappable function (detail expected in a later section).
- **M.I.D.I. Map Name** (p.82): a user-defined identifier, stored inside the Map's own XML file.
- **MIDI Feedback modes, F and FF** (p.83): **F** (Simple feedback) -- the controller receives feedback only when the mapped control changes via something other than that same controller (mouse, VBAN request, etc.); usually enough for ordinary knobs/faders. **FF** (Double feedback) -- needed for LED buttons (to reflect the actual Voicemeeter button state after a push) and some motorized faders that require positional acknowledgement (hardware-dependent).
- **MIDI Forward** (p.83): MIDI Mapping can receive from 2 distinct physical MIDI controllers plus an incoming VBAN-MIDI stream, simultaneously; everything received is also auto-forwarded to the MacroButtons application. Since March 2021, received MIDI can additionally be forwarded outward through an outgoing VBAN-MIDI stream, letting a MIDI controller attached to a remote computer work transparently over the network, feedback included.
- **MIDI Advanced Feedback** (p.84, since Nov 2022): lets a 2-position control define custom feedback values (default 127=ON, 0=OFF) instead of the fixed default, useful for controller LEDs using other color codes. Reached by right-clicking the 'F' button: "MIDI Feedback Options" dialog with Feedback Value ON, Feedback Value OFF, Feedback Message ON (HEXA, free-form -- supports full custom/SYS-EX messages, e.g. to address MIDI LCD or "MACHINE CONTROL" protocol devices), Feedback Message OFF (HEXA).
- **Duplicate-mapping warning** (p.84): a MIDI message shown in yellow in the mapping table means it's already used by another parameter; right-click it to see which one.

## Specifications (Standard edition, exact table) (p.85)

| Field | Value |
|---|---|
| Device Type | PC-Core Virtual Audio Mixing Console |
| Compatibility | Windows XP, VISTA, WIN7, WIN8, WIN10 (32/64 bits) |
| PC Configuration | Min: Celeron / Duo Core 1.8 GHz - 512 MB RAM - Disk < 100 MB |
| Number of Audio Device I/O | 3 Inputs (2 physicals / 1 Virtual); 3 Outputs (2 physicals / 1 Virtual) |
| BUS / Layer | 2x BUS (A and B) / Single Layer |
| Audio Engine Capabilities | 32, 44.1, 48, 88.2, 96, 176.4 or 192 kHz DSP Processing (defined by Output A1 configuration) |
| Output A1 (Main) | WDM, KS, MME, ASIO (32 kHz to 192 kHz) - 1 to 8 channels |
| Output A2 | WDM, KS, MME (8 kHz to 192 kHz) - 1 to 8 channels |
| 2x Physical Inputs | WDM, KS, MME (8 kHz to 192 kHz) - mono or stereo |
| 1x Virtual I/O | WDM, KS, MME, DirectX, WaveRT (8 kHz to 192 kHz) 1 to 8 channels; **8 channels on virtual input, 2 on virtual output** |
| 1x Virtual ASIO I/O | ASIO (32 kHz to 192 kHz) 8 Channels (in and out) / 4x Client Applications. Virtual ASIO configuration is given by Main Output A1 (SR and Buffering) |
| M.I.D.I. Implementation (remoting) | Gain faders, Mute, Solo, M.C., Audibility, 3 Bands EQ (Configuration by Learn process) |
| Strip Processing | "Color Panel" Control (Equalization); 3D Panoramic Control (source positioning by binaural effect); Audibility Knob (Compressor / Gate effect); 3 Bands Graphic Equalizer (on Virtual Input); Mute / Solo |
| BUS Processing | Integrated 0 dBFs Limiter and Peak Remover; Mix Down to convert 5.1 or 7.1 to Stereo; Stereo Repeat (Stereo signal copied on ch 3,4 / 5,6 / 7,8); Mute / Mono |
| Others | Physical Output Synchronization Delay in system settings dialog box |

**Note on internal inconsistency, flagged for accuracy, not a Loomix gap:** this table's own "M.I.D.I. Implementation (remoting)" list does *not* mention Pan/L-R-PAN at all, despite "L/R PAN Strip #1/#2/Virtual Strip #3" being real, visible rows in the actual M.I.D.I. Mapping dialog documented on p.82 two pages earlier -- this Specifications summary table appears to be an incomplete recap of the real dialog's contents, not a more-authoritative or narrower statement of scope. Don't treat this table's omission of Pan as evidence Pan isn't real or mappable; the dialog screenshot on p.82 is the more direct source.

**Precise confirmation of virtual-output channel asymmetry:** "8 channels on virtual input, 2 on virtual output" -- Bus B's *base* spec is 2 channels; the earlier-documented "3 additional routing modes" (Stereo Repeat, etc., p.26) are what let it actually carry up to 8 when needed, not a contradiction of that base spec.

## Voicemeeter I/O Diagram (p.86)

- Purely explanatory block diagram (Physical Audio Devices -> Voicemeeter Virtual Input -> Physical Audio Devices -> Virtual Output) with labeled jack numbers matching the topology already extracted (1-2 = Stereo Input #1, 3-4 = Stereo Input #2, 5-12 = Input #3's 8-channel virtual input, A1/A2 = hardware outputs, B = virtual output). No new controls; restates that Virtual I/O exposes both a Windows-native interface (MME/KS/WASAPI/DirectX) and a separate ASIO interface simultaneously, so ordinary Windows apps and ASIO-only pro DAWs can both connect.

## Licensing and activation mechanics (p.87-90)

Administrative/commercial mechanics, not audio-mixer features -- recorded briefly for completeness, clearly out of scope for a Loomix feature-parity audit:

- **Standard edition license model** (p.87): "Simple donationware" -- free to use with every function available except paid add-ons (like VAIO Extension), user encouraged (expected, for professional use) to pay via a donation with 5 selectable price tiers.
- **VAIO Extension license** (p.88): a separately purchased, paid feature unlocking the VAIO 1-5 extension slots (described earlier, p.32) for both Standard and Banana editions, via a machine-specific Challenge Code + a purchased Response Code tied to a registered e-mail.
- **License management details** (p.89): an option to hide the registered e-mail from view; a "Computer Footprint" export/import mechanism to transfer/restore activation across a Windows reinstall; an activation-history log file (`Voicemeeter_ActivationLog.dat`) that can be copied between installs.
- **Purchase flow** (p.90): "Buy Online" opens the VB-Audio webshop with the Challenge Code pre-filled; the VAIO Extension is explicitly marketed as "FOR VOICEMEETER STANDARD & BANANA ONLY," a fixed price plus 5 selectable "contribution" tiers, described as "a permanent license for a given PC configuration."
