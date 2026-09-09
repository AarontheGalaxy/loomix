# Voicemeeter Standard manual, pages 16-30

Source: ~/Documents/loomix-refs/Voicemeeter_UserManual.pdf ("VOICEMEETER Standard", Version 1.1.2.2, Dec 2025). No blank/decorative pages in this range.

## Monitor Input in Real Time (p.16)

- **Real-time input monitoring, default bus routing** (p.16): "Input signal is per default sent to both busses (A & B)" -- both A and B bus-assign buttons are ON by default for a hardware strip in this manual's walkthrough. Flagged for the classification pass: `docs/SPEC.md` section 1.3 currently states the default is "Bus assign ... Default A1 on" (singular). Possible manual-vs-manual or manual-vs-Potato-default disagreement -- needs checking against Potato's own manual before concluding either way.

## STEP 3: Virtual Input as default playback device (p.17-20)

- **Virtual Input as a real Windows playback device** (p.17): Strip #3 (the virtual input) is exposed to Windows as an ordinary playback device ("VoiceMeeter Input" / VB-Audio VoiceMeeter VAIO), selectable as any app's output, or set as the Windows system default so all system audio routes into it.
- **Virtual Input is multi-channel, up to 8 ch** (p.18): can carry e.g. 5.1 surround from a DVD player, if Windows' own Speaker Setup for the Voicemeeter playback device is configured to a multichannel layout (Windows dialog options seen: Mono, Stereo, Quadraphonic, 5.1 Surround, 7.1 Surround -- a Windows-level setting, not a Voicemeeter control).
- **Bus B meter channel-count limitation** (p.18): "BUS B is also multi-channel but level meter shows always 2 first channels only" -- an explicit, named UI limitation on the Bus B meter specifically (not Bus A).
- **Virtual Input / Virtual Output mapping to Windows devices** (p.19): the Voicemeeter *playback* device (as seen in Windows Sound) is what feeds the Virtual Input strip; Bus B's audio is sent out to a Voicemeeter *recording* device ("VoiceMeeter Output") that other apps can select as their microphone/input source. Explanatory diagram only, no new controls.
- **Extended Virtual I/O ("VAIOs") scheme, Windows 10+** (p.20): "Voicemeeter now offers up to 8 Virtual I/O. I/O 1 to 5 are VAIO extensions. Voicemeeter input is also the Virtual input 6." The base Voicemeeter Output is renamed "Voicemeeter out B1" (Virtual output 6) once this scheme is active. Windows device lists shown include "Voicemeeter AUX Input", "Voicemeeter in 1" through "Voicemeeter in 5", "Voicemeeter VAIO3 Input" (playback side) and "Voicemeeter Out A2" through "A5", "Out B1" through presumably "B3" (recording side). **VAIO Extensions require an additional license activation code** (p.20, confirmed again p.29) -- this is a distinct, separately-licensed extension mechanism layered on top of the base Standard topology, not the same thing as Potato's built-in 8-bus design; flagged for the classification pass to treat as a distinct capability, not assume it's just "Potato's routing under a different name."

## STEP 4: Send Mix Output to Skype (p.21)

- **Bus B as a VOIP microphone source** (p.21): worked example wiring Bus B's virtual output into Skype's Microphone device setting. Application-integration example, not a new Voicemeeter control.

## STEP 5: Audio controls -- Intellipan and Audibility (p.22-23)

- **INTELLIPAN Color Panel** (p.22): a 2D XY pad on hardware strips. Horizontal axis tilts bass (left) toward medium (right); vertical axis adds treble going up. Description: "Based on basic equalizer, this panel will allow you to change the color of your voice in a quick way. It gives a spectral identity to your voice by acting on 3 frequency bands **and a tiny reverb on the half top**." The reverb-on-the-upper-half detail is explicit and specific here.
- **INTELLIPAN 3D Panel / Binaural effect** (p.22): right-click on the Color Panel switches the same pad to this alternate mode. A 2D "Position" pad, stereo-only, positions audio sources spatially with "a simple room effect" to increase dialog intelligibility when multiple people talk at once. Labeled example states: Neutral Position, Pan to Left, Pan to Right, and a distinct "Psycho Acoustic Spatialization FX" sub-view showing explicit "Source Position", "distance", and "Listener" markers -- described as saving audio energy compared to a regular pan pot, which "could completely im remove the sound from left or right." This reads as more elaborate (a distance/room dimension, not just position) than a plain L/R binaural pan.
- **Audibility control** (p.23): single knob, present on both hardware input strips. "This single knob controls a compressor / gate allowing to boost your voice and manage noisy talk. It needs to be adjusted according microphone capabilities and sound environment." Numeric readout shown (examples: 0.0, 5.8).
- **Equalizer (virtual strip)** (p.23): 3 bands, Bass/Med/High, boost or cut in dB, described as acting on bass/medium/high(treble) frequency.
- **Universal reset gesture** (p.23): "Trick: All controls go back to default value if double click on it!" -- stated as applying to controls generally, not one specific control.
- **Windows Communications-tab caveat** (p.23): Windows' "Communications" settings tab (Mute all other sounds / Reduce by 80% / Reduce by 50% / Do nothing) must be set to "Do nothing," or Windows will auto-ddisturb other devices during VOIP calls and break Voicemeeter routing. Windows-level setting, documented as a required interaction, not a Voicemeeter control.
- **Built-in microphone caveat** (p.23): laptop built-in mic/speaker combos risk feedback loops and inconsistent behavior; manual recommends a USB headset or external microphone instead.

## Strip Menu (p.24)

- **Strip context menu, hardware strips** (p.24): right-click the INTELLIPAN label. Items: Reset Strip, Reset INTELLIPAN, Reset Audibility, Copy All (a checkable/exclusive selector alongside the two below, determining Paste's scope), Copy INTELLIPAN, Copy Audibility, Paste, Save All, Save INTELLIPAN, Save Audibility, Load...
- **Strip context menu, virtual strip** (p.24): click the EQUALIZER label (different trigger than hardware strips' right-click). Items: Reset Strip, Reset EQUALIZER, **Reset Pan Pot**, Copy All, Copy EQUALIZER, **Copy Pan Pot**, Paste, Save All, Save EQUALIZER, **Save Pan Pot**, Load... Flagged explicitly for the classification pass: this menu names a "Pan Pot" as one of the virtual strip's own parameter categories, alongside its Equalizer -- worth resolving whether this is literally the same balance/pan control (currently modeled as hardware-only in this codebase) under a shared name, or the manual's label for the 5.1 position pad shown elsewhere. Not resolved here; extraction only.
- **General strip-menu mechanism** (p.24): RESET (all strip parameters, or one named effect, to default), COPY/PASTE (copy either all parameters or one named effect from one strip to another), LOAD/SAVE (persist all-or-one-effect to/from an XML file, shareable between users/computers). Excludes: slider (fader), Solo/Mute, and bus assignment -- explicitly stated as not covered by this menu's Reset/Copy/Paste/Load/Save.

## STEP 6: ASIO client connections (p.25)

- **Virtual I/O ASIO capacity** (p.25): each Voicemeeter Virtual I/O point supports up to 4 simultaneous ASIO client applications.
- **ASIO client signal flow** (p.25): all 4 ASIO clients receive the identical signal, sourced from BUS B; all 4 clients' own outputs are mixed together back onto Virtual Input (strip 3), combined with whatever plain Windows-audio-interface (MME/WDM/etc.) sound is already arriving there.
- **Loop-back warning** (p.25): recording applications risk an infinite feedback loop unless outputs are muted or monitoring disabled.
- Third-party application settings shown in the worked examples (a DAW's ASIO device/driver selection, buffer size, active channel checkboxes, MIDI input selection; a soft-synth's audio device/sample-rate/buffer-size fields) are external-application configuration, not Voicemeeter's own controls -- noted for completeness, not treated as a Voicemeeter feature.

## Special Routing Options on Output BUS (p.26)

Manual's own framing: "Voicemeeter provides **3 additional routing modes** for each Busses A (Physical) & B (Virtual)" -- named as exactly three modes in this edition, not `docs/SPEC.md`'s full 12-mode Potato set.

- **MIX DOWN** (p.26): one button (not a separate A/B variant). Makes a stereo mix-down from 5.1/7.1 sound arriving on the virtual input (strip 3): "Left and right channels, Center, Sub and rear are combined to output on stereo speakers."
- **STEREO REPEAT** (p.26): repeats a stereo signal across channel pairs 3/4, 5/6, and 7/8 of the bus's 8 available channels.
- **COMPOSITE** (p.26): "made for audio post production." Exact, fully specified layout for this edition's 8 bus channels: ch1,2 = the bus's usual stereo output; ch3,4 = Voicemeeter input #1 **before** its gain fader; ch5,6 = Voicemeeter input #2 **before** its gain fader; ch7,8 = Virtual input channel 1,2 **before** its gain fader. Use case given: recording all Voicemeeter inputs (each in stereo) via a DAW connected over Virtual ASIO, for VOIP interview/conference post-production with the 3 separate stereo tracks. This concrete layout is specific to the Standard edition's 3-strip topology and won't transfer numerically to a larger edition, but the mechanism (pre-fader taps composited onto extra channel pairs) is the same shape `docs/SPEC.md` section 1.6 already documents for Composite mode.
- **MONO (bus)** (p.26): "simply merges channel 1 & 2 to make mono signal in both channel 1 & 2. Made for single speaker monitoring." A per-bus control, distinct from the per-strip Mono button mentioned in the earlier chunk.
- **M.C. "Mute Center"** (p.26): on the Virtual Input strip specifically. "Made to mute dialog on DVD played in multichannel mode like 5.1 or 7.1. It allows over dubbing your favorite movies for example." Concrete use case: muting a movie's center/dialog channel for dubbing work.

## ASIO Routing Capabilities (p.27)

- **Direct physical-input/bus-to-ASIO-channel patching** (p.27): with Voicemeeter 1.0.5.0 / 2.0.3.0 or later, every physical input and every bus can be routed to any of up to 64 I/O on the ASIO driver selected as output A1 -- "the optimal way to use Voicemeeter with a professional audio board." Enabled by selecting "no device" for a given physical input/bus, then assigning its ASIO channel(s) explicitly in the System Settings dialog (example fields seen: "PATCH ASIO Inputs to Strips: IN 1 [1][2], IN 2 [3][4]"; "PATCH BUS TO A1 ASIO Outputs: [63][64]").
- **BUS A1's default ASIO channels** (p.27): always uses the first 8 output channels of the selected ASIO driver by default.
- **System Settings dialog fields visible in this example** (p.27): Buffering MME (samples, default 1024), Buffering WDM (samples, default 512), Buffering KS (samples, default 512), Buffering ASIO (dropdown, "Default" = the ASIO driver's own preferred size), Virtual ASIO Type (dropdown, e.g. "Float32LSB"), WDM Input Exclusive Mode (Yes/No dropdown), Preferred Main SampleRate (numeric, e.g. 44100 Hz), Engine Mode (dropdown, e.g. "Normal"), Monitoring Synchro Delay per output (OUT A1 / OUT A2, each in ms).
- **ASIO channel-overlap warning** (p.27): bus outputs are copied into ASIO output channels in logical order A1, A2, A3...; if a later bus's assigned channel range overlaps an earlier one, the earlier bus's audio is silently overwritten (e.g. A2 routed to channels 1+2 replaces A1's normal channels 1+2 there).

## Voicemeeter Main Menu (p.28-29)

Exact item list from the "Menu" dropdown screenshot, top to bottom, with keyboard shortcuts where shown:

- Restart Audio Engine `[CTL+R]`
- Auto Restart Audio Engine (A1 Device) -- checkbox
- Auto Restart Audio Engine (All Device) -- checkbox
- Load Settings...
- Load Settings on Startup: (shows the configured filename, e.g. "VoiceMeeterDefault.xml"; greyed out if none set)
- Save Settings...
- Reset Settings (Re-Initialization)
- System Tray -- checkbox
- Run on Windows Startup -- checkbox
- Show App On Startup -- checkbox
- Set as Always Visible -- checkbox
- Lock Graphic User Interface -- checkbox (prevents changing settings once locked)
- Run MacroButtons on Voicemeeter start -- checkbox
- Run Streamer View on Voicemeeter start -- checkbox
- Run other applications tools... -- submenu (other bundled/installed tools)
- Shortcut Key (Hook)... -- submenu (binds keyboard volume-hook keys to A1, A2, or Strip #1 level)
- Limit Remote Gain to 0 dBFs -- checkbox (caps MIDI/remote-control gain at 0dB instead of the normal +12dB ceiling)
- Show Contextual Help In Caption -- checkbox, checked by default in the screenshot
- Show Preset Scene On Startup -- checkbox
- Preset Scene... `[CTRL+4]`
- System Settings... `[CTRL+',']`
- M.I.D.I. Mapping... `[CTRL+3]`
- VBAN Options... (VB-Audio Network) `[CTRL+1]`
- VBAN Chat Room... (VB-Audio Network) `[CTRL+2]`
- Shut Down Voicemeeter
- About Box / License...
- VAIO Extension License...
- Online Help / Support...
- Check Driver Installation... -- runs `VBDeviceCheck.exe`, a diagnostic verifying the driver install wasn't corrupted by a Windows Update, run automatically on every launch too (per p.29 prose)

Prose additions (p.29): VAIO Extension License dialog appears on Win10 64-bit+ once the new VAIO driver is installed, for activating 1-2 extra virtual I/O via a purchased activation code. System Settings dialog configures audio-device-management parameters generally. M.I.D.I. Mapping dialog connects a MIDI control surface to Voicemeeter. VBAN dialog configures sending/receiving audio streams to/from other computers on the local network.

## Other Voicemeeter Tools & Accessories (p.29)

Three separate, standalone bundled applications (installed alongside Voicemeeter, not part of the main mixer window):

- **8x8 Gain Matrix** (p.29): standalone app, one gain matrix (percentage per cell) applicable to a selected bus, tabs shown for BUS A1-A5 and B1-B3 even in this Standard-edition screenshot (implying the underlying engine addresses that many buses regardless of what the console UI itself surfaces). Built as a Remote-API-SDK example app; redistributes audio across multichannel speaker systems.
- **15 Bands graphic EQ** (p.29): standalone app, a 15-band graphic EQ applicable to a selected bus, described as useful for tuning a PA system (stereo only).
- **VBAN2MIDI** (p.29): standalone app converting a physical MIDI input stream to a VBAN-MIDI network stream and back. Fields: MIDI Input Device, MIDI Output Device, Stream Name, IP Address To/From, UDP Port. Warning: two running instances cannot share the same UDP port.

## Preset Scene (p.30)

- **Preset count and storage** (p.30): 64 presets per edition, auto-stored under "My Documents\Voicemeeter\Scene_Standard" (or "Scene_Banana"/"Scene_Potato" for the other editions).
- **Preset Scene window** (p.30): an independent, resizable window listing numbered preset slots, each showing either a name+comment or, if uncommented, the creation date/time.
- **Recall** (p.30): press ENTER, or CTRL+Click a slot. Keys F1-F24 recall the first 24 presets directly.
- **Right-click context menu on a preset slot** (p.30): Recall Preset `[CTRL+CLICK]`, Store Preset, Edit Name, Edit Comment, Copy Preset, Paste Preset, Load Preset..., Save Preset As..., Delete Preset.
- **Preset scope, explicit** (p.30): "Preset contains all mixing settings. All audio controls in main Voicemeeter graphic user interface and audio fx." Explicitly excluded: "device selection, system settings, MIDI or VBAN parameters."
