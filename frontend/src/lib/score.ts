// Half-point score formatting.
//
// osp-core keeps every point-like quantity (a player's total points, MacMahon
// start, and the MacMahon-inclusive `…M` tie-breaks) in **half-point units**
// (×2), so a half-point absence (`0=`) stays an exact integer on the wire. The
// UI divides by two and renders the fraction with a ½ glyph. The wins-only
// (`…W`) tie-breaks, `victories`, `dc` and the ELO estimate are whole counts and
// are shown as-is.

import type { Tiebreak } from "./types";

/**
 * Render a score held in half-point units: `0`, `½`, `1`, `1½`, `2`, … So `0`→
 * `"0"`, `1`→`"½"`, `2`→`"1"`, `7`→`"3½"`.
 */
export function formatScore(halfUnits: number): string {
  const whole = Math.floor(halfUnits / 2);
  const half = halfUnits % 2 !== 0;
  if (!half) return String(whole);
  return whole === 0 ? "½" : `${whole}½`;
}

/**
 * The tie-break codes whose value is in half-point units (so it must go through
 * {@link formatScore}). The `…W` flavours, `dc` and `est_elo` are whole and are
 * shown as plain integers.
 */
export const HALF_POINT_TIEBREAKS: ReadonlySet<Tiebreak> = new Set<Tiebreak>([
  "points",
  "sos_m",
  "sodos_m",
  "sosos_m",
  "sos_m1",
  "sos_m2",
  "cuss_m",
]);
