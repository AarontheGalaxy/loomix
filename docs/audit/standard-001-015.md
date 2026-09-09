# Voicemeeter Standard manual, pages 1-15

Source: ~/Documents/loomix-refs/Voicemeeter_UserManual.pdf ("VOICEMEETER Standard", Version 1.1.2.2, Dec 2025). Pages 4-7 are the Table of Contents only (navigational, no feature content to extract; listed section names are covered as those pages are reached in later chunks). Page 1 is a cover page (product name, tagline "Real Time Virtual Audio Mixer for Windows", version number) with no distinct controls.

## Topology (General)

- **Standard edition I/O topology** (p.8): 3 audio inputs (2 physical hardware strips, 1 virtual strip) and 3 audio outputs (2 physical, 1 virtual), routed through exactly 2 buses, A and B. Diagram caption: "Voicemeeter General Diagram (3 inputs / 2 Buses Mixing Console)". This is materially smaller than the 8-strip/8-bus Potato topology `docs/SPEC.md` is based on.
- **Bus A / Bus B roles** (p.8): text suggests typical usage is Bus A for monitoring (speakers) and Bus B for VOIP or audio recording applications, but this is a suggested convention, not an enforced restriction.
- **Virtual Input (IN 3)** (p.8-9): the virtual input point ("VAIO") is 8 channels wide even though the mixing console itself only names 3 top-level inputs -- multi-channel capacity exists "under" the 3-input console view.
- **Virtual Output (Bus B / VAIO)** (p.9): also supports up to 4 simultaneous ASIO client applications reading from the 8-channel virtual ASIO output.
- **Audio interface types supported** (p.9), table: MME (universal, ~100ms latency), WDM (via WASAPI, <30ms, best performance, available since Vista), KS (Kernel Streaming, low latency, since XP, not all devices support it), WaveRT (Vista+, good performance/low latency, comparable to KS), Direct-X (used by games/some audio software, latency comparable to MME), ASIO (Steinberg protocol, low latency, high fidelity).
- **VBAN network audio** (p.10): send/receive audio streams to/from other computers on a local network. Mentioned here as a headline capability; full detail expected in a later chunk (ToC p.5 lists a dedicated "VBAN: VB-Audio Network" section starting p.45).
- **Voicemeeter Remote API** (p.10): a DLL-based API (`VoicemeeterRemote.dll` / `VoicemeeterRemote64.dll`) for external client applications to control Voicemeeter programmatically. Detailed section expected later (ToC p.6 lists "Voicemeeter Remote API (for developer only)" at p.77).
- **Macro Buttons application** (p.10): a separate, bundled application installed alongside Voicemeeter, offering user-programmable buttons driven by a request script. Example button labels shown in a screenshot: "PTT" (Push To Talk), "FX Voice" (Robotic), "MUTE ALL", "RESTART" -- these read as illustrative examples, not fixed/default buttons.
- **Internal clock fallback** (p.13): "Voicemeeter can now work without any audio device by running on its own internal clock (if no device is selected on output A1)."
- **Main output device (A1) sample rate range** (p.13): 32 kHz, 44.1 kHz, 48 kHz, 88.2 kHz, 96 kHz, 176.4 kHz, or 192 kHz. This becomes the master sample rate for the whole mixing process; Voicemeeter resamples other inputs/outputs running at different rates.
- **Windows "Listen to this device" caveat** (p.14): documented as needing to be OFF, since it can disturb Voicemeeter's own routing -- a Windows-level setting, not a Voicemeeter control, but explicitly called out as an interaction to manage.
- **Real-time input monitoring** (p.15): selecting an input device on a hardware strip immediately allows hearing it in real time through whatever output the strip is routed to (A/B) -- described as inherent behavior once a device and a bus assignment are set, not a separate toggle on this page.

## STEP 0: Quick Startup / System Tray menu (p.12)

Right-click menu on the Voicemeeter system tray icon, exact items shown in the screenshot:
- **Restart Audio Engine** (p.12): menu action.
- **Load Settings...** (p.12): menu action, loads a saved settings file.
- **Save Settings...** (p.12): menu action, saves current settings to a file.
- **Reset Settings (Re-Initialization)** (p.12): menu action.
- **System Tray (Run at Startup)** (p.12): checkbox toggle, shown checked in the example.
- **Show App on Startup** (p.12): checkbox toggle.
- **MacroButtons: Run on Voicemeeter start** (p.12): checkbox toggle -- launches the separate Macro Buttons application automatically alongside Voicemeeter.
- **Always Visible** (p.12): checkbox toggle (keep-on-top behavior implied).
- **Hook Volume Keys (for Level Output A1)** (p.12): checkbox toggle -- binds the keyboard's hardware volume keys to control bus A1's level.
- **Hook Volume Keys (for Level Input A1)** (p.12): checkbox toggle -- binds the keyboard's hardware volume keys to control input strip 1's level.

## STEP 1: Output device selection (p.13-14)

- **A1 output device button** (p.13): clicking the "A1" button on the master section opens the output device selector for bus A1's physical device.
- **Device selector: interface tabs** (p.13): devices are grouped into four tabs -- WDM (WASAPI), KS (Kernel Streaming), MME (Multimedia), ASIO (Steinberg) -- each listing devices alphabetically with an icon, device name, and its native sample rate/channel count (e.g. "Speakers, High Definition Audio Device, 48000 Hz, 2 Ch").
- **Open Windows Sound button** (p.13, p.14): opens the Windows Sound control panel dialog directly from the device selector, to adjust device sound format (sample rate/bit depth) or other system audio settings.
- **Remove Selection button** (p.13): device selector dialog button, clears the current device assignment.
- **Exit / Cancel button** (p.13): device selector dialog button, closes without changing selection.
- **Recommendation** (p.13): manual recommends selecting ASIO first if available, otherwise WDM, for best latency; WDM/KS playback devices run in exclusive mode by default, bypassing the Windows mixer and its volume control.

## STEP 2: Input device selection (p.15)

- **Per-strip input device selector** (p.15): clicking the "Stereo input area" on a hardware strip's header opens a device selector scoped to that strip, with the same WDM/KS/MME/ASIO tab layout, "Open Windows Sound", "Remove Selection", and "Exit / Cancel" controls as the output selector.
- **Driver recommendation** (p.15): WDM recommended over MME for latency/performance; MME only if WDM is unavailable or misbehaves with the specific hardware.

## Strip/Bus controls visible in diagrams, pages 8-11 (not yet formally introduced by name in prose on these pages -- listed here for completeness, expect fuller treatment in a later chunk)

- **Gain slider per input strip** (p.8): a vertical fader per strip feeding into bus A and bus B.
- **A / B bus-assign buttons per strip** (p.8, p.11): toggles routing a strip's output into bus A and/or bus B independently.
- **SOLO button per strip** (p.11): labeled "S" in the screenshot, isolates the strip for monitoring.
- **MUTE button per strip and per bus** (p.11): labeled "M".
- **Fader Gain, generic** (p.11): described as present on every strip and every bus, adjusting that strip/bus's volume.
- **MONO / STEREO indicator or toggle per hardware strip** (p.11): strip 1's screenshot shows "MONO" above an Intellipan "Color Panel", while strip 2's shows "STEREO" above a "3D Panel" -- the label differs per strip in this example (possibly reflecting each strip's actual input channel count and a correspondingly different Intellipan sub-mode, not necessarily a manual toggle; not enough text on this page to confirm which -- flagged for the classification pass to resolve against a later, fuller Intellipan section).
- **Intellipan panel per hardware strip** (p.9-11): a knob-based panel shown on both hardware strips in every screenshot on these pages; strip 1 shown with a "Color Panel" (single knob, example value 0.9, with an "fx echo" label nearby) and strip 2 with what's labeled a "3D Panel" in one screenshot (p.11) or the same "Color Panel" style in another (p.9-10) -- inconsistent between screenshots on this page range, likely different example states of the same control rather than different controls; full explanation expected later (ToC p.5: "INTELLIPAN 3D PANEL: The Binaural effect", p.22).
- **Audibility control per hardware strip** (p.9-11): a second knob next to Intellipan, example values 0.0 and -4.7 dB shown; full explanation expected later (ToC p.5: "Audibility control & equalizer", p.23).
- **3-band Equalizer on the virtual strip** (p.9-10): three knobs labeled Bass / Med / High, each showing a dB value (0.0 dB in the examples). Only shown on the virtual input strip in these screenshots, not the hardware strips.
- **Bus mode-looking buttons on the master section** (p.11): a screenshot of the master (BUS A & B) section shows buttons labeled "MIX down", "Stereo Repeat", and what appears to be "Comp osite" (partially obscured/wrapped in the description) next to the bus faders -- these labels match `docs/SPEC.md` section 1.6's bus-mode names (Mix Down, Stereo Repeat, Composite), which the spec currently frames as Potato-specific. Flagged explicitly: this page does not explain whether these are real, present controls in the Standard edition or a diagram artifact; needs confirmation against the fuller bus-mode section expected later in this or another manual before any classification judgement is made.
- **VBAN toggle button** (p.10): a button labeled "VBAN" in the top bar of the mixer window screenshot, next to "Menu".
- **Menu button** (p.10, p.12): top bar button opening the main application menu (system tray menu items above are reached this way too, per p.12's screenshot flow).
