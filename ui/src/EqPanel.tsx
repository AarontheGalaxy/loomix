// The shared parametric EQ panel (spec 1.7), one component for both the
// hardware strip EQ (stereo, spec 1.2 step 7) and the bus EQ (independent
// per channel, spec 1.5) -- the engine already shares one `ParametricEq<N>`
// implementation for both (`docs/DSP.md`), so the panel does too, rather
// than two near-identical dialogs. Built on `eqGraph.ts`'s existing SVG
// renderer and `eqResponse.ts`'s existing response math (both M6, never
// wired to anything before this milestone).
//
// Scope, decided explicitly rather than left to guesswork: on/off, A/B,
// the six cells, trim, delay, FLAT (reset one channel) and CH COPY are
// all in this panel -- every one of them was already fully implemented
// and tested in `loomix-core` before M10, so leaving any of them unwired
// would recreate the exact state-2 gap this milestone exists to close.
// Still deferred, each to a real milestone rather than silently dropped
// (`docs/ARCHITECTURE.md`'s M10 entry, `docs/SPEC.md` 1.7 -- M14 owns all
// three): COPY ALL (a genuinely different command shape -- it copies
// between two separate EQ instances, strip-to-bus or bus-to-bus, not one
// channel to another inside the same instance), loading/saving the whole
// EQ set as a file (real file I/O and a save/open dialog, not just wiring
// an existing pure function), and right-click-to-type-an-exact-value /
// right-click-to-change-the-graph's-dB-scale (spec 1.18 interaction
// conventions that cut across many controls, not specific to this panel).
//
// Purely prop-driven, no fetch of its own: `App.tsx` already polls
// `EqSnapshot` continuously (the glance indicator on the main view needs
// it live, not just while a panel happens to be open), so the panel just
// reads the slice it needs and calls back up through the same command
// functions every other control in this app already uses.

import { useMemo, useState } from "react";
import { EQ_CELL_TYPES, type EqCellParams, type EqCellType, type EqChannelParams } from "./bridge";
import { computeResponseCurve } from "./eqResponse";
import { renderEqGraphSvg } from "./eqGraph";

// The graph's math needs a sample rate; the engine's real one isn't
// exposed over the bridge today (nothing else needs it yet). 48kHz is the
// engine's own default (`loomix_core::engine::DEFAULT_SAMPLE_RATE`) and
// close enough for a visual graph regardless of the real device rate --
// ponytail: hardcode 48kHz, upgrade to a real query if the curve's shape
// is ever found to visibly mismatch a session running at a different rate.
const GRAPH_SAMPLE_RATE = 48_000;
const GRAPH_WIDTH = 320;
const GRAPH_HEIGHT = 120;
const NUM_GRAPH_POINTS = 200;

const GRAPH_FREQS: number[] = Array.from({ length: NUM_GRAPH_POINTS }, (_, i) => {
  const t = i / (NUM_GRAPH_POINTS - 1);
  return 20 * 1000 ** t; // 20Hz .. 20kHz, log-spaced (matches eqGraph.ts's own mapping)
});

export interface EqPanelTarget {
  kind: "strip" | "bus";
  index: number;
  /** Channel labels in order -- `["L", "R"]` for a strip, `FL..SR` for a bus. */
  channelLabels: readonly string[];
  channels: readonly EqChannelParams[];
  on: boolean;
  memoryIsB: boolean;
}

interface EqPanelProps {
  target: EqPanelTarget;
  onSetOn: (on: boolean) => void;
  onSetMemoryIsB: (isB: boolean) => void;
  onSetCell: (channel: number, cellIndex: number, params: EqCellParams) => void;
  onSetTrim: (channel: number, trimDb: number) => void;
  onSetDelay: (channel: number, delayMs: number) => void;
  onResetChannel: (channel: number) => void;
  onCopyChannel: (from: number, to: number) => void;
  onClose: () => void;
}

export function EqPanel({
  target,
  onSetOn,
  onSetMemoryIsB,
  onSetCell,
  onSetTrim,
  onSetDelay,
  onResetChannel,
  onCopyChannel,
  onClose,
}: EqPanelProps) {
  const [selectedChannel, setSelectedChannel] = useState(0);
  const [copyTarget, setCopyTarget] = useState(() =>
    target.channelLabels.length > 1 ? 1 : 0,
  );
  const channel = target.channels[selectedChannel];

  const graphSvg = useMemo(() => {
    if (!channel) return "";
    const points = computeResponseCurve(channel, GRAPH_FREQS, GRAPH_SAMPLE_RATE);
    return renderEqGraphSvg(points, {
      width: GRAPH_WIDTH,
      height: GRAPH_HEIGHT,
      dbRange: [-18, 18],
    });
  }, [channel]);

  const title = target.kind === "strip" ? `Strip ${target.index + 1}` : `Bus ${target.index + 1}`;

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="eq-panel" onClick={(e) => e.stopPropagation()}>
        <div className="eq-panel-header">
          <h2>{title} parametric EQ</h2>
          <button className="eq-panel-close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </div>

        <div className="eq-panel-toolbar">
          <button
            className={target.on ? "toggle toggle-active" : "toggle"}
            onClick={() => onSetOn(!target.on)}
          >
            {target.on ? "On" : "Off"}
          </button>
          <div className="eq-panel-memory">
            <button
              className={!target.memoryIsB ? "toggle toggle-active" : "toggle"}
              onClick={() => onSetMemoryIsB(false)}
            >
              A
            </button>
            <button
              className={target.memoryIsB ? "toggle toggle-active" : "toggle"}
              onClick={() => onSetMemoryIsB(true)}
            >
              B
            </button>
          </div>
          <div className="eq-panel-channels">
            {target.channelLabels.map((label, i) => (
              <button
                key={label}
                className={i === selectedChannel ? "toggle toggle-active" : "toggle"}
                onClick={() => setSelectedChannel(i)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>

        {channel ? (
          <>
            <div className="eq-graph-wrap" dangerouslySetInnerHTML={{ __html: graphSvg }} />

            <div className="eq-panel-channel-tools">
              <label className="eq-panel-field" title="Trim: -24..+24 dB, applied to the whole channel">
                Trim
                <input
                  type="number"
                  min={-24}
                  max={24}
                  step={0.5}
                  value={channel.trim_db}
                  onChange={(e) => onSetTrim(selectedChannel, Number(e.target.value))}
                />
                dB
              </label>
              <label className="eq-panel-field" title="Delay: 0..500 ms, applied to the whole channel">
                Delay
                <input
                  type="number"
                  min={0}
                  max={500}
                  step={1}
                  value={channel.delay_ms}
                  onChange={(e) => onSetDelay(selectedChannel, Number(e.target.value))}
                />
                ms
              </label>
              <button
                className="toggle"
                onClick={() => onResetChannel(selectedChannel)}
                title="FLAT: reset this channel to its neutral default (every cell off, trim 0, delay 0)"
              >
                FLAT
              </button>
              {target.channelLabels.length > 1 && (
                <span className="eq-panel-copy">
                  <span>CH COPY to</span>
                  <select
                    value={copyTarget}
                    onChange={(e) => setCopyTarget(Number(e.target.value))}
                  >
                    {target.channelLabels.map((label, i) =>
                      i === selectedChannel ? null : (
                        <option key={label} value={i}>
                          {label}
                        </option>
                      ),
                    )}
                  </select>
                  <button
                    className="toggle"
                    onClick={() => onCopyChannel(selectedChannel, copyTarget)}
                    title={`Copy this channel's EQ onto ${target.channelLabels[copyTarget]}`}
                  >
                    Copy
                  </button>
                </span>
              )}
            </div>

            <table className="eq-cell-table">
              <thead>
                <tr>
                  <th>On</th>
                  <th>Type</th>
                  <th>Freq (Hz)</th>
                  <th>Gain (dB)</th>
                  <th>Q</th>
                </tr>
              </thead>
              <tbody>
                {channel.cells.map((cell, i) => (
                  <tr key={i}>
                    <td>
                      <input
                        type="checkbox"
                        checked={cell.on}
                        onChange={(e) =>
                          onSetCell(selectedChannel, i, { ...cell, on: e.target.checked })
                        }
                      />
                    </td>
                    <td>
                      <select
                        value={cell.cell_type}
                        onChange={(e) =>
                          onSetCell(selectedChannel, i, {
                            ...cell,
                            cell_type: e.target.value as EqCellType,
                          })
                        }
                      >
                        {EQ_CELL_TYPES.map((t) => (
                          <option key={t} value={t}>
                            {t}
                          </option>
                        ))}
                      </select>
                    </td>
                    <td>
                      <input
                        type="number"
                        min={20}
                        max={20000}
                        value={Math.round(cell.freq_hz)}
                        onChange={(e) =>
                          onSetCell(selectedChannel, i, { ...cell, freq_hz: Number(e.target.value) })
                        }
                      />
                    </td>
                    <td>
                      <input
                        type="number"
                        min={-36}
                        max={18}
                        step={0.5}
                        value={cell.gain_db}
                        onChange={(e) =>
                          onSetCell(selectedChannel, i, { ...cell, gain_db: Number(e.target.value) })
                        }
                      />
                    </td>
                    <td>
                      <input
                        type="number"
                        min={1}
                        max={100}
                        step={0.1}
                        value={cell.q}
                        onChange={(e) =>
                          onSetCell(selectedChannel, i, { ...cell, q: Number(e.target.value) })
                        }
                      />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        ) : (
          <p className="eq-panel-loading">No channel data yet.</p>
        )}
      </div>
    </div>
  );
}
