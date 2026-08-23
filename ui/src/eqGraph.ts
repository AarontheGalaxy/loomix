// Framework-free rendering of an EQ response curve (`eqResponse.ts`) as an
// SVG string, spec 1.7's EQ graph -- no React/Tauri yet (`ui/` stays a
// bare TS project until the app-shell milestone needs one). This is a
// draw function, not an interactive control: right-click-to-type-a-value
// and right-click-to-change-the-dB-scale are real UI/React work, deferred.
// `opts.dbRange` exists precisely so a future interactive scale control
// has something to drive.

import type { ResponsePoint } from "./eqResponse.js";

export interface EqGraphOptions {
  width: number;
  height: number;
  /** Visible dB range, symmetric or not — e.g. `[-18, 18]`. */
  dbRange: readonly [number, number];
  /** Gridlines drawn at these dB values, if within `dbRange`. */
  dbGridlines?: readonly number[];
}

const DEFAULT_GRIDLINES = [-12, -6, 0, 6, 12];

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** Maps a response curve's dB value to an SVG y-coordinate (0 at top). */
function dbToY(db: number, opts: EqGraphOptions): number {
  const [dbMin, dbMax] = opts.dbRange;
  const t = clamp((db - dbMin) / (dbMax - dbMin), 0, 1);
  return opts.height * (1 - t);
}

/** Maps a frequency (log scale, 20Hz..20kHz per spec 1.7) to an SVG x-coordinate. */
function freqToX(freqHz: number, opts: EqGraphOptions): number {
  const logMin = Math.log10(20);
  const logMax = Math.log10(20_000);
  const t = clamp((Math.log10(Math.max(freqHz, 20)) - logMin) / (logMax - logMin), 0, 1);
  return opts.width * t;
}

/**
 * Renders `points` (already computed via `computeResponseCurve`) as a
 * self-contained SVG string: a background, dB gridlines, and the response
 * curve as a single polyline. No external assets, no script.
 */
export function renderEqGraphSvg(points: readonly ResponsePoint[], opts: EqGraphOptions): string {
  const gridlines = opts.dbGridlines ?? DEFAULT_GRIDLINES;
  const [dbMin, dbMax] = opts.dbRange;

  const gridlineElements = gridlines
    .filter((db) => db >= dbMin && db <= dbMax)
    .map((db) => {
      const y = dbToY(db, opts);
      return `<line x1="0" y1="${y}" x2="${opts.width}" y2="${y}" class="eq-graph-gridline" data-db="${db}" />`;
    })
    .join("");

  const pathPoints = points
    .map((p) => `${freqToX(p.freqHz, opts)},${dbToY(p.db, opts)}`)
    .join(" ");

  return [
    `<svg xmlns="http://www.w3.org/2000/svg" width="${opts.width}" height="${opts.height}" viewBox="0 0 ${opts.width} ${opts.height}" class="eq-graph">`,
    `<rect x="0" y="0" width="${opts.width}" height="${opts.height}" class="eq-graph-background" />`,
    gridlineElements,
    points.length > 0
      ? `<polyline points="${pathPoints}" class="eq-graph-curve" fill="none" />`
      : "",
    `</svg>`,
  ].join("");
}
