// A bounded 2D drag surface, shared by the Intellipan pad (spec 1.2 step 8,
// hardware strips, 3 mutually exclusive modes) and the virtual strip's 5.1
// position pad (spec 1.4) -- both are genuinely the same control shape (a
// clamped x/y drag surface, different axis ranges, different downstream
// effect), so one component serves both rather than two near-identical ones.

import { useCallback, useRef } from "react";

export interface XYPadProps {
  /** Current position, already in this pad's own x/y units (not 0..1). */
  x: number;
  y: number;
  xRange: readonly [number, number];
  yRange: readonly [number, number];
  onChange: (x: number, y: number) => void;
  /** Right-click: spec 1.18's "right click the 2D pad, cycle Color,
   * Position, Modulation." Omitted for the 5.1 pad, which has no modes. */
  onCycleMode?: () => void;
  /** Small text drawn in the pad's corner -- the active Intellipan mode,
   * or nothing for the 5.1 pad. */
  label?: string;
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

/**
 * Coalesces same-frame drag updates to at most one call, the JS-side half
 * of spec 3.3's "never one queued command per pointer move" (the Rust side
 * -- `CommandSink`'s per-parameter last-value-wins coalescing -- is proven
 * in `control::tests::a_flood_of_xy_pad_drag_updates_...`; this is what
 * keeps a fast native `pointermove` stream, which can fire well above
 * 60Hz, from turning into that many separate Tauri IPC round trips in the
 * first place). Only the most recent call within a frame survives.
 */
function rafThrottle<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void {
  let scheduled = false;
  let latestArgs: A | null = null;
  return (...args: A) => {
    latestArgs = args;
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      if (latestArgs) fn(...latestArgs);
    });
  };
}

export function XYPad({ x, y, xRange, yRange, onChange, onCycleMode, label }: XYPadProps) {
  const trackRef = useRef<HTMLDivElement>(null);
  const throttledChange = useRef(rafThrottle(onChange));
  throttledChange.current = rafThrottle(onChange); // keep the latest onChange closure

  const positionFromEvent = useCallback(
    (e: { clientX: number; clientY: number }) => {
      const track = trackRef.current;
      if (!track) return null;
      const rect = track.getBoundingClientRect();
      const tx = clamp((e.clientX - rect.left) / rect.width, 0, 1);
      // Screen y grows downward; the pad's own y (0..1, spec 1.3/1.4) grows
      // upward, matching every other vertical control in this app (the
      // fader is min-at-bottom too).
      const ty = clamp(1 - (e.clientY - rect.top) / rect.height, 0, 1);
      const [xMin, xMax] = xRange;
      const [yMin, yMax] = yRange;
      return { x: xMin + tx * (xMax - xMin), y: yMin + ty * (yMax - yMin) };
    },
    [xRange, yRange],
  );

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return; // right click is reserved for the mode cycle below
    e.currentTarget.setPointerCapture(e.pointerId);
    const pos = positionFromEvent(e);
    if (pos) onChange(pos.x, pos.y); // the initial click/tap commits immediately, not throttled
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.buttons !== 1) return; // only while actually dragging
    const pos = positionFromEvent(e);
    if (pos) throttledChange.current(pos.x, pos.y);
  };

  const onContextMenu = (e: React.MouseEvent<HTMLDivElement>) => {
    e.preventDefault(); // spec 1.18's real right-click gesture, not a substitute
    onCycleMode?.();
  };

  const [xMin, xMax] = xRange;
  const [yMin, yMax] = yRange;
  const tx = clamp((x - xMin) / (xMax - xMin), 0, 1);
  const ty = clamp((y - yMin) / (yMax - yMin), 0, 1);

  return (
    <div
      ref={trackRef}
      className="xy-pad"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onContextMenu={onContextMenu}
      title={onCycleMode ? "Drag to position, right click to cycle mode" : "Drag to position"}
    >
      <div className="xy-pad-axis-x" />
      <div className="xy-pad-axis-y" />
      {label && <span className="xy-pad-label">{label}</span>}
      <div
        className="xy-pad-dot"
        style={{ left: `${tx * 100}%`, bottom: `${ty * 100}%` }}
      />
    </div>
  );
}
