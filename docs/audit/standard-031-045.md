# Voicemeeter Standard manual, pages 31-45

Source: ~/Documents/loomix-refs/Voicemeeter_UserManual.pdf ("VOICEMEETER Standard", Version 1.1.2.2, Dec 2025). Pages 34, 39, 42, and 44 are pure section-divider pages (a title and one-line subtitle only: "CASE STUDY #1 -- How to talk and send music in the same time on Skype?", "CASE STUDY #2 -- How to manage 2 headsets on Skype?", "CASE STUDY #3 -- How to record Conference-Call in 8 tracks for post production?", "VBAN -- VB-Audio Network"); no controls to extract from them.

## VAIO Latency (p.31)

- **Latency menu, right-click on Virtual Input caption** (p.31): opens a menu with "Show Control Panel..." at top, then a fixed list of selectable sample-count values: 384 (3x128), 768 (3x256), 1024, 1536 (3x512), 2048, 3072 (3x1024), 4096, 5120, 6144 (3x2048), **7168 (Default, checked in the example)**, 8192, 10240, 12288 (3x4096), 14336, 16384 -- the values from 8192 upward render greyed/disabled in this particular screenshot state.
- **Latency rule** (p.31): "VAIO must have an internal latency not below 3x time the buffer size used by Voicemeeter Audio Engine and the Applications connected to the Virtual I/O." 7168 samples is expected to work 100% of the time; 3072 or less can be tried to reduce latency.
- **Latency scope, linked input/output** (p.31): the same latency setting applies to the virtual input stream, any loopback stream, and the virtual output stream together -- "If you modify the latency for Voicemeeter input for example, it will be modified for the output B1 too" (not independently adjustable per direction).
- **Latency persistence** (p.31): stored as a system parameter, recalled automatically each time the audio engine starts.

## VAIO Control Panel (p.32)

- **VAIO Control Panel window** (p.32): a separate diagnostic/control window, shown for a specific selected virtual I/O ("I/O 6" in the example). Displays: driver name/version, max latency, internal sample rate, current latency (editable field), input/output driver statistics (DMA size, error counters, buffer calls, timer glitches, per-buffer-size counters), a Mode indicator ("Virtual I/O"), a Volume Control status ("Enabled"/"Disabled"), and per-channel Input Levels / Output Levels as percentages across all 8 channels (FL/FR/FC/LF/RL/RR/SL/SR).
- **Base VAIO mapping table** (p.32): "New Voicemeeter Audio driver is offering 8 couples of I/O where VAIO 1-5 are VAIO extension, and VAIO 6,7,8 are related to initial Voicemeeter Virtual inputs or B BUS":
  | Virtual Input | Voicemeeter input | Virtual Output | Voicemeeter Output |
  |---|---|---|---|
  | VAIO in 6 | Voicemeeter input | VAIO out 6 | Voicemeeter BUS B1 |
  | VAIO in 7 | unused | VAIO out 7 | unused |
  | VAIO in 8 | unused | VAIO out 8 | unused |
- **VAIO Extension mapping table, if licensed** (p.32): "If activated (by a specific license), other VAIOs can be used by physical input and output BUS":
  | Virtual Input | Voicemeeter input | Virtual Output | Voicemeeter Output |
  |---|---|---|---|
  | VAIO in 1 | Voicemeeter input #1 | VAIO out 1 | Voicemeeter BUS A1 |
  | VAIO in 2 | Voicemeeter input #2 | VAIO out 2 | unused |
  | VAIO in 3 | unused | VAIO out 3 | unused |
  | VAIO in 4 | unused | VAIO out 4 | unused |
  | VAIO in 5 | unused | VAIO out 5 | unused |
- **VB-Audio Devices Checker tool** (p.32): a separate diagnostic app listing all Voicemeeter-driver Playback Devices and Recording Devices side by side with sample rate/bit depth/channel count each: "Voicemeeter In 1" through "In 5", "Voicemeeter Input", "Voicemeeter AUX Input", "Voicemeeter VAIO3 Input" (playback side); "Voicemeeter Out A1" through "A6", "Out B1" through "B3" (recording side, at least 9 entries visible/implied).

## VAIO Loopback Mode / VAIO Volume Control (p.33)

- **VAIO Loopback Mode** (p.33): "normally activated by default. It allows capture applications like OBS to get audio from the Voicemeeter input directly." Toggled via the VAIO Control Panel's own Options menu.
- **VAIO Control Panel Options menu, exact items** (p.33): Reset Pin Name and Icon 1, Reset Pin Name and Icon 2, Enable Windows Volume Control (checkbox), Enable Loopback Streaming (checkbox, checked by default), Set Max Latency: 4096 / 5120 / 6144 / 7168 / 8192 / 12288 / 16384 / 32768 smp (each individually selectable, each "requires REBOOT"), Internal Sampling Rate: 44100 / 48000 / 88200 / 96000 / 176400 / 192000 Hz (selectable list), Reset settings to initial state.
- **Reset Pin Name and Icon** (p.33): resets a given Voicemeeter virtual I/O's Windows-visible device name and icon to default, useful if it was changed or is set incorrectly.
- **Administrator mode requirement** (p.33): changing VAIO Control Panel options requires running the application as administrator, since it needs system registry access.
- **Enable Windows Volume Control** (p.33): off by default -- by default, the Windows OS volume control has no effect on any Voicemeeter VAIO input (playback device); enabling this option makes the OS volume slider work for all Voicemeeter inputs when one is set as the Windows default playback device.
- **No output gain management** (p.33): explicitly stated -- "Voicemeeter outputs provide no gain management," i.e. the Windows-volume-control feature above only ever applies to virtual inputs, never outputs.

## Case studies -- new facts beyond controls already documented in earlier chunks (p.35-43)

These three worked tutorials mostly reuse controls already extracted (per-strip A/B routing, Composite bus mode). New, distinct facts only:

- **VB-Audio Virtual Cable, a separate companion product** (p.35-36): Case Study #1 requires installing "VB-Audio Virtual Cable" -- a distinct driver product from Voicemeeter's own VAIO, downloaded separately from vb-cable.com. Installing it creates one new playback device ("CABLE Input") and one new recording device ("CABLE Output"): "like every cable, all sounds sent to cable input will go on cable output." Used here to route Skype's own output back into a Voicemeeter hardware input strip.
- **Two independent physical outputs from the same bus** (p.40-41, Case Study #2): confirms A1 and A2 can each be assigned a different real output device simultaneously (two separate headsets), both carrying the same Bus A content -- two independent physical taps off one bus, not two separate buses.
- **Composite mode's exact Standard-edition layout, applied practically** (p.42-43, Case Study #3): connecting a DAW (REAPER, in the worked example) to Voicemeeter's Virtual ASIO input records the bus's 8 Composite channels as 4 stereo tracks: Track 1 = the bus's regular stereo output, Track 2 = Input #1 (pre-fader), Track 3 = Input #2 (pre-fader), Track 4 = Virtual Input #3 (pre-fader) -- a concrete instance of the layout already extracted on p.26, confirming it end to end against a real recording app.
- **Recorder-app feedback warning, repeated** (p.43): "Be careful to disable input monitoring in your recorder application to avoid feedback loop (prevent signal to go again into Voicemeeter virtual input)."

## VBAN: VB-Audio Network, introduction (p.45)

- **VBAN protocol** (p.45): a simple UDP-based protocol for real-time digital audio transport over an IP-based **local network** (explicit: "any computers on a local network," not the wider internet directly).
- **VBAN-Talkie / VBAN-Receptor** (p.45): separate mobile applications (iOS/Android) implementing the VBAN protocol to send/receive audio streams to/from Voicemeeter.
- **Protocol openness** (p.45): the PCM native audio protocol, VBAN-TEXT protocol, and VBAN-MIDI protocol are all public and documented (spec available on VB-Audio's support page); numerous third-party GitHub projects implement them independently.
- **Typical topology, per the diagram** (p.45): a "Stage Box" Voicemeeter instance and a "Studio / Broadcast Room" Voicemeeter instance connected bidirectionally over VBAN ("Inter-phony"), each also bridging to VOIP apps (Zoom, Twitch, Skype, Discord) and VBAN-Talkie/Receptor mobile clients -- captioned "Voicemeeter as Audio HUB, Stage Box, VOIP insert...".
- **VBAN Configuration dialog** (p.45): opened via a VBAN icon; configures both the incoming stream (audio received from another computer) and the outgoing stream (audio sent to other computers) -- full field-level detail expected on the following pages (next chunk).
