import { useEffect, useRef, useState } from "react";
import {
  BUS_MODES,
  BUS_MODE_LABELS,
  CHANNELS,
  NUM_BUSES,
  NUM_STRIPS,
  connectAudio,
  copyBusEqChannel,
  copyStripEqChannel,
  disconnectAudio,
  getAudioStatus,
  getControlSnapshot,
  getEqSnapshot,
  getMeters,
  listAudioDevices,
  resetBusEqChannel,
  resetStripEqChannel,
  setBusEqCell,
  setBusEqDelay,
  setBusEqMemory,
  setBusEqOn,
  setBusEqTrim,
  setBusGain,
  setBusMode,
  setBusMute,
  setStripBusAssign,
  setStripCompKnob,
  setStripDenoiserKnob,
  setStripEq3,
  setStripEqCell,
  setStripEqDelay,
  setStripEqMemory,
  setStripEqOn,
  setStripEqTrim,
  setStripGainLayer,
  setStripGateKnob,
  setStripIntellipanMode,
  setStripIntellipanXY,
  setStripKaraoke,
  setStripLimiterThreshold,
  setStripMc,
  setStripMono,
  setStripMute,
  setStripPan,
  setStripPositionPad,
  setStripSolo,
  type AudioStatus,
  type ControlSnapshot,
  type DeviceInfo,
  type EqCellParams,
  type EqSnapshot,
  type IntellipanModeName,
  type KaraokeModeName,
  type MeterSnapshot,
} from "./bridge";
import { XYPad } from "./XYPad";
import { EqPanel } from "./EqPanel";
import { eqButtonState } from "./eqButtonState";

// spec 1.1: 5 physical buses (A1..A5) then 3 virtual (B1..B3), in that
// fixed index order.
const BUS_LABELS = ["A1", "A2", "A3", "A4", "A5", "B1", "B2", "B3"];

// spec 1.1: strips 0..4 are hardware, 5..7 are virtual.
const STRIP_LABELS = ["HW 1", "HW 2", "HW 3", "HW 4", "HW 5", "VI 1", "VI Aux", "VI 3"];
const NUM_HARDWARE_STRIPS = 5;
// spec 1.4: Karaoke lives on the AUX virtual strip only (`loomix_core::
// strip::topology_is_aux`, confirmed true only for index 6).
const AUX_STRIP_INDEX = 6;

// spec 1.6's 8-channel layout, used for bus EQ's per-channel tabs.
const BUS_CHANNEL_LABELS = ["FL", "FR", "FC", "SW", "RL", "RR", "SL", "SR"];
const STRIP_EQ_CHANNEL_LABELS = ["L", "R"];

// spec 1.18: right click cycles Color -> Position -> Modulation -> Color.
const INTELLIPAN_MODE_CYCLE: IntellipanModeName[] = ["color", "position", "modulation"];
function nextIntellipanMode(mode: IntellipanModeName): IntellipanModeName {
  const i = INTELLIPAN_MODE_CYCLE.indexOf(mode);
  return INTELLIPAN_MODE_CYCLE[(i + 1) % INTELLIPAN_MODE_CYCLE.length] ?? "color";
}
const INTELLIPAN_MODE_LABELS: Record<IntellipanModeName, string> = {
  color: "Color",
  position: "Position",
  modulation: "Modulation",
};

// spec 1.4: off, K-m, K-1, K-2, K-v, in that cycling order.
const KARAOKE_MODE_CYCLE: KaraokeModeName[] = ["off", "km", "k1", "k2", "kv"];
function nextKaraokeMode(mode: KaraokeModeName): KaraokeModeName {
  const i = KARAOKE_MODE_CYCLE.indexOf(mode);
  return KARAOKE_MODE_CYCLE[(i + 1) % KARAOKE_MODE_CYCLE.length] ?? "off";
}
const KARAOKE_MODE_LABELS: Record<KaraokeModeName, string> = {
  off: "Off",
  km: "K-m",
  k1: "K-1",
  k2: "K-2",
  kv: "K-v",
};

// Reconciliation snapshot: low rate, on direct instruction ("not a
// per-frame round trip") -- the UI already updates optimistically the
// moment a control fires, this just catches drift.
const CONTROL_POLL_MS = 500;
// Meters are meant to move visibly, so this polls much closer to the
// audio thread's own publish rate.
const METER_POLL_MS = 50;
// The device list rarely changes mid-session; polling it at the control
// rate would just be wasted enumeration calls.
const DEVICE_POLL_MS = 3000;

function peakToUnit(level: number): number {
  return Math.min(1, Math.max(0, level));
}

// spec 1.2 step 9's balance law (`docs/DSP.md`): -1 hard left, 0 center
// (the default), 1 hard right -- matches the pan pot's own -1..1 range
// exactly, so a raw pan value maps straight to a percentage toward the
// extreme with no rescaling.
function panLabel(pan: number): string {
  if (Math.abs(pan) < 0.005) return "C";
  const pct = Math.round(Math.abs(pan) * 100);
  return pan < 0 ? `L${pct}` : `R${pct}`;
}

function Meter({ levels }: { levels: number[] | undefined }) {
  const level = levels ? Math.max(levels[0] ?? 0, levels[1] ?? 0) : 0;
  return (
    <div className="meter-vertical">
      <div className="meter-vertical-fill" style={{ height: `${peakToUnit(level) * 100}%` }} />
    </div>
  );
}

// One compact labelled row per M10 macro control (gate/comp/denoiser
// knobs, limiter threshold, the 3-band EQ's bands) -- the same shape as
// the existing pan-row above, generalised so eleven new controls don't
// mean eleven near-identical blocks of markup (`docs/ARCHITECTURE.md`'s
// M10 entry: the same copy-paste risk the Rust command layer's table was
// built to avoid applies here too, just at a much smaller scale).
interface MiniSliderProps {
  /** Shown at all times -- short, but the spec's own term, not an
   * invented abbreviation (a user shouldn't have to guess what "DN"
   * means). */
  label: string;
  /** The full name plus the current value, in the hover tooltip -- the
   * label alone stays short because the column is narrow, not because
   * the fuller name isn't available on demand. */
  fullName: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  /** Appended after the value in the tooltip, e.g. " dB"; omit for a
   * unitless 0..10 macro knob. */
  unit?: string;
  /** Appended in parentheses, e.g. "0 = bypassed". */
  neutralHint?: string;
  onChange: (value: number) => void;
}

function MiniSlider({
  label,
  fullName,
  value,
  min,
  max,
  step = 0.1,
  unit = "",
  neutralHint,
  onChange,
}: MiniSliderProps) {
  const title = `${fullName}: ${value.toFixed(1)}${unit}${neutralHint ? ` (${neutralHint})` : ""}`;
  return (
    <div className="mini-slider-row" title={title}>
      <span className="mini-slider-label">{label}</span>
      <input
        className="mini-slider"
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        aria-label={title}
      />
    </div>
  );
}

interface EqButtonProps {
  state: ReturnType<typeof eqButtonState>;
  onClick: () => void;
}

/** Spec 1.5's colour convention (`EqPanel.tsx`'s own doc comment) -- never
 * requires opening the dialog to see whether a strip/bus is filtered. */
function EqButton({ state, onClick }: EqButtonProps) {
  return (
    <button className={`eq-button eq-button-${state}`} onClick={onClick} title="Parametric EQ">
      EQ
    </button>
  );
}

interface StripColumnProps {
  index: number;
  snapshot: ControlSnapshot["strips"][number] | undefined;
  eqChannels: EqSnapshot["strips"][number] | undefined;
  meterLevels: number[] | undefined;
  selectedBus: number;
  onChange: () => void;
  onOpenEq: () => void;
}

function StripColumn({
  index,
  snapshot,
  eqChannels,
  meterLevels,
  selectedBus,
  onChange,
  onOpenEq,
}: StripColumnProps) {
  if (!snapshot) return null;
  const gainDb = snapshot.gain_layer_db[selectedBus] ?? 0;
  const assigned = snapshot.bus_assign[selectedBus] ?? false;
  const isHardware = index < NUM_HARDWARE_STRIPS;
  const isAux = index === AUX_STRIP_INDEX;

  return (
    <div className="channel-strip">
      <div className="channel-label">{STRIP_LABELS[index]}</div>
      <div className="channel-buttons">
        <button
          className={snapshot.mute ? "toggle toggle-active mute" : "toggle mute"}
          onClick={() => void setStripMute(index, !snapshot.mute).then(onChange)}
        >
          M
        </button>
        <button
          className={snapshot.solo ? "toggle toggle-active solo" : "toggle solo"}
          onClick={() => void setStripSolo(index, !snapshot.solo).then(onChange)}
        >
          S
        </button>
        <button
          className={snapshot.mono ? "toggle toggle-active" : "toggle"}
          onClick={() => void setStripMono(index, !snapshot.mono).then(onChange)}
        >
          Mono
        </button>
        {!isHardware && (
          <button
            className={snapshot.mc ? "toggle toggle-active" : "toggle"}
            onClick={() => void setStripMc(index, !snapshot.mc).then(onChange)}
            title="M.C.: mute the centre channel of multichannel material"
          >
            MC
          </button>
        )}
        {/* Karaoke lives in this same row, only rendered for the AUX
            strip, rather than a row of its own -- a row that exists on
            one virtual strip and not its neighbours is exactly what
            shifted VI Aux's whole stack down relative to VI 1/VI 3 in the
            first pass of this layout. Every row from here down is either
            present on every strip or reserved-but-invisible on the ones
            it doesn't apply to, so the fader always lines up across all
            eight columns. */}
        {isAux && (
          <button
            className={snapshot.karaoke !== "off" ? "toggle toggle-active" : "toggle"}
            onClick={() =>
              void setStripKaraoke(index, nextKaraokeMode(snapshot.karaoke)).then(onChange)
            }
            title={`Karaoke: ${KARAOKE_MODE_LABELS[snapshot.karaoke]} -- click to cycle Off / K-m / K-1 / K-2 / K-v`}
          >
            {KARAOKE_MODE_LABELS[snapshot.karaoke]}
          </button>
        )}
      </div>
      <label className="bus-assign">
        <input
          type="checkbox"
          checked={assigned}
          onChange={(e) =>
            void setStripBusAssign(index, selectedBus, e.target.checked).then(onChange)
          }
        />
        {BUS_LABELS[selectedBus]}
      </label>
      {isHardware ? (
        <>
          <MiniSlider
            label="Denoiser"
            fullName="Denoiser"
            value={snapshot.denoiser_knob}
            min={0}
            max={10}
            neutralHint="0 = bypassed"
            onChange={(v) => void setStripDenoiserKnob(index, v).then(onChange)}
          />
          <MiniSlider
            label="Gate"
            fullName="Gate"
            value={snapshot.gate_knob}
            min={0}
            max={10}
            neutralHint="0 = bypassed"
            onChange={(v) => void setStripGateKnob(index, v).then(onChange)}
          />
          <MiniSlider
            label="Comp"
            fullName="Compressor"
            value={snapshot.comp_knob}
            min={0}
            max={10}
            neutralHint="0 = bypassed"
            onChange={(v) => void setStripCompKnob(index, v).then(onChange)}
          />
          <EqButton state={eqButtonState(snapshot.strip_eq_on, eqChannels ?? [])} onClick={onOpenEq} />
          <XYPad
            x={snapshot.intellipan_x}
            y={snapshot.intellipan_y}
            xRange={[-0.5, 0.5]}
            yRange={[0, 1]}
            onChange={(x, y) => void setStripIntellipanXY(index, x, y).then(onChange)}
            onCycleMode={() =>
              void setStripIntellipanMode(index, nextIntellipanMode(snapshot.intellipan_mode)).then(
                onChange,
              )
            }
            label={INTELLIPAN_MODE_LABELS[snapshot.intellipan_mode]}
          />
        </>
      ) : (
        <>
          <MiniSlider
            label="Bass"
            fullName="3-band EQ: Bass"
            value={snapshot.eq3_bass_db}
            min={-12}
            max={12}
            unit=" dB"
            onChange={(v) =>
              void setStripEq3(index, v, snapshot.eq3_mid_db, snapshot.eq3_treble_db).then(onChange)
            }
          />
          <MiniSlider
            label="Mid"
            fullName="3-band EQ: Mid"
            value={snapshot.eq3_mid_db}
            min={-12}
            max={12}
            unit=" dB"
            onChange={(v) =>
              void setStripEq3(index, snapshot.eq3_bass_db, v, snapshot.eq3_treble_db).then(onChange)
            }
          />
          <MiniSlider
            label="Treble"
            fullName="3-band EQ: Treble"
            value={snapshot.eq3_treble_db}
            min={-12}
            max={12}
            unit=" dB"
            onChange={(v) =>
              void setStripEq3(index, snapshot.eq3_bass_db, snapshot.eq3_mid_db, v).then(onChange)
            }
          />
          {/* Reserved, not omitted: a hardware strip's EQ-button row takes
              real vertical space, and virtual strips have no parametric
              EQ to trigger (spec 1.4's 3-band EQ instead) -- an invisible
              placeholder of the same height keeps every column's fader
              starting at the same row, the same fix as the karaoke row
              above, applied to a row that differs by strip *kind* rather
              than by one strip's own state. */}
          <button className="eq-button" style={{ visibility: "hidden" }} aria-hidden="true">
            EQ
          </button>
          <XYPad
            x={snapshot.position_pad_x}
            y={snapshot.position_pad_y}
            xRange={[-0.5, 0.5]}
            yRange={[0, 1]}
            onChange={(x, y) => void setStripPositionPad(index, x, y).then(onChange)}
            label="5.1"
          />
        </>
      )}
      {isHardware && (
        <div className="pan-row">
          <input
            className="pan-slider"
            type="range"
            min={-1}
            max={1}
            step={0.01}
            value={snapshot.pan}
            onChange={(e) => void setStripPan(index, Number(e.target.value)).then(onChange)}
            title={`Pan: ${panLabel(snapshot.pan)}`}
          />
          <span className="pan-value">{panLabel(snapshot.pan)}</span>
        </div>
      )}
      {!isHardware && (
        // Reserved (see the EQ-button placeholder above): a virtual strip
        // has no separate pan pot (spec 1.4's 5.1 pad is its only
        // positioning control), but the row still needs to take up the
        // same space hardware's pan-row does.
        <div className="pan-row" style={{ visibility: "hidden" }} aria-hidden="true">
          <input className="pan-slider" type="range" min={-1} max={1} readOnly value={0} tabIndex={-1} />
          <span className="pan-value">C</span>
        </div>
      )}
      <MiniSlider
        label="Limiter"
        fullName="Limiter threshold"
        value={snapshot.limiter_threshold_db}
        min={-40}
        max={12}
        unit=" dB"
        onChange={(v) => void setStripLimiterThreshold(index, v).then(onChange)}
      />
      <div className="fader-meter-row">
        <input
          className="fader"
          type="range"
          min={-60}
          max={12}
          step={0.1}
          value={gainDb}
          onChange={(e) =>
            void setStripGainLayer(index, selectedBus, Number(e.target.value)).then(onChange)
          }
        />
        <Meter levels={meterLevels} />
      </div>
      <div className="fader-value">{gainDb.toFixed(1)} dB</div>
    </div>
  );
}

interface BusColumnProps {
  index: number;
  snapshot: ControlSnapshot["buses"][number] | undefined;
  eqChannels: EqSnapshot["buses"][number] | undefined;
  meterLevels: number[] | undefined;
  selected: boolean;
  onSelect: () => void;
  onChange: () => void;
  onOpenEq: () => void;
}

function BusColumn({
  index,
  snapshot,
  eqChannels,
  meterLevels,
  selected,
  onSelect,
  onChange,
  onOpenEq,
}: BusColumnProps) {
  if (!snapshot) return null;
  return (
    <div className={selected ? "channel-strip bus-strip selected" : "channel-strip bus-strip"}>
      <button className="channel-label bus-select" onClick={onSelect}>
        {BUS_LABELS[index]}
      </button>
      <div className="channel-buttons">
        <button
          className={snapshot.mute ? "toggle toggle-active mute" : "toggle mute"}
          onClick={() => void setBusMute(index, !snapshot.mute).then(onChange)}
        >
          M
        </button>
        <EqButton state={eqButtonState(snapshot.eq_on, eqChannels ?? [])} onClick={onOpenEq} />
      </div>
      <select
        className="bus-mode"
        value={snapshot.mode}
        onChange={(e) => void setBusMode(index, e.target.value as (typeof BUS_MODES)[number]).then(onChange)}
      >
        {BUS_MODES.map((mode) => (
          <option key={mode} value={mode}>
            {BUS_MODE_LABELS[mode]}
          </option>
        ))}
      </select>
      <div className="fader-meter-row">
        <input
          className="fader"
          type="range"
          min={-60}
          max={12}
          step={0.1}
          value={snapshot.gain_db}
          onChange={(e) => void setBusGain(index, Number(e.target.value)).then(onChange)}
        />
        <Meter levels={meterLevels} />
      </div>
      <div className="fader-value">{snapshot.gain_db.toFixed(1)} dB</div>
    </div>
  );
}

interface DevicePickerProps {
  devices: DeviceInfo[];
  status: AudioStatus | null;
  onConnectionChange: () => void;
}

function DevicePicker({ devices, status, onConnectionChange }: DevicePickerProps) {
  const inputs = devices.filter((d) => d.input_channels > 0);
  const outputs = devices.filter((d) => d.output_channels > 0);
  const [inputUid, setInputUid] = useState("");
  const [outputUid, setOutputUid] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const connected = status?.connected ?? false;

  const connect = () => {
    if (!outputUid) {
      setError("choose an output device first");
      return;
    }
    setConnecting(true);
    setError(null);
    void connectAudio(inputUid || null, outputUid)
      .then(onConnectionChange)
      .catch((e: unknown) => setError(String(e)))
      .finally(() => setConnecting(false));
  };

  const disconnect = () => {
    void disconnectAudio().then(onConnectionChange);
  };

  return (
    <div className="device-picker">
      <select
        className="device-select"
        value={inputUid}
        onChange={(e) => setInputUid(e.target.value)}
        disabled={connected}
      >
        <option value="">No input</option>
        {inputs.map((d) => (
          <option key={d.uid} value={d.uid} title={d.warnings.join(" ")}>
            {d.name} ({d.input_channels}ch in)
            {d.warnings.length > 0 ? " -- degraded" : ""}
          </option>
        ))}
      </select>
      <select
        className="device-select"
        value={outputUid}
        onChange={(e) => setOutputUid(e.target.value)}
        disabled={connected}
      >
        <option value="">Choose output...</option>
        {outputs.map((d) => (
          <option key={d.uid} value={d.uid} title={d.warnings.join(" ")}>
            {d.name} ({d.output_channels}ch out)
            {d.warnings.length > 0 ? " -- degraded" : ""}
          </option>
        ))}
      </select>
      {connected ? (
        <button className="device-connect" onClick={disconnect}>
          Disconnect
        </button>
      ) : (
        <button className="device-connect" onClick={connect} disabled={connecting}>
          {connecting ? "Connecting..." : "Connect"}
        </button>
      )}
      <span className="device-status">
        {connected
          ? `Connected${
              status?.capture_underruns != null
                ? ` -- ${status.capture_underruns} input underrun(s)`
                : ""
            }`
          : "Not connected"}
      </span>
      {error && <span className="device-error">{error}</span>}
      {connected &&
        status?.device_warnings.map((w) => (
          <span key={w} className="device-warning">
            {w}
          </span>
        ))}
    </div>
  );
}

export default function App() {
  const [snapshot, setSnapshot] = useState<ControlSnapshot | null>(null);
  const [eqSnapshot, setEqSnapshot] = useState<EqSnapshot | null>(null);
  const [meters, setMeters] = useState<MeterSnapshot | null>(null);
  const [selectedBus, setSelectedBus] = useState(0);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [audioStatus, setAudioStatus] = useState<AudioStatus | null>(null);
  const [eqPanelTarget, setEqPanelTarget] = useState<{ kind: "strip" | "bus"; index: number } | null>(
    null,
  );
  const pendingRefresh = useRef(false);

  const refreshControl = () => {
    void getControlSnapshot().then(setSnapshot);
  };

  // Polled at the same low, reconciliation rate as `ControlSnapshot`
  // (spec 3.3's low-rate "just enough that drift is detectable" pattern,
  // extended to EQ cells now that a UI actually edits them) -- every EQ
  // trigger's glance indicator needs this live on the main view, not just
  // while its own panel happens to be open, so this can't be deferred
  // until a panel opens the way an on-demand fetch would.
  const refreshEq = () => {
    void getEqSnapshot().then(setEqSnapshot);
  };

  const refreshDevices = () => {
    void listAudioDevices().then(setDevices);
  };

  const refreshStatus = () => {
    void getAudioStatus().then(setAudioStatus);
  };

  useEffect(() => {
    refreshControl();
    const id = setInterval(refreshControl, CONTROL_POLL_MS);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    refreshEq();
    const id = setInterval(refreshEq, CONTROL_POLL_MS);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    const id = setInterval(() => {
      void getMeters().then(setMeters);
    }, METER_POLL_MS);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    refreshDevices();
    const id = setInterval(refreshDevices, DEVICE_POLL_MS);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    refreshStatus();
    const id = setInterval(refreshStatus, CONTROL_POLL_MS);
    return () => clearInterval(id);
  }, []);

  // An action already updated the UI's optimistic state locally in spirit
  // (the control matches its own event), but this app polls rather than
  // keeping a separate local mirror in this first slice -- refreshing
  // right after a command still gives near-immediate feedback without
  // that extra bookkeeping. Debounced so a rapid burst of edits doesn't
  // fire a refresh per keystroke.
  const onChange = () => {
    if (pendingRefresh.current) return;
    pendingRefresh.current = true;
    setTimeout(() => {
      pendingRefresh.current = false;
      refreshControl();
    }, 60);
  };

  return (
    <div className="app">
      <header className="app-header">
        <div className="app-header-row">
          <div>
            <h1>Loomix</h1>
            <p className="app-subtitle">
              Editing gain layers for bus <strong>{BUS_LABELS[selectedBus]}</strong> -- select a
              bus below to edit its layer instead.
            </p>
          </div>
          <DevicePicker
            devices={devices}
            status={audioStatus}
            onConnectionChange={() => {
              refreshStatus();
              refreshControl();
            }}
          />
        </div>
      </header>
      <div className="mixer">
        <section className="strip-rack">
          {Array.from({ length: NUM_STRIPS }, (_, i) => (
            <StripColumn
              key={i}
              index={i}
              snapshot={snapshot?.strips[i]}
              eqChannels={eqSnapshot?.strips[i]}
              meterLevels={meters?.strips[i]}
              selectedBus={selectedBus}
              onChange={onChange}
              onOpenEq={() => setEqPanelTarget({ kind: "strip", index: i })}
            />
          ))}
        </section>
        <section className="bus-rack">
          {Array.from({ length: NUM_BUSES }, (_, i) => (
            <BusColumn
              key={i}
              index={i}
              snapshot={snapshot?.buses[i]}
              eqChannels={eqSnapshot?.buses[i]}
              meterLevels={meters?.buses[i]}
              selected={i === selectedBus}
              onSelect={() => setSelectedBus(i)}
              onChange={onChange}
              onOpenEq={() => setEqPanelTarget({ kind: "bus", index: i })}
            />
          ))}
        </section>
      </div>
      <footer className="app-footer">{CHANNELS} channels per bus.</footer>
      {eqPanelTarget &&
        (() => {
          const { kind, index } = eqPanelTarget;
          const on =
            kind === "strip" ? snapshot?.strips[index]?.strip_eq_on : snapshot?.buses[index]?.eq_on;
          const memoryIsB =
            (kind === "strip"
              ? snapshot?.strips[index]?.strip_eq_memory
              : snapshot?.buses[index]?.eq_memory) === "b";
          const channels = kind === "strip" ? eqSnapshot?.strips[index] : eqSnapshot?.buses[index];
          if (on === undefined || !channels) return null;
          return (
            <EqPanel
              target={{
                kind,
                index,
                channelLabels: kind === "strip" ? STRIP_EQ_CHANNEL_LABELS : BUS_CHANNEL_LABELS,
                channels,
                on,
                memoryIsB,
              }}
              onSetOn={(v) => {
                void (kind === "strip" ? setStripEqOn(index, v) : setBusEqOn(index, v)).then(() => {
                  refreshControl();
                });
              }}
              onSetMemoryIsB={(isB) => {
                const memory = isB ? "b" : "a";
                void (kind === "strip"
                  ? setStripEqMemory(index, memory)
                  : setBusEqMemory(index, memory)
                ).then(() => {
                  refreshControl();
                  refreshEq();
                });
              }}
              onSetCell={(channel, cellIndex, params: EqCellParams) => {
                void (kind === "strip"
                  ? setStripEqCell(index, channel, cellIndex, params)
                  : setBusEqCell(index, channel, cellIndex, params)
                ).then(refreshEq);
              }}
              onSetTrim={(channel, trimDb) => {
                void (kind === "strip"
                  ? setStripEqTrim(index, channel, trimDb)
                  : setBusEqTrim(index, channel, trimDb)
                ).then(refreshEq);
              }}
              onSetDelay={(channel, delayMs) => {
                void (kind === "strip"
                  ? setStripEqDelay(index, channel, delayMs)
                  : setBusEqDelay(index, channel, delayMs)
                ).then(refreshEq);
              }}
              onResetChannel={(channel) => {
                void (kind === "strip"
                  ? resetStripEqChannel(index, channel)
                  : resetBusEqChannel(index, channel)
                ).then(refreshEq);
              }}
              onCopyChannel={(from, to) => {
                void (kind === "strip"
                  ? copyStripEqChannel(index, from, to)
                  : copyBusEqChannel(index, from, to)
                ).then(refreshEq);
              }}
              onClose={() => setEqPanelTarget(null)}
            />
          );
        })()}
    </div>
  );
}
