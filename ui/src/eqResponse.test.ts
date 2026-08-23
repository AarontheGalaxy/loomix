import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { channelResponseDb, type EqChannelParams } from "./eqResponse.js";

// The same file `crates/loomix-core/tests/eq_response_fixture.rs` checks
// its own Rust computation against — read here too, so this test compares
// this TS implementation against the Rust engine's actual output, not
// against numbers copied by hand into this file.
const fixturePath = fileURLToPath(
  new URL("../../testdata/fixtures/eq_response_reference.json", import.meta.url),
);

interface Fixture {
  sample_rate: number;
  cases: {
    name: string;
    channel: EqChannelParams;
    points: { freq_hz: number; db: number }[];
  }[];
}

function loadFixture(): Fixture {
  return JSON.parse(readFileSync(fixturePath, "utf-8")) as Fixture;
}

describe("channelResponseDb against the Rust engine's fixture", () => {
  const fixture = loadFixture();

  it("has at least one case to check", () => {
    expect(fixture.cases.length).toBeGreaterThan(0);
  });

  for (const testCase of fixture.cases) {
    it(`matches the engine's measured response for "${testCase.name}"`, () => {
      for (const point of testCase.points) {
        const db = channelResponseDb(testCase.channel, point.freq_hz, fixture.sample_rate);
        // Within 0.05dB: tight enough to catch a real formula divergence,
        // loose enough for residual f32 precision in the Rust engine's own
        // filter state (0.005dB failed on that alone -- see the fixture
        // generator's f64-Goertzel doc comment for the matching issue on
        // its measurement side).
        expect(
          db,
          `case ${testCase.name} @ ${point.freq_hz}Hz: ts=${db}dB rust=${point.db}dB`,
        ).toBeCloseTo(point.db, 1);
      }
    });
  }
});

describe("channelResponseDb neutral and stability", () => {
  const flatChannel: EqChannelParams = {
    cells: Array.from({ length: 6 }, () => ({
      on: false,
      cell_type: "Peak" as const,
      freq_hz: 1000,
      gain_db: 0,
      q: 1,
    })),
    trim_db: 0,
    delay_ms: 0,
  };

  it("an all-off, zero-trim channel is exactly flat at 0dB", () => {
    for (const freqHz of [20, 200, 1000, 5000, 20000]) {
      expect(channelResponseDb(flatChannel, freqHz, 48000)).toBe(0);
    }
  });

  it("random cell configurations never produce a non-finite response", () => {
    let seed = 12345;
    const rand = () => {
      seed = (seed * 1664525 + 1013904223) >>> 0;
      return seed / 0xffffffff;
    };
    const cellTypes: EqChannelParams["cells"][number]["cell_type"][] = [
      "Peak",
      "LowPass",
      "HighPass",
      "LowShelf",
      "HighShelf",
      "BandPass",
      "Notch",
    ];
    for (let i = 0; i < 2000; i++) {
      const channel: EqChannelParams = {
        cells: Array.from({ length: 6 }, (_, cellIndex) => ({
          on: rand() > 0.3,
          cell_type: cellTypes[Math.floor(rand() * cellTypes.length)]!,
          freq_hz: 20 + rand() * 19_980,
          gain_db: -36 + rand() * 54,
          q: 1 + rand() * 99,
        })),
        trim_db: -24 + rand() * 48,
        delay_ms: rand() * 500,
      };
      const freqHz = 20 + rand() * 19_980;
      const db = channelResponseDb(channel, freqHz, 48000);
      expect(Number.isFinite(db), `non-finite response at iteration ${i}`).toBe(true);
    }
  });
});
