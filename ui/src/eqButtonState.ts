// Split out of `EqPanel.tsx` (not a component itself, and a component
// file may only export components -- `react-refresh/only-export-
// components`) so both `App.tsx`'s glance indicator and `EqPanel.tsx`
// itself can share one definition.

import type { EqChannelParams } from "./bridge";

/**
 * Spec 1.5's own button-colour convention ("green equalized or gain
 * changed, red delayed, yellow both, black untouched"), extended to strip
 * EQ too (M10, `docs/ARCHITECTURE.md`) since both share the identical
 * `ParametricEq<N>` engine and both snapshot the same `trim_db`/`delay_ms`
 * fields. Aggregated across every channel a single button represents (2
 * for a strip, 8 for a bus): a genuine per-channel readout would need a
 * button per channel, which spec 1.5's single EQ toggle doesn't have room
 * for -- "any channel carries this state" is the honest reduction of that
 * convention onto one button, not an invented one.
 */
export type EqButtonState = "off" | "neutral" | "eq" | "delay" | "both";

export function eqButtonState(on: boolean, channels: readonly EqChannelParams[]): EqButtonState {
  if (!on) return "off";
  const eqChanged = channels.some((c) => c.trim_db !== 0 || c.cells.some((cell) => cell.on));
  const delayed = channels.some((c) => c.delay_ms !== 0);
  if (eqChanged && delayed) return "both";
  if (delayed) return "delay";
  if (eqChanged) return "eq";
  return "neutral";
}
