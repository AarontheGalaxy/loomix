# Voicemeeter Standard manual, pages 46-60

Source: ~/Documents/loomix-refs/Voicemeeter_UserManual.pdf ("VOICEMEETER Standard", Version 1.1.2.2, Dec 2025). Page 55 is a pure section-divider page ("MACRO-Buttons -- Voicemeeter Remote API"), no controls to extract. The Bus Audio Devices table starting at the bottom of p.60 is cut off mid-table by the page break; its remainder is picked up at the start of the next chunk (pages 61-75).

## VBAN Configuration dialog (p.46-47)

- **VBAN ON/OFF** (p.46): global enable/disable toggle for the VBAN service.
- **Global UDP port** (p.46): 6980 by default.
- **IP Host Address** (p.46): displays this machine's IP address(es), for up to the first 3 possible network adapters.
- **Main Stream Samplerate** (p.46): a single samplerate field for the whole VBAN session (44100 Hz shown).
- **Reset Config / Load Config / Save Config buttons** (p.46).
- **HostName / UserName** (p.46): this machine's own VBAN identification fields, shown live (e.g. "vbi3" / "Big Boss").
- **Incoming Streams table** (p.46, p.48): columns On (toggle), Stream Name, IP Address From, Port, Info, SampleRate, Ch, Format, Net Quality, Destination (e.g. "In #3", "In #5", "Virtual Input"), Errors (a row of blinking-red error-category LEDs).
- **Outgoing Streams table** (p.46, p.48): columns On (toggle), Source (e.g. "BUS A1", "BUS A4"), Stream Name, IP Address To, Port, SampleRate, Ch, Format, Net Quality, Errors.
- **Stream capacity** (p.46): "it is possible to listen to 4x streams (on any input) and send 4 streams to different computers or mobile device (audio source is given by BUS A or BUS B Source)" -- 4 incoming and 4 outgoing *audio* stream slots (separate from the dedicated MIDI/Command stream rows below).
- **Audio stream format range** (p.46): any standard samplerate from 11025 Hz to 96 kHz, 16 or 24 bit resolution, 1 to 8 channels (mono to 7.1).
- **Broadcast support** (p.46): a `.255` destination IP address (e.g. `192.168.1.255`) broadcasts to every device on that subnet; note such UDP broadcast typically can't cross a router or WiFi access point.
- **Serial and ASCII (Command) incoming streams** (p.46, p.48): beyond audio, VBAN also carries a Serial stream (for MIDI) and an ASCII stream (for text/request-script commands) to remote Voicemeeter -- each its own row in the Incoming Streams table.
- **VBAN-MIDI outgoing stream** (p.46): sources from Voicemeeter's own MIDI Mapping; can send all incoming MIDI, or one particular MIDI source (including MIDI output, for feedback), over the network -- pairs with the standalone VBAN2MIDI app for a network-attached MIDI controller.
- **VBAN-Ping identification** (p.47): an "i" indicator beside a stream's IP-Address field lights when the connection is validated by a VBAN-Ping handshake; right-click "i" for the remote unit's info (Hostname, Application, Language/Country, TimeStamp) and a "Resend VBAN-Ping..." action.
- **Hostname entry** (p.47): the IP-Address field on any stream also accepts a hostname in place of a numeric IP.
- **VBAN-Chat dialog** (p.47): opened by right-clicking the VBAN icon. Basic text chat between VBAN units; auto-populates with every IP address configured in the VBAN dialog plus any identified via VBAN-Ping. A sent TEXT message is auto-displayed by every connected unit's own Chat dialog. Its own Options menu: change display options and font size; can send special `<nudge>` and `<alert>` messages.

## Configuring a VBAN audio stream (p.48)

- **Stream identity** (p.48): a stream is uniquely defined by Stream Name + IP-Address-From (+ UDP port); receiving requires all three to match on both ends exactly.
- **Green LED pair per stream** (p.48): one lights when audio is actually being received, a separate one when audio is actually being sent -- distinct from the On/Off enable toggle.
- **Network Quality parameter** (p.48): Fast (no built-in delay/error tolerance, needs a good network) vs. Slow (tolerates delayed or lost packets, for a busy network) -- a per-stream setting, most relevant on incoming streams.
- **Error LED categories, exactly 5** (p.48): 1 Overload (audio arriving faster than expected), 2 Corrupt (corrupted packets received), 3 Disorder (packets received out of the original order), 4 Missing (packets lost), 5 Underrun (not enough packets received -- stream too slow or stopped). Recommendation: adjust Network Quality toward Fast or Slow if overload/underrun errors keep appearing.

## VBAN-MIDI / VBAN-Command streams (p.49-50)

- **No fixed source address for MIDI/Command streams** (p.49): unlike audio streams, the Serial (MIDI) and ASCII (Command) incoming stream rows don't require a matching "IP Address From" -- left blank, they accept a message from any sender ("No address = any").
- **Live message readouts** (p.49): the Incoming Streams table shows the last received TXT message and last received MIDI message inline, per stream row.
- **VBAN2MIDI standalone application** (p.49): installed alongside Voicemeeter. Converts a physical MIDI input device into an outgoing VBAN-MIDI stream, and conversely an incoming VBAN-MIDI stream into a physical MIDI output. Fields: MIDI Input Device, MIDI Output Device, Outgoing VBAN Stream (Stream Name, IP-Address To, UDP Port, On), Incoming VBAN Stream (Stream Name, IP-Address From, On), an MTC timecode readout. Warning (repeated from p.49's earlier general VBAN note): two running instances of an app using the same UDP port will conflict.
- **Documented product-manual naming inconsistency** (p.50): the manual itself flags that Voicemeeter's own MIDI Mapping dialog box labels a field "VBAN MIDI Input" when it actually reflects the VBAN MIDI *outgoing* stream, and "should be labeled 'VBAN MIDI Output' instead" -- a naming bug the vendor manual calls out about their own product, not a Loomix-relevant control by itself, recorded for completeness only.

## VBAN-Frame: screen sharing (p.51-54)

- **Capture source options** (p.51): "App View" (Voicemeeter's own main window only) or Display 1 through Display 4 (any of up to 4 connected physical displays, up to 4K resolution).
- **Frame rate options** (p.51): a fixed dropdown list -- 1, 2, 3, 5, 10, 20, 24, 25, 30 FPS.
- **Image format** (p.51): PNG or JPG. JPG adds a Quality percentage field (1-100%). A separate "Capture Mouse Cursor" checkbox.
- **Bandwidth field** (p.51): shown as e.g. "12 mbps" -- described as a soft, theoretical target that prioritizes the audio stream, not a hard cap (can exceed 30 Mbps if actually needed).
- **Remote mouse control via VBAN-Text** (p.51): if a VBAN-Screen client is displaying the stream, Voicemeeter can receive mouse events back from it over an incoming VBAN-TEXT stream, letting the sending machine be controlled remotely. Enabled by right-clicking the samplerate/format field on that incoming VBAN-TEXT stream row and toggling "Manage Mouse command"; an "M" indicator appears in the table once active.
- **Same-machine streaming** (p.52): destination `127.0.0.1` streams to VBAN-Screen running on the same PC; use a different UDP port if the sender and VBAN-Screen share a machine. PNG recommended to preserve full image quality for App View capture.
- **Capture works while hidden** (p.52): the Voicemeeter window capture succeeds even when the window is not in the foreground, as long as it's open at all.
- **Full-display capture** (p.53): same VBAN-Frame mechanism with "Display 1" (or 2/3/4) as the source instead of App View; JPG recommended here (with adjustable 1-100% quality) to reduce bandwidth for a full-screen capture.
- **VBAN-Screen (receiving/display app)** (p.54): a separate PC application (manual notes Mac/other-platform support "coming soon"), monitoring up to 4 simultaneous VBAN-Frame + VBAN-Audio stream pairs at once (labeled VBAN-Screen 1 through 4), each with its own Text Command In, Audio Stream In, Video Stream In, and Mouse command OUT rows (Stream Name, IP Address, Port, Format, Net Quality, Errors per row).
- **VBAN-Screen as a "magic board"** (p.54): can also display freehand drawings sent via an incoming VBAN-Text stream.
- **VBAN-Screen outgoing mouse control** (p.54): offers its own outgoing VBAN-TEXT stream to send Mouse Move, Left Click, and Right Click (via the CTRL key) commands back to the source machine.

## MACRO Buttons (p.56-58)

- **Application overview** (p.56): bundled and auto-installed with Voicemeeter. Displays 4 to 80 programmable buttons, each Push or "2 Positions" (toggle) mode, with a title/subtitle, assignable to a keyboard shortcut, mouse, game pad, MIDI message, or an audio-level trigger. Built on the Voicemeeter Remote API itself, as a real example third-party client.
- **Documented use cases, all editions** (p.56): mute a strip or bus; change gain on one or several strips/busses; toggle bus assignment on one or several strips; combine multiple requests into one complex action (worked example: a single Push-To-Talk/Auto-Ducking button that sets music gain to -10dB *and* mutes another talker at the same time); change voice color/audibility for special announcements; restart the audio engine; save or load a complete configuration file.
- **Banana-edition-only MacroButtons capabilities, explicitly scoped** (p.56): "On Voicemeeter BANANA version, it is also possible to..." make voice FX by changing Modulation and Color Panel; launch sound via the integrated audio player; make corrections via the bus parametric EQ; remote all VBAN functions. Confirmed edition-gated, not available when driving a Standard-edition instance.
- **System-level functions, all editions** (p.56): send a Keyboard Event to the system queue (to remote other applications); execute any program (optionally with a command line); send an M.I.D.I. message to up to 2 devices; send VBAN-MIDI / VBAN-TXT requests.
- **Per-button configuration dialog, exact fields** (p.57): Button Name, Button Color (dropdown), Button Type (Push Button / 2 Positions), Button Sub Name, GPIO (dropdown, "no" shown as an example, implying a GPIO-triggerable option exists), Keyboard Shortcut (dropdown), "Exclusive Key" checkbox, Image filename with separate "Select File Off" / "On" pickers (one image per button state), **Request For Initial State** (a script textbox, run once on Voicemeeter startup), **Request for Button ON / Trigger IN** (script textbox plus a Voicemeeter-Event source dropdown), **Request for Button OFF / Trigger OUT** (script textbox plus its own dropdown), M.I.D.I. Implementation ("Learn (From MIDI mapping device)" checkbox, a learned-code readout e.g. "#1 Note On C#2 (49)", a Reset button), XINPUT (Enable checkbox, a Ctrl/controller-index dropdown, a GamePad Button dropdown e.g. "START"), a TRIGGER section (Enable checkbox, target Strip selector, In threshold, Out threshold, Hold time in ms, Input Mode dropdown shown as "Pre-Fader" -- implying a Post-Fader alternative likely exists though not directly shown on this page), and a HID Device Button field with its own Config button ("connecting directly to any HID device").
- **TRIGGER mechanism, audio-level button activation** (p.57): the same class of control as spec's "audio level trigger" -- an IN threshold (visualized with a green cursor on a level scale) presses the button once the selected strip's level rises above it; an OUT threshold (red cursor) releases it once level falls below; a Hold time sets a minimum period the button stays engaged once triggered.
- **Button Color** (p.58): exactly 8 selectable background colors, numbered 1-8 (orange, yellow/olive, green, teal, blue, purple, magenta, red).
- **Button Image** (p.58): separate images for the OFF and ON states; native resolution 130x108 px; accepts BMP, PNG, JPG, or TIF; a larger image is reframed/scaled, a smaller one centered. Transparent-layer/shadow-effect rendering only works correctly for a PNG smaller than the native resolution. Once an image is assigned, the keyboard-shortcut label is no longer shown on the button face.
- **Voicemeeter Event trigger source** (p.58): an alternative to a scripted/threshold trigger -- a dropdown offering No event, Tape Play, Tape Stop, Tape Rec, Tape EndOfFile, tying a button's ON/OFF state directly to the tape recorder's transport state.

## Voicemeeter Remote Requests -- the scripting/API surface (p.59-60)

- **Request syntax** (p.59): a structured name referring to a Voicemeeter control/parameter, plus a value or string; some instructions are case-sensitive, and the manual recommends capitalizing the first letter of each word by convention.
- **Numeric examples given** (p.59): `Strip(0).mute=1;` (mute on), `Strip(0).mute=0;` (unmute), `Strip(0).mute+=1;` (toggle current state), `Bus(0).mono=1;` (set bus to mono mode), `Bus(0).gain=-10.0;`, `Strip(0).gain=+6.0;`, `Bus(0).gain+=3.0;` (relative add), `Strip(0).gain-=3;` (relative subtract), `Command.Restart=1;` (restart the audio engine).
- **String example given** (p.59): `Command.Load="C:\My Documents\VMConfig1.xml";` (loads a config file by path).
- **Strip indexing is edition-dependent** (p.59): "Strip index is a zero based index related to Voicemeeter version (**3 strips on Voicemeeter [Standard], 5 on Voicemeeter Banana**)." Potato's own count expected to be confirmed from its manual chunk.

- **Strip functions/parameters table, exact** (p.59-60):

  | Parameter | Value range | Remark |
  |---|---|---|
  | `Strip[i].Mono` | 0 (off) or 1 (on) | Mono Button |
  | `Strip[i].Mute` | 0 or 1 | Mute Button |
  | `Strip[i].Solo` | 0 or 1 | Solo Button |
  | `Strip[i].MC` | 0 or 1 | Mute Center Button |
  | `Strip[i].Gain` | -60 to +12 dB | Gain slider |
  | `Strip[i].Pan_x` | -0.5 to +0.5 | (no remark given) |
  | `Strip[i].Pan_y` | 0 to 1.0 | (no remark given) |
  | `Strip[i].Color_x` | -0.5 to +0.5 | Physical Strip Only |
  | `Strip[i].Color_y` | 0 to 1.0 | Physical Strip Only |
  | `Strip[i].Audibility` | 0 to 10 | Voicemeeter 1 [Standard] only |
  | `Strip[i].EQGain1` | -12 to +12 dB | Virtual Strip Only |
  | `Strip[i].EQGain2` | -12 to +12 dB | Virtual Strip Only |
  | `Strip[i].EQGain3` | -12 to +12 dB | Virtual Strip Only |
  | `Strip[i].Label` | String | Strip Label |
  | `Strip[i].A1` | 0 or 1 | Out BUS Assignation |
  | `Strip[i].B1` | 0 or 1 | Out BUS Assignation |
  | `Strip[i].FadeTo` | String `(dBTarget, msTime);` | |
  | `Strip[i].FadeBy` | String `(dB relative change, msTime);` | |
  | `Strip[i].VAIO` | 0 or 1 | Physical strip only |

  **Flagged with maximum precision for the classification pass, not resolved here:** `Pan_x`/`Pan_y` are a distinct **X/Y pair** (not a single scalar), carry no "Physical/Virtual Strip Only" remark (unlike `Color_x`/`Color_y`, which are explicitly marked Physical-only, and `EQGain1-3`, explicitly Virtual-only) -- meaning the API surface may treat Pan as available generically, separately from both the Color Panel's own X/Y (`Color_x`/`Color_y`) and the Equalizer. Given `Color_x`/`Color_y`'s range and hardware-only scoping closely matches the Intellipan Color Panel's 2D pad described on p.22, and given `Pan_x`/`Pan_y`'s own range (`-0.5..0.5` / `0..1.0`) is structurally identical in shape to `Color_x`/`Color_y`'s, `Pan_x`/`Pan_y` may correspond to the Intellipan 3D/Position pad's 2D coordinate (the "POSITION" pad also on p.22) rather than to a simple L/R balance scalar -- in which case this table may not name a simple one-dimensional "pan pot" API parameter at all. This needs deliberate resolution once the fuller strip-control section (expected in a later chunk, both this manual and Potato's) has been extracted too -- do not assume either interpretation without that corroboration.

- **Strip Audio Devices table, physical strip only** (p.60): `Strip[i].device.wdm`, `.ks`, `.mme`, `.asio` -- each Value Range "Device Name" (String).

- **Bus indexing is edition-dependent** (p.60): "Bus index is a zero based index related to Voicemeeter version (**2 busses on Voicemeeter [Standard], 5 on Voicemeeter Banana**)."

- **Bus functions/parameters table, exact, as far as page 60 goes (continues onto the next page)**:

  | Parameter | Value range | Remark |
  |---|---|---|
  | `Bus[i].Mono` | 0 (off), 1 (mono), 2 (stereo reverse) | Mono Button |
  | `Bus[i].Mute` | 0 or 1 | Mute Button |
  | `Bus[i].Gain` | -60 to +12 dB | Gain slider |
  | `Bus[i].Label` | String | "Strip Label" (as printed -- likely a copy/paste label in the manual, presumably meant as "Bus Label") |
  | `Bus[i].mode.normal` | 0 or 1 | BUS Mode |
  | `Bus[i].mode.Amix` | 0 or 1 | BUS Mode (the API name for the "MIX down" button seen in the UI) |
  | `Bus[i].mode.Repeat` | 0 or 1 | BUS Mode (the API name for "Stereo Repeat") |
  | `Bus[i].mode.Composite` | 0 or 1 | BUS Mode |
  | `Bus[i].FadeTo` | String `(dBTarget, msTime);` | |
  | `Bus[i].FadeBy` | String `(dB change, msTime);` | |
  | `Bus[i].VAIO` | 0 or 1 | Physical BUS only |

  **Confirms the per-bus Mono button is a genuine 3-state cycle** (`Bus[i].Mono` = 0/1/2, "off / mono / stereo reverse"), matching the visual walkthrough description already extracted (p.26: "first press sums to mono, second press swaps channels 1 and 2 (stereo reverse), third press returns to off") -- corroborated directly at the API level, not just the UI-prose level.

- **Bus Audio Devices table begins at the bottom of p.60** (physical bus only): `Bus[i].device.wdm`, `.ks`, `.mme` visible so far, each "Device Name" (String), remark "Write only" -- table is cut off by the page break; continued in the pages 61-75 chunk.
