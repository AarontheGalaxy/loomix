import { describe, expect, it } from "vitest";
import { renderEqGraphSvg } from "./eqGraph.js";
import type { ResponsePoint } from "./eqResponse.js";

const opts = { width: 400, height: 200, dbRange: [-18, 18] as const };

function flatCurve(db: number, n = 5): ResponsePoint[] {
  const freqs = [20, 200, 1000, 5000, 20000].slice(0, n);
  return freqs.map((freqHz) => ({ freqHz, db }));
}

describe("renderEqGraphSvg", () => {
  it("produces valid, well-formed SVG containing one point per curve sample", () => {
    const svg = renderEqGraphSvg(flatCurve(0), opts);
    expect(svg).toContain("<svg");
    expect(svg).toContain("</svg>");
    const match = svg.match(/<polyline points="([^"]+)"/);
    expect(match).not.toBeNull();
    const pointCount = match![1]!.trim().split(/\s+/).length;
    expect(pointCount).toBe(5);
  });

  it("a higher dB value places the curve higher on screen (smaller y)", () => {
    const low = renderEqGraphSvg(flatCurve(-12, 1), opts);
    const high = renderEqGraphSvg(flatCurve(12, 1), opts);
    const yOf = (svg: string) => Number(svg.match(/<polyline points="[\d.]+,([\d.]+)/)![1]);
    expect(yOf(high)).toBeLessThan(yOf(low));
  });

  it("0dB sits at the vertical midpoint for a symmetric dB range", () => {
    const svg = renderEqGraphSvg(flatCurve(0, 1), opts);
    const y = Number(svg.match(/<polyline points="[\d.]+,([\d.]+)/)![1]);
    expect(y).toBeCloseTo(opts.height / 2, 5);
  });

  it("requested gridlines within range each render exactly once", () => {
    const svg = renderEqGraphSvg(flatCurve(0), { ...opts, dbGridlines: [-12, 0, 12] });
    for (const db of [-12, 0, 12]) {
      const count = svg.split(`data-db="${db}"`).length - 1;
      expect(count, `gridline ${db}dB`).toBe(1);
    }
  });

  it("a gridline outside dbRange is not drawn", () => {
    const svg = renderEqGraphSvg(flatCurve(0), { ...opts, dbGridlines: [-12, 0, 12, 24] });
    expect(svg).not.toContain('data-db="24"');
  });

  it("an empty curve still renders a valid, polyline-free SVG", () => {
    const svg = renderEqGraphSvg([], opts);
    expect(svg).toContain("<svg");
    expect(svg).not.toContain("<polyline");
  });
});
