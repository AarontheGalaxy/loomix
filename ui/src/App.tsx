import { useEffect, useRef, useState } from "react";
import {
  BUS_MODES,
  BUS_MODE_LABELS,
  CHANNELS,
  NUM_BUSES,
  NUM_STRIPS,
  getControlSnapshot,
  getMeters,
  setBusGain,
  setBusMode,
  setBusMute,
  setStripBusAssign,
  setStripGainLayer,
  setStripMono,
  setStripMute,
  setStripSolo,
  type ControlSnapshot,
  type MeterSnapshot,
} from "./bridge";

// spec 1.1: 5 physical buses (A1..A5) then 3 virtual (B1..B3), in that
// fixed index order.
const BUS_LABELS = ["A1", "A2", "A3", "A4", "A5", "B1", "B2", "B3"];

// spec 1.1: strips 0..4 are hardware, 5..7 are virtual.
const STRIP_LABELS = ["HW 1", "HW 2", "HW 3", "HW 4", "HW 5", "VI 1", "VI Aux", "VI 3"];

// Reconciliation snapshot: low rate, on direct instruction ("not a
// per-frame round trip") -- the UI already updates optimistically the
// moment a control fires, this just catches drift.
const CONTROL_POLL_MS = 500;
// Meters are meant to move visibly, so this polls much closer to the
// audio thread's own publish rate.
const METER_POLL_MS = 50;

function peakToUnit(level: number): number {
  return Math.min(1, Math.max(0, level));
}

function Meter({ levels }: { levels: number[] | undefined }) {
  const level = levels ? Math.max(levels[0] ?? 0, levels[1] ?? 0) : 0;
  return (
    <div className="meter-vertical">
      <div className="meter-vertical-fill" style={{ height: `${peakToUnit(level) * 100}%` }} />
    </div>
  );
}

interface StripColumnProps {
  index: number;
  snapshot: ControlSnapshot["strips"][number] | undefined;
  meterLevels: number[] | undefined;
  selectedBus: number;
  onChange: () => void;
}

function StripColumn({ index, snapshot, meterLevels, selectedBus, onChange }: StripColumnProps) {
  if (!snapshot) return null;
  const gainDb = snapshot.gain_layer_db[selectedBus] ?? 0;
  const assigned = snapshot.bus_assign[selectedBus] ?? false;

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
  meterLevels: number[] | undefined;
  selected: boolean;
  onSelect: () => void;
  onChange: () => void;
}

function BusColumn({ index, snapshot, meterLevels, selected, onSelect, onChange }: BusColumnProps) {
  if (!snapshot) return null;
  return (
    <div className={selected ? "channel-strip bus-strip selected" : "channel-strip bus-strip"}>
      <button className="channel-label bus-select" onClick={onSelect}>
        {BUS_LABELS[index]}
      </button>
      <button
        className={snapshot.mute ? "toggle toggle-active mute" : "toggle mute"}
        onClick={() => void setBusMute(index, !snapshot.mute).then(onChange)}
      >
        M
      </button>
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

export default function App() {
  const [snapshot, setSnapshot] = useState<ControlSnapshot | null>(null);
  const [meters, setMeters] = useState<MeterSnapshot | null>(null);
  const [selectedBus, setSelectedBus] = useState(0);
  const pendingRefresh = useRef(false);

  const refreshControl = () => {
    void getControlSnapshot().then(setSnapshot);
  };

  useEffect(() => {
    refreshControl();
    const id = setInterval(refreshControl, CONTROL_POLL_MS);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    const id = setInterval(() => {
      void getMeters().then(setMeters);
    }, METER_POLL_MS);
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
        <h1>Loomix</h1>
        <p className="app-subtitle">
          Editing gain layers for bus <strong>{BUS_LABELS[selectedBus]}</strong> -- select a bus
          below to edit its layer instead.
        </p>
      </header>
      <div className="mixer">
        <section className="strip-rack">
          {Array.from({ length: NUM_STRIPS }, (_, i) => (
            <StripColumn
              key={i}
              index={i}
              snapshot={snapshot?.strips[i]}
              meterLevels={meters?.strips[i]}
              selectedBus={selectedBus}
              onChange={onChange}
            />
          ))}
        </section>
        <section className="bus-rack">
          {Array.from({ length: NUM_BUSES }, (_, i) => (
            <BusColumn
              key={i}
              index={i}
              snapshot={snapshot?.buses[i]}
              meterLevels={meters?.buses[i]}
              selected={i === selectedBus}
              onSelect={() => setSelectedBus(i)}
              onChange={onChange}
            />
          ))}
        </section>
      </div>
      <footer className="app-footer">
        {CHANNELS} channels per bus -- strip processing, the EQ graph and device selection land
        next.
      </footer>
    </div>
  );
}
