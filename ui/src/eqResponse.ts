// The parametric EQ's frequency-response math (spec 1.7, M6), independent
// of the Rust engine (`crates/loomix-core/src/biquad.rs` /
// `parametric_eq.rs`) — same RBJ/Audio EQ Cookbook formulas, reimplemented
// here rather than shared, since the UI and the engine are separate
// language runtimes. `eqResponse.test.ts` checks this against a fixture
// generated *from* the Rust engine
// (`testdata/fixtures/eq_response_reference.json`,
// `crates/loomix-core/tests/eq_response_fixture.rs`) so this file can't
// silently drift from what the engine actually does — a graph that lies
// about the engine's own response is worse than no graph.
//
// This computes the *analytic* steady-state magnitude response
// (evaluating each biquad's transfer function H(e^jw) directly), not a
// time-domain simulation — mathematically the same curve a long enough
// steady tone converges to, and standard for a frequency-response graph.
// Delay is intentionally not reflected here: a pure delay is all-pass (it
// changes phase, not magnitude), so it never appears on an EQ's dB curve,
// on the real engine's ramped filter or this one.

export type EqCellType =
  | "Peak"
  | "LowPass"
  | "HighPass"
  | "LowShelf"
  | "HighShelf"
  | "BandPass"
  | "Notch";

export interface EqCellParams {
  on: boolean;
  cell_type: EqCellType;
  freq_hz: number;
  gain_db: number;
  q: number;
}

export interface EqChannelParams {
  cells: EqCellParams[];
  trim_db: number;
  delay_ms: number;
}

interface BiquadCoeffs {
  b0: number;
  b1: number;
  b2: number;
  a1: number;
  a2: number;
}

function peaking(sampleRate: number, freqHz: number, q: number, gainDb: number): BiquadCoeffs {
  const a = 10 ** (gainDb / 40);
  const w0 = (2 * Math.PI * freqHz) / sampleRate;
  const alpha = Math.sin(w0) / (2 * q);
  const cosW0 = Math.cos(w0);
  const a0 = 1 + alpha / a;
  return {
    b0: (1 + alpha * a) / a0,
    b1: (-2 * cosW0) / a0,
    b2: (1 - alpha * a) / a0,
    a1: (-2 * cosW0) / a0,
    a2: (1 - alpha / a) / a0,
  };
}

function bandPass(sampleRate: number, freqHz: number, q: number): BiquadCoeffs {
  const w0 = (2 * Math.PI * freqHz) / sampleRate;
  const alpha = Math.sin(w0) / (2 * q);
  const cosW0 = Math.cos(w0);
  const a0 = 1 + alpha;
  return {
    b0: alpha / a0,
    b1: 0,
    b2: -alpha / a0,
    a1: (-2 * cosW0) / a0,
    a2: (1 - alpha) / a0,
  };
}

function notch(sampleRate: number, freqHz: number, q: number): BiquadCoeffs {
  const w0 = (2 * Math.PI * freqHz) / sampleRate;
  const alpha = Math.sin(w0) / (2 * q);
  const cosW0 = Math.cos(w0);
  const a0 = 1 + alpha;
  return {
    b0: 1 / a0,
    b1: (-2 * cosW0) / a0,
    b2: 1 / a0,
    a1: (-2 * cosW0) / a0,
    a2: (1 - alpha) / a0,
  };
}

function lowPass(sampleRate: number, freqHz: number, q: number): BiquadCoeffs {
  const w0 = (2 * Math.PI * freqHz) / sampleRate;
  const alpha = Math.sin(w0) / (2 * q);
  const cosW0 = Math.cos(w0);
  const a0 = 1 + alpha;
  return {
    b0: (1 - cosW0) / 2 / a0,
    b1: (1 - cosW0) / a0,
    b2: (1 - cosW0) / 2 / a0,
    a1: (-2 * cosW0) / a0,
    a2: (1 - alpha) / a0,
  };
}

function highPass(sampleRate: number, freqHz: number, q: number): BiquadCoeffs {
  const w0 = (2 * Math.PI * freqHz) / sampleRate;
  const alpha = Math.sin(w0) / (2 * q);
  const cosW0 = Math.cos(w0);
  const a0 = 1 + alpha;
  return {
    b0: (1 + cosW0) / 2 / a0,
    b1: -(1 + cosW0) / a0,
    b2: (1 + cosW0) / 2 / a0,
    a1: (-2 * cosW0) / a0,
    a2: (1 - alpha) / a0,
  };
}

function lowShelf(sampleRate: number, freqHz: number, q: number, gainDb: number): BiquadCoeffs {
  const a = 10 ** (gainDb / 40);
  const w0 = (2 * Math.PI * freqHz) / sampleRate;
  const alpha = Math.sin(w0) / (2 * q);
  const cosW0 = Math.cos(w0);
  const sqrtA = Math.sqrt(a);
  const a0 = a + 1 + (a - 1) * cosW0 + 2 * sqrtA * alpha;
  return {
    b0: (a * (a + 1 - (a - 1) * cosW0 + 2 * sqrtA * alpha)) / a0,
    b1: (2 * a * (a - 1 - (a + 1) * cosW0)) / a0,
    b2: (a * (a + 1 - (a - 1) * cosW0 - 2 * sqrtA * alpha)) / a0,
    a1: (-2 * (a - 1 + (a + 1) * cosW0)) / a0,
    a2: (a + 1 + (a - 1) * cosW0 - 2 * sqrtA * alpha) / a0,
  };
}

function highShelf(sampleRate: number, freqHz: number, q: number, gainDb: number): BiquadCoeffs {
  const a = 10 ** (gainDb / 40);
  const w0 = (2 * Math.PI * freqHz) / sampleRate;
  const alpha = Math.sin(w0) / (2 * q);
  const cosW0 = Math.cos(w0);
  const sqrtA = Math.sqrt(a);
  const a0 = a + 1 - (a - 1) * cosW0 + 2 * sqrtA * alpha;
  return {
    b0: (a * (a + 1 + (a - 1) * cosW0 + 2 * sqrtA * alpha)) / a0,
    b1: (-2 * a * (a - 1 + (a + 1) * cosW0)) / a0,
    b2: (a * (a + 1 + (a - 1) * cosW0 - 2 * sqrtA * alpha)) / a0,
    a1: (2 * (a - 1 - (a + 1) * cosW0)) / a0,
    a2: (a + 1 - (a - 1) * cosW0 - 2 * sqrtA * alpha) / a0,
  };
}

function coeffsFor(cell: EqCellParams, sampleRate: number): BiquadCoeffs {
  switch (cell.cell_type) {
    case "Peak":
      return peaking(sampleRate, cell.freq_hz, cell.q, cell.gain_db);
    case "LowPass":
      return lowPass(sampleRate, cell.freq_hz, cell.q);
    case "HighPass":
      return highPass(sampleRate, cell.freq_hz, cell.q);
    case "LowShelf":
      return lowShelf(sampleRate, cell.freq_hz, cell.q, cell.gain_db);
    case "HighShelf":
      return highShelf(sampleRate, cell.freq_hz, cell.q, cell.gain_db);
    case "BandPass":
      return bandPass(sampleRate, cell.freq_hz, cell.q);
    case "Notch":
      return notch(sampleRate, cell.freq_hz, cell.q);
  }
}

/** `|H(e^jw)|` in dB for one biquad at `freqHz`. */
function cellMagnitudeDb(coeffs: BiquadCoeffs, freqHz: number, sampleRate: number): number {
  const w = (2 * Math.PI * freqHz) / sampleRate;
  // e^-jw = cos(w) - j sin(w); e^-2jw = cos(2w) - j sin(2w).
  const cos1 = Math.cos(w);
  const sin1 = -Math.sin(w);
  const cos2 = Math.cos(2 * w);
  const sin2 = -Math.sin(2 * w);

  const numRe = coeffs.b0 + coeffs.b1 * cos1 + coeffs.b2 * cos2;
  const numIm = coeffs.b1 * sin1 + coeffs.b2 * sin2;
  const denRe = 1 + coeffs.a1 * cos1 + coeffs.a2 * cos2;
  const denIm = coeffs.a1 * sin1 + coeffs.a2 * sin2;

  const numMag = Math.hypot(numRe, numIm);
  const denMag = Math.hypot(denRe, denIm);
  return 20 * Math.log10(numMag / denMag);
}

/**
 * The channel's total response at `freqHz`, dB: every `on` cell's
 * magnitude in series (dB adds under cascade) plus trim (a flat gain,
 * frequency-independent). Delay is not reflected — see the file doc
 * comment.
 */
export function channelResponseDb(
  channel: EqChannelParams,
  freqHz: number,
  sampleRate: number,
): number {
  let totalDb = channel.trim_db;
  for (const cell of channel.cells) {
    if (!cell.on) continue;
    totalDb += cellMagnitudeDb(coeffsFor(cell, sampleRate), freqHz, sampleRate);
  }
  return totalDb;
}

export interface ResponsePoint {
  freqHz: number;
  db: number;
}

/** The full curve for `channel` across `freqsHz`, in order. */
export function computeResponseCurve(
  channel: EqChannelParams,
  freqsHz: readonly number[],
  sampleRate: number,
): ResponsePoint[] {
  return freqsHz.map((freqHz) => ({ freqHz, db: channelResponseDb(channel, freqHz, sampleRate) }));
}
