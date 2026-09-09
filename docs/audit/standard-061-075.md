# Voicemeeter Standard manual, pages 61-75

Source: ~/Documents/loomix-refs/Voicemeeter_UserManual.pdf ("VOICEMEETER Standard", Version 1.1.2.2, Dec 2025). No blank/decorative pages in this range.

## Bus Audio Devices table, completed from p.60 (p.61)

- `Bus[i].device.asio`: Device Name (String), "Write only" -- completes the table begun at the bottom of p.60 (`.wdm`, `.ks`, `.mme`, `.asio` -- all four write-only, physical bus only).

## Special functions to make timed fade in/out (p.61)

- **`Strip().FadeTo` / `Bus().FadeTo`** (p.61): sets a gain slider with a progressive fade to a target dB value over a given time in ms (0 to 120000). Parameter is a string carrying both values. Examples: `Strip(0).FadeTo=(-10.0, 500);` (fade to -10dB over 500ms), `Strip(0).FadeTo=(-20.0, 2000);` (fade to -20dB over 2s), `Bus(0).FadeTo=(0.0, 1500);` (fade to 0dB over 1.5s).
- **`FadeBy`** (p.61): same remark applies -- a relative-change version of FadeTo (signature implied identical shape).

## System Settings Option -- Patch Options (p.61)

| Parameter | Value range | Remark | API ver. |
|---|---|---|---|
| `Patch.asio[i]` | 0 to ASIO input | ASIO Patch | 1 |
| `Patch.composite[j]` | 0 to 22 (1 = first channel) | 0 = default BUS | 2 |
| `Patch insert[k]` | 0 (off) or 1 (on) | Virtual ASIO insert | 2 |
| `Patch.PostFaderComposite` | 0 (PRE) or 1 (POST) | COMPOSITE Mode | 2 |
| `Patch.PostFxInsert` | 0 (PRE) or 1 (POST) | Virtual INSERT Point | 2 |

`i` = input channel zero-based index (physical strips only, 2 channels per strip). `j` = composite channel zero-based index (0 to 7; "COMPOSITE mode is maed [sic] of 8 channels"). `k` = input channel zero-based index (0 to 21).

## System Settings Option -- System Settings (p.61)

| Parameter | Value range | Remark | API ver. |
|---|---|---|---|
| `Option.sr` | 44.1, 48, 88.2, 96, 176.4, or 192 kHz | Preferred samplerate | 1 |
| `Option.ASIOsr` | 0: default ASIO Samplerate; 1: preferred samplerate | For ASIO driver connected on output A1 | 1 |
| `Option.delay[i]` | 0 to 500ms max | BUS output delay | 1 |
| `Option.buffer.mme` | 128 to 2048 | MME buffer size | 1 |
| `Option.buffer.wdm` | 128 to 2048 | WDM buffer size | 1 |
| `Option.buffer.ks` | 128 to 2048 | KS buffer size | 1 |
| `Option.buffer.asio` | 128 to 2048 | ASIO Buffer Size | 1 |
| `Option.mode.exclusif` | 0 (off) or 1 (on) | WDM input exclusive | 1 |
| `Option.mode.swift` | 0 (off) or 1 (on) | WDM swift mode | 1 |

`i` = output zero-based index (physical bus only).

## Special Commands (p.62)

Commands trigger an *action*, rather than changing a parameter value. "Write only" by nature.

| Command | Value range | Remark | API ver. |
|---|---|---|---|
| `Command.Shutdown` | 1 (unused) | Shutdown Voicemeeter | 1 |
| `Command.Show` | 1 (unused) | Show Voicemeeter | 1 |
| `Command.Restart` | 1 (unused) | Restart Audio Engine | 1 |
| `Command.Reset` | 1 (unused) | Reset All configuration | 1 |
| `Command.Save` | String | Complete filename (xml) | 1 |
| `Command.Load` | String | Complete filename (xml) | 1 |
| `Command.Lock` | 0 or 1 | (Un)Lock GUI (Menu option) | 1 |
| `Command.Button[i].State` | 0 or 1 | Change Macro Button State | 1 |
| `Command.Button[i].StateOnly` | 0 or 1 | Change Button State only | 1 |
| `Command.Button[i].Trigger` | 0 or 1 | Change Trigger Enable State | 1 |
| `Command.Button[i].Color` | 0 to 8 | Change the Button Color | 1 |
| `Command.DialogShow.VBANCHAT` | 0 or 1 | Show VBAN-Chat Dialog | 1 |
| `Command.SaveBUSEQ[j]` | String | Complete filename (xml) | 2 |
| `Command.LoadBUSEQ[j]` | String | Complete filename (xml) | 2 |
| `Command.Preset[k].Recall` | 1 (unused) | Recall Preset Scene | 1 |
| `Command.Preset[k].Store` | String | Update Preset Scene (with a new name if the string is not empty) | 1 |
| `Command.RecallPreset` | String | Recall Preset by Name (recalls current preset if string is empty) | 1 |
| `Command.UpdatePreset` | String | Update Preset by Name (updates current preset if string is empty) | (unspecified) |

`i` = MacroButton ID (zero-based index). `j` = BUS index (zero-based). `k` = preset index implied.

- **Command priority/ordering rule** (p.62): "command requests are prior to other requests. It means other type of request could not be processed if in the same request packet than a command request." Example: a Shutdown request in a packet simply closes the program without processing anything queued after it; a Load request resets all possible previous or next requests present in the same packet.
- **Warning** (p.62): "Command.Button" must be used in a VBAN-TEXT request only (see next section on p.73-75).
- **Button command for button interactions** (p.62): changes another button's state directly, or emulates a PUSH/RELEASE, via `Button(i).State`. Examples: `Button(5).State=1;` (PUSH button ID 5), `Button(5).State=0;` (RELEASE button ID 5), `Button(5).StateOnly=1;` (set button ID 5's visual state to pushed without firing its trigger).

## Exclusive Commands -- local to MacroButtons, not sent to Voicemeeter (p.63)

- **`Button.State = 0;` in an INIT script only** (p.63): changes the state of the *current* button (only valid there).
- **`Button(5).Trigger = 1/0;`** (p.63): enables/disables the audio trigger option on a given button.
- **Wait command, sequencing** (p.63, added September 2019): `Wait(ms);` introduces a timed pause between requests inside one button's script, to build a sequence. Example given chains `Strip(0).gain=-12.0;`, `Wait(2000);`, `Strip(0).gain=0.0;`, `Wait(1000);`, `Strip(0).FadeTo=(-10.0, 1000);`, `Wait(1000);`, `Strip(0).FadeTo=(0.0, 1000);`.
- **Load Button map** (p.63, added "Mars 2020" [March 2020] version): `Load("filename");` loads another MacroButtons config file (a "Button Map," previously saved via the app's own Save function), from inside a button's own script.
- **Show command** (p.63, added October 2021 version): `Show(0);` hides the MacroButtons application window; `Show(1);` shows it again. Explicit warning: remember to implement a corresponding `Show(1)` somewhere, or the window stays hidden with no way back through the UI.

## VBAN Options, Remote API (p.64)

Full table, `i` = zero-based index (0 to 7):

| Parameter | Value range | Remark | API ver. |
|---|---|---|---|
| `vban.Enable` | 0 (off) or 1 (on) | VBAN functions | 1 |
| `vban.instream[i].on` | 0 or 1 | Stream On/Off | 1 |
| `vban.instream[i].name` | String | Stream Name | 1 |
| `vban.instream[i].ip` | String | IP Address from | 1 |
| `vban.instream[i].port` | 16 bit range | PORT (Ethernet) | 1 |
| `vban.instream[i].sr` | 11025 to 96 kHz | Read only | 1 |
| `vban.instream[i].channel` | 1 to 8 | Read only | 1 |
| `vban.instream[i].bit` | VBAN data type | Read only | 1 |
| `vban.instream[i].quality` | 0 to 4 | 0 = Optimal | 1 |
| `vban.instream[i].route` | 0 to 8 | Strip Selector | 1 |
| `vban.outstream[i].on` | 0 or 1 | Stream On/Off | 1 |
| `vban.outstream[i].name` | String | Stream Name | 1 |
| `vban.outstream[i].ip` | String | IP Address To | 1 |
| `vban.outstream[i].port` | 16 bit range | PORT (Ethernet) | 1 |
| `vban.outstream[i].sr` | 11025 to 96 kHz | (no remark) | 1 |
| `vban.outstream[i].channel` | 1 to 8 | (no remark) | 1 |
| `vban.outstream[i].bit` | VBAN data type | 1 = 16 bits PCM | 1 |
| `vban.outstream[i].quality` | 0 to 4 | 0 = Optimal | 1 |
| `vban.outstream[i].route` | 0 to 8 (BUS selector); 0 to 4 (screen capture select); 0 to 7 (MIDI source select) | three overloaded meanings depending on stream type | 1 |
| `vban.outstream[i].vfps` | 1 to 30 | Video FPS | 1 |
| `vban.outstream[i].vFormat` | 1 (PNG) / 2 (JPG) | Video Image Format | 1 |
| `vban.outstream[i].vQuality` | 1 to 100% | Video JPG quality | 1 |
| `vban.outstream[i].vCursor` | 0 (off) or 1 (on) | Video Cursor Capture | 1 |

- **MIDI Source bit combination** (p.64): `vban.outstream[i].route` for a MIDI-source stream is a bitmask -- MIDI Input: `0x01`, MIDI Aux input: `0x02`, VBAN IN: `0x04`, MIDI OUT: `0x08`. Example: "ALL MIDI IN" = `0x01+0x02+0x04 = 7`.
- **Parameters that trigger an Audio Engine Restart when changed** (p.64): `vban.Enable`, `vban.instream[i].port`, `vban.instream[i].quality`, `vban.outstream[i].quality`.
- **VBAN SampleRate, full allowed list** (p.64): 11025, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000 Hz.

## VBAN Quality / Bit Resolution (p.65)

- **VBAN Quality, 5 levels** (p.65): 0 (Optimal), 1 (Fast), 2 (Medium), 3 (Slow), 4 (Very slow). Conditions the size of an internal stack (and therefore latency) to trade off against network-instability tolerance. Optimal assumes the network can transmit packets in real time with good regularity; Very slow assumes the network has timing problems and unexpected waiting cycles. More useful on the receiver side; the transmitter is expected to always run in Optimal mode.
- **VBAN Bit Resolution / data format** (p.65): allowed formats are 1 (16-bit PCM) or 2 (24-bit PCM).

## AUTO Ducking (Trigger) (p.65-66)

- **Trigger-driven auto ducking, worked example** (p.65): a button's Trigger IN script fades one strip (a music/virtual input) down and cuts its EQ mid-band when another strip's (microphone) level crosses the IN threshold; the Trigger OUT script fades it back up when level falls below the OUT threshold. Example scripts shown: ON/IN -- `Strip[3].FadeTo=(-15.0, 100); Strip[3].EQgain2=-12;`; OFF/OUT -- `Strip[3].fadeto=(0, 200); Strip[3].EQgain2=0;`.
- **IN / OUT threshold cursors** (p.66): green cursor = IN threshold (level to exceed to fire trigger-in); red cursor = OUT threshold (level to fall below to fire trigger-out).
- **HOLD parameter** (p.66): keeps the trigger open for this minimum time (ms) regardless of level, once fired.
- **Trigger source scope** (p.66): dependent on an input Strip level (`in #1` to `in #8`) **or** an output BUS level (`out#1` to `out#8`) -- confirms up to 8 addressable strips and 8 addressable buses are selectable as a trigger source in this dialog, regardless of how many the current edition's console UI itself displays.
- **Input Mode, 3 options when the trigger source is an input strip** (p.66): Pre-Fader, Post-Fader (pending on slider gain), Post-Mute (pending on mute button) -- confirms the Post-Fader/Post-Mute options only hinted at on p.57 do exist, alongside Pre-Fader.

## HID Manager (p.67)

- **HID Device Button configuration** (p.67): connects a MacroButtons button to any HID (Human Interface Device), for controllers/devices not covered by the Keyboard Shortcut list. Dialog fields: Select HID Device (dropdown of detected HID devices with hardware IDs), HID Name (editable friendly name), Status (Connected/Disconnected indicator), Control Code (e.g. "Bit 26 = 1"), HID Button Name (editable friendly name for the specific control/key), a live HID Current Data readout (raw HID report bytes when a button/key is pressed), and a "Learn" checkbox to auto-detect the Control Code from a physical key-press.
- **Regular keyboard/mouse via HID manager** (p.67): explicitly notable -- "you can use the HID manager to manage your regular keyboard and mouse as well. This is sometimes useful to manage special keys, not listed in Keyboard Shortcut List."

## GPIO (p.68)

- **GPI8-USB module** (p.68): an extra, separately purchased USB hardware module providing 16 GPI (general purpose input) lines, selectable via the GPIO combo box in a button's configuration.
- **DB9 connector** (p.68): the module's DB9 connector wires up to 8 simple electric switches, a foot pedal, or anything able to make an electric contact; a second DB9 connector handles lines 9-16.

## System Functions -- sending commands to Windows (p.69-71)

- **Concept** (p.69): "The Universal Programmable Buttons" can be triggered by mouse, one key or a combination (e.g. Shift+A), a MIDI event, a GamePad, an HID device, or a Voicemeeter input-level Trigger -- and can, in turn: send a command to Voicemeeter, send a Keyboard Event into the system queue (to remote a softphone or any hotkey-driven app), call/execute any program or application, send a MIDI code to up to 2 MIDI output devices, or send a MIDI code or Voicemeeter script through a VBAN stream.
- **System Command table** (p.69):

  | Function | Value type | Remark |
  |---|---|---|
  | `System.KeyDown("Key")` | String | |
  | `System.KeyUp("Key")` | String | |
  | `System.KeyPress("Key")` | String | sends Key Down + Key Up |
  | `System.SetFocus("ApplicationName")` | String | sets keyboard focus |
  | `System.ResetFocus()` | -- | resets keyboard focus |
  | `System.Mouse("Action")` | String | LBUTTONDOWN, LBUTTONUP, RBUTTONDOWN, RBUTTONUP, MBUTTONDOWN, MBUTTONUP (example: `System.Mouse("LBUTTONDOWN");`) |
  | `System.Execute(exe, dir, arg)` | Strings | |

  Explicit note: "these commands are not sent to Voicemeeter" -- handled entirely by the MacroButtons app itself.
- **`System.Execute`** (p.69-70): works like Windows' `CreateProcess`/`ShellExecute`, launching an application with a command-line argument. Examples given: opening a web page via Internet Explorer (`System.Execute("C:\Program Files\Internet Explorer\iexplore.exe", "", "-new www.voicemeeter.com");`); running the Windows WRITE editor with an environment-variable path (`%windir%\write.exe`, `%TMP%`); running a DOS command via `cmd.exe /K` (`ipconfig`, or `ping 192.168.1.1`).
- **Special-character escaping** (p.70): `%'` (percent + single quote) becomes a literal double quote `"`; doubling a percent (`%%`) becomes a single literal `%`.
- **Environment variables** (p.70): usable via `%envname%` syntax anywhere in an Execute call.
- **`/C` vs `/K` for cmd.exe** (p.70): `/C` runs the given command then terminates the window; `/K` runs it and leaves the window open.
- **`System.KeyDown` / `KeyUp` / `KeyPress`** (p.70): send a combination of 1 to 4 keys via a string like `"CTRL+SHIFT+F10"` or a single key like `"0"`. `KeyPress` sends both Down and Up in one call. Examples: `System.KeyDown("A");`, `System.KeyDown("SHIFT+T");`, `System.KeyDown("CTRL+NP1");`, `System.KeyDown("ALT+F8");`, and their `KeyUp`/`KeyPress` equivalents.
- **List of Key Names, full table** (p.71): Regular Keys (0-9, A-Z, BACK, TAB, RETURN, ESC, SPACE, PAGEUP, PAGEDOWN, END, HOME, LEFT, UP, RIGHT, DOWN, INSERT, DELETE); NUM PAD (NP0-NP9, NPMUL, NPADD, NPDOT, NPSUB, NPDEC, NPDIV, NUMLOCK, SCROLLLOCK, CAPSLOCK, PRINTSCREEN, PAUSE, CLEAR, SELECT, PRINT, PRINTSCREEN [again], HELP, APP, EXECUTE); Special Key (BROWSERBACK, BROWSERFORWARD, BROWSERREFRESH, BROWSERSTOP, BROWSERSEARCH, BROWSERFAV, BROWSERHOME, VOLUMEMUTE, VOLUMEDOWN, VOLUMEUP, MEDIANEXT, MEDIAPREV, MEDIASTOP, MEDIAPAUSE/MEDIAPLAY, LAUNCHMAIL, MEDIASELECT, LAUNCHAPP1, LAUNCHAPP2, PLAY); FUNCTION (SHIFT, CTRL, ALT, LWIN, RWIN, LSHIFT, RSHIFT, LCTRL, RCTRL, LMENU, RMENU, F1 to F12, F13 to F24).

## Send M.I.D.I. Message (p.71-72)

- **2 MIDI output devices** (p.71): MacroButtons (from v1.0.1.1) lets you select 2 independent MIDI output devices ("out1"/"out2") via the app's own system menu (MIDI OUT1 device / MIDI OUT2 device submenus, listing detected MIDI devices, e.g. "Microsoft GS Wavetable Synth", "Delta AP MIDI", "Launchpad", "Saffire 6USB2.0", "nanoKONTROL2", "- No Midi -").
- **`System.SendMidi`, 4 request types** (p.71-72), channel 1 to 16: `System.SendMidi("out1", "note-on", channel, note, velocity);`, `"note-off"`, `"ctrl-change"` (channel, ctrl, value), `"prg-change"` (channel, nPrg).
- **RAW DATA function** (p.72): `System.SendMidi("out1", "data", aa, bb, cc, ee, ff, gg, ...);` sends any raw MIDI message including sys-ex; values in this function only are hex (00 to FF), whereas the note-on/off/ctrl-change/prg-change functions take decimal values (0-127) as usual for MIDI.
- **`SendMidi` without `System.` prefix** (p.72, October 2020+): the `System.` prefix can be omitted -- `SendMidi("out1", "note-on", channel, note, velocity);` etc. work directly.
- **Worked button example** (p.72): a button whose Initial-State and Trigger-IN scripts send `note-on` messages plus a raw `"data"` sys-ex-style message, and whose Trigger-OUT sends corresponding `note-off` messages plus a different raw data payload -- also shows a TRIGGER section with a "Level Option" field including an "After Mute" checkbox (not previously seen), implying trigger evaluation can optionally ignore/respect the strip's current mute state.

## Send VBAN-MIDI or VBAN-TEXT / Voicemeeter script through VBAN (p.73-75)

- **VBAN-MIDI / VBAN-TEXT sending, version gate** (p.73): available from Voicemeeter 1.0.3.5 / 2.0.3.5. MacroButtons can send MIDI messages via VBAN-MIDI and full Voicemeeter script requests via VBAN-TEXT; MacroButtons can also *learn* MIDI codes arriving on an incoming VBAN-MIDI stream. A separate "MIDI2VBAN" application (also installed with Voicemeeter) converts physical MIDI I/O into a VBAN-MIDI stream.
- **MacroButtons controllable by Xbox controller** (p.73): explicitly illustrated, via the same GamePad Button mechanism documented earlier.
- **MacroButtons' own VBAN Configuration dialog** (p.73-74): reached via MacroButtons' own system menu (distinct from Voicemeeter's own VBAN dialog). Configures: VBAN-MIDI output Streams (2 slots: "vban1"/"vban2", each Stream Name/IP Address To/UDP port), VBAN-TEXT output Streams (4 slots: "vban1" through "vban4", each Stream Name/IP Address To/UDP port), VBAN-FRAME output Streams (GUI Capture) -- streaming the MacroButtons window itself to another device via VBAN-Frame, same mechanism as Voicemeeter's own screen sharing -- and an incoming VBAN-TEXT "Incoming Command (Mouse)" stream for receiving remote mouse control from a VBAN-Screen client.
- **Menu items visible in this dialog's parent menu** (p.74): Restart Engines, New Button Map, Load Button Map, Save Button Map, System Tray (Close = Hide), Run on Windows Startup, Show App On Startup, Always Visible, Store Last Buttons State, MIDI OUT1 device (submenu), MIDI OUT2 device (submenu), VBAN Configuration..., DMX Configuration..., GPIO Configuration..., Restart HID Manager, About / License..., Online Help / Support..., Shut Down Macro Buttons (Q). -- Notably lists a **DMX Configuration...** menu item, confirming DMX-512 lighting control (already flagged from the earlier single-pass audit) is a real, present MacroButtons capability, reachable from this exact menu.
- **Send MIDI command through a VBAN Stream** (p.74): once an outgoing VBAN-MIDI stream is configured, `System.SendMidi("vban1", "note-on"/"note-off"/"ctrl-change"/"prg-change", ...)` (or the RAW `"data"` variant) targets that VBAN stream by name instead of a physical MIDI device -- same 4 request types plus RAW DATA, just aimed at a VBAN target string.
- **Send Voicemeeter TEXT Request through a VBAN Stream** (p.74-75): once an outgoing VBAN-TEXT stream is configured, a full multi-line Voicemeeter script can be sent through it using a `BEGIN_SECTION("streamname") ... END_SECTION` block. Example:
  ```
  BEGIN_SECTION("vban1")
  Strip(0).mute=1; Strip(1).mute=1;
  Bus(0).gain= 0.0;
  END_SECTION
  ```
  A regular function-style syntax is also supported (to match the syntax used by the VBAN-Button Android app): `SendText("vban2", Strip(0).mute=1; Strip(1).mute=1; Bus(0).gain=0.0;);`, which can also be split across multiple lines -- the closing parenthesis ends the entire request regardless of line breaks.
