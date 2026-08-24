// The typed frontend side of `loomix-app`'s Tauri commands (spec 3.4 M8),
// mirroring `crates/loomix-app/src/main.rs`'s DTOs and command names
// exactly -- this file has no logic of its own beyond that mapping, so a
// backend command rename is a one-place fix, not a hunt through every
// component that calls it.

import { invoke } from "@tauri-apps/api/core";

export const NUM_STRIPS = 8;
export const NUM_BUSES = 8;
export const CHANNELS = 8;

export type BusMonoName = "off" | "mono" | "stereo_reverse";

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
}

export interface BusSnapshot {
  mute: boolean;
  mono: BusMonoName;
  mode: BusModeName;
  gain_db: number;
}

export interface ControlSnapshot {
  strips: StripSnapshot[];
  buses: BusSnapshot[];
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
}

export interface AudioStatus {
  connected: boolean;
  /** `null` when connected with no input device attached, not just "0 so far". */
  capture_underruns: number | null;
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
