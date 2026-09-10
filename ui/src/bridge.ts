// The typed frontend side of `loomix-app`'s Tauri commands (spec 3.4 M8),
// mirroring `crates/loomix-app/src/main.rs`'s DTOs and command names
// exactly -- this file has no logic of its own beyond that mapping, so a
// backend command rename is a one-place fix, not a hunt through every
// component that calls it.

import { invoke } from "@tauri-apps/api/core";
import type { EqCellParams, EqCellType, EqChannelParams } from "./eqResponse";
export type { EqCellParams, EqCellType, EqChannelParams } from "./eqResponse";

export const NUM_STRIPS = 8;
export const NUM_BUSES = 8;
export const CHANNELS = 8;
// spec 1.2 step 7: the hardware strip EQ is stereo, unlike the bus EQ's
// independent `CHANNELS` (8).
export const STRIP_EQ_CHANNELS = 2;
// spec 1.7: 6 cells per channel.
export const NUM_CELLS = 6;

export type BusMonoName = "off" | "mono" | "stereo_reverse";

// M10: spec 1.18's "cycle Color, Position, Modulation."
export type IntellipanModeName = "color" | "position" | "modulation";

// M10: spec 1.4's Karaoke button, off plus the four named modes.
export type KaraokeModeName = "off" | "km" | "k1" | "k2" | "kv";

// M10: spec 1.7's A/B memory, shared by the strip and bus parametric EQ.
export type MemoryName = "a" | "b";

// spec 1.7's 7 filter types, index 0..6 -- serialised as bare Rust variant
// names (`EqCellType` has no `#[serde(rename_all)]`), not snake_case.
// `EqCellType`/`EqCellParams`/`EqChannelParams` themselves are re-exported
// from `./eqResponse` above (M6 already defined this exact shape for the
// graph math; a second, parallel definition here would just be something
// else to keep in sync with the Rust structs it mirrors).
export const EQ_CELL_TYPES: readonly EqCellType[] = [
  "Peak",
  "LowPass",
  "HighPass",
  "LowShelf",
  "HighShelf",
  "BandPass",
  "Notch",
];

export function defaultEqCellParams(): EqCellParams {
  return { on: false, cell_type: "Peak", freq_hz: 1000, gain_db: 0, q: 1 };
}

// spec 1.6's 12 bus modes, in the same order as `docs/SPEC.md` section
// 1.6 and `loomix_core::bus_mode::BusMode`.
export const BUS_MODES = [
  "normal",
  "mix_down_a",
  "mix_down_b",
  "stereo_repeat",
  "composite",
  "up_mix_tv",
  "up_mix_2_1",
  "up_mix_4_1",
  "up_mix_6_1",
  "center_only",
  "lfe_only",
  "rear_only",
] as const;
export type BusModeName = (typeof BUS_MODES)[number];

export const BUS_MODE_LABELS: Record<BusModeName, string> = {
  normal: "Normal",
  mix_down_a: "Mix Down A",
  mix_down_b: "Mix Down B",
  stereo_repeat: "Stereo Repeat",
  composite: "Composite",
  up_mix_tv: "Up Mix TV",
  up_mix_2_1: "Up Mix 2.1",
  up_mix_4_1: "Up Mix 4.1",
  up_mix_6_1: "Up Mix 6.1",
  center_only: "Center Only",
  lfe_only: "LFE Only",
  rear_only: "Rear Only",
};

export interface StripSnapshot {
  mute: boolean;
  solo: boolean;
  mono: boolean;
  bus_assign: boolean[];
  gain_layer_db: number[];
  /** `0` (center) on a virtual strip, which has no pan pot. */
  pan: number;
  // M10: hardware-only fields read as their own neutral default on a
  // virtual strip, and vice versa for the virtual-only fields below --
  // same convention as `pan` above (`docs/ARCHITECTURE.md`'s M10 entry).
  gate_knob: number;
  comp_knob: number;
  denoiser_knob: number;
  /** Live on both hardware and virtual strips -- both have a limiter. */
  limiter_threshold_db: number;
  intellipan_mode: IntellipanModeName;
  intellipan_x: number;
  intellipan_y: number;
  strip_eq_on: boolean;
  strip_eq_memory: MemoryName;
  eq3_bass_db: number;
  eq3_mid_db: number;
  eq3_treble_db: number;
  position_pad_x: number;
  position_pad_y: number;
  mc: boolean;
  karaoke: KaraokeModeName;
}

export interface BusSnapshot {
  mute: boolean;
  mono: BusMonoName;
  mode: BusModeName;
  gain_db: number;
  eq_on: boolean;
  eq_memory: MemoryName;
}

export interface ControlSnapshot {
  strips: StripSnapshot[];
  buses: BusSnapshot[];
}

/**
 * M10's EQ panel state: `[strip][channel]`/`[bus][channel]`. Polled only
 * while a panel is actually open (`getEqSnapshot`'s own doc comment),
 * unlike `ControlSnapshot`'s continuous low-rate reconciliation -- a
 * virtual strip's channels read as `defaultEqCellParams()`-shaped
 * defaults, since it has no parametric EQ at all (spec 1.4's 3-band EQ
 * instead).
 */
export interface EqSnapshot {
  strips: EqChannelParams[][];
  buses: EqChannelParams[][];
}

/** `[stripOrBus][channel]` peak-hold levels, linear amplitude (spec 1.3/1.5). */
export interface MeterSnapshot {
  strips: number[][];
  buses: number[][];
}

export interface DeviceInfo {
  uid: string;
  name: string;
  input_channels: number;
  output_channels: number;
  /** M11: degraded-device warnings (reduced Bluetooth profile, mono-only output), if any. */
  warnings: string[];
}

export interface AudioStatus {
  connected: boolean;
  /** `null` when connected with no input device attached, not just "0 so far". */
  capture_underruns: number | null;
  /** M11: degraded-device warnings for whichever devices are actually connected. */
  device_warnings: string[];
}

export function listAudioDevices(): Promise<DeviceInfo[]> {
  return invoke("list_audio_devices");
}

export function getAudioStatus(): Promise<AudioStatus> {
  return invoke("get_audio_status");
}

export function connectAudio(inputUid: string | null, outputUid: string): Promise<void> {
  return invoke("connect_audio", { inputUid, outputUid });
}

export function disconnectAudio(): Promise<void> {
  return invoke("disconnect_audio");
}

export function getControlSnapshot(): Promise<ControlSnapshot> {
  return invoke("get_control_snapshot");
}

export function getMeters(): Promise<MeterSnapshot> {
  return invoke("get_meters");
}

export function setStripMute(strip: number, on: boolean): Promise<void> {
  return invoke("set_strip_mute", { strip, on });
}

export function setStripSolo(strip: number, on: boolean): Promise<void> {
  return invoke("set_strip_solo", { strip, on });
}

export function setStripMono(strip: number, on: boolean): Promise<void> {
  return invoke("set_strip_mono", { strip, on });
}

export function setStripBusAssign(strip: number, bus: number, on: boolean): Promise<void> {
  return invoke("set_strip_bus_assign", { strip, bus, on });
}

export function setStripGainLayer(strip: number, bus: number, db: number): Promise<void> {
  return invoke("set_strip_gain_layer", { strip, bus, db });
}

/** `-1` hard left, `0` center, `1` hard right. Hardware strips only. */
export function setStripPan(strip: number, pan: number): Promise<void> {
  return invoke("set_strip_pan", { strip, pan });
}

export function setBusMute(bus: number, on: boolean): Promise<void> {
  return invoke("set_bus_mute", { bus, on });
}

export function setBusMono(bus: number, mono: BusMonoName): Promise<void> {
  return invoke("set_bus_mono", { bus, mono });
}

export function setBusMode(bus: number, mode: BusModeName): Promise<void> {
  return invoke("set_bus_mode", { bus, mode });
}

export function setBusGain(bus: number, db: number): Promise<void> {
  return invoke("set_bus_gain", { bus, db });
}

// -- M10: the 2026-09-09 coverage audit's state-2 list. Each function
// below is the same one-line `invoke(...)` shape as every function above
// -- cross-checked argument-for-argument against `main.rs`'s own command
// signatures and `control.rs`'s `CONTROL_CASES` table, per
// `docs/ARCHITECTURE.md`'s M10 entry, rather than hand-testing this file
// separately (there's no logic here to test beyond the mapping itself).

/** Hardware strips only; `knob <= 0` is a true bypass (`docs/DSP.md`). */
export function setStripGateKnob(strip: number, knob: number): Promise<void> {
  return invoke("set_strip_gate_knob", { strip, knob });
}

/** Hardware strips only. */
export function setStripCompKnob(strip: number, knob: number): Promise<void> {
  return invoke("set_strip_comp_knob", { strip, knob });
}

/** Hardware strips only. */
export function setStripDenoiserKnob(strip: number, knob: number): Promise<void> {
  return invoke("set_strip_denoiser_knob", { strip, knob });
}

/** Both hardware and virtual strips have their own limiter. */
export function setStripLimiterThreshold(strip: number, db: number): Promise<void> {
  return invoke("set_strip_limiter_threshold", { strip, db });
}

/** Hardware strips only; spec 1.18's "right click cycles" gesture. */
export function setStripIntellipanMode(
  strip: number,
  mode: IntellipanModeName,
): Promise<void> {
  return invoke("set_strip_intellipan_mode", { strip, mode });
}

/** Hardware strips only; applies to whichever mode is currently active. */
export function setStripIntellipanXY(strip: number, x: number, y: number): Promise<void> {
  return invoke("set_strip_intellipan_xy", { strip, x, y });
}

/** Virtual strips only (spec 1.4). Each gain is -12..+12 dB. */
export function setStripEq3(
  strip: number,
  bassDb: number,
  midDb: number,
  trebleDb: number,
): Promise<void> {
  return invoke("set_strip_eq3", { strip, bassDb, midDb, trebleDb });
}

/** Virtual strips only; spec 1.4's 5.1 position pad. */
export function setStripPositionPad(strip: number, x: number, y: number): Promise<void> {
  return invoke("set_strip_position_pad", { strip, x, y });
}

/** Virtual strips only; mutes the centre channel of multichannel material. */
export function setStripMc(strip: number, on: boolean): Promise<void> {
  return invoke("set_strip_mc", { strip, on });
}

/** Virtual strips only; audible only on the AUX strip (spec 1.4). */
export function setStripKaraoke(strip: number, mode: KaraokeModeName): Promise<void> {
  return invoke("set_strip_karaoke", { strip, mode });
}

/** Hardware strips only; the strip parametric EQ's on/off toggle. */
export function setStripEqOn(strip: number, on: boolean): Promise<void> {
  return invoke("set_strip_eq_on", { strip, on });
}

/** Hardware strips only; the strip parametric EQ's A/B memory. */
export function setStripEqMemory(strip: number, memory: MemoryName): Promise<void> {
  return invoke("set_strip_eq_memory", { strip, memory });
}

export function setBusEqOn(bus: number, on: boolean): Promise<void> {
  return invoke("set_bus_eq_on", { bus, on });
}

export function setBusEqMemory(bus: number, memory: MemoryName): Promise<void> {
  return invoke("set_bus_eq_memory", { bus, memory });
}

/**
 * Only meaningful while an EQ panel is open -- see this call's own
 * `main.rs::get_eq_snapshot` doc comment for why it isn't part of the
 * continuous `getControlSnapshot` poll.
 */
export function getEqSnapshot(): Promise<EqSnapshot> {
  return invoke("get_eq_snapshot");
}

/** Hardware strips only (spec 1.2 step 7); a no-op on a virtual strip. */
export function setStripEqCell(
  strip: number,
  channel: number,
  cell: number,
  params: EqCellParams,
): Promise<void> {
  return invoke("set_strip_eq_cell", { strip, channel, cell, params });
}

export function setBusEqCell(
  bus: number,
  channel: number,
  cell: number,
  params: EqCellParams,
): Promise<void> {
  return invoke("set_bus_eq_cell", { bus, channel, cell, params });
}

// -- M10 (continued): spec 1.7's per-channel trim/delay, FLAT and CH
// COPY -- brought into M10 rather than deferred (`docs/ARCHITECTURE.md`'s
// M10 entry).

/** Hardware strips only (spec 1.2 step 7); -24..+24 dB. */
export function setStripEqTrim(strip: number, channel: number, trimDb: number): Promise<void> {
  return invoke("set_strip_eq_trim", { strip, channel, trimDb });
}

/** Hardware strips only; 0..500 ms. */
export function setStripEqDelay(strip: number, channel: number, delayMs: number): Promise<void> {
  return invoke("set_strip_eq_delay", { strip, channel, delayMs });
}

export function setBusEqTrim(bus: number, channel: number, trimDb: number): Promise<void> {
  return invoke("set_bus_eq_trim", { bus, channel, trimDb });
}

export function setBusEqDelay(bus: number, channel: number, delayMs: number): Promise<void> {
  return invoke("set_bus_eq_delay", { bus, channel, delayMs });
}

/** FLAT (spec 1.7): resets one channel to its neutral default. */
export function resetStripEqChannel(strip: number, channel: number): Promise<void> {
  return invoke("reset_strip_eq_channel", { strip, channel });
}

export function resetBusEqChannel(bus: number, channel: number): Promise<void> {
  return invoke("reset_bus_eq_channel", { bus, channel });
}

/** CH COPY (spec 1.7): copies `from`'s params onto `to`, within the same EQ. */
export function copyStripEqChannel(strip: number, from: number, to: number): Promise<void> {
  return invoke("copy_strip_eq_channel", { strip, from, to });
}

export function copyBusEqChannel(bus: number, from: number, to: number): Promise<void> {
  return invoke("copy_bus_eq_channel", { bus, from, to });
}
