// Half-point score formatting.
//
// osp-core keeps every score-like quantity in **half units** (×2), so a `0=`
// stays an exact integer on the wire: points, the MacMahon start and the `…M`
// tie-breaks in half-*points*, and wins and everything summed from them
// (`victories`, the `…W` tie-breaks, `dc`) in half-*wins*. A `0=` is worth half
// of each, which is why both scales are doubled — a whole win count would have
// to round it, and the Wins column would then disagree with the Points column
// beside it. The UI divides by two and renders the fraction with a ½ glyph.
//
// The ELO estimate is the one metric that is not on this scale: it is a rating,
// not a score, and is shown as-is.

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
 * The tie-break codes whose value is in half units, so it must go through
 * {@link formatScore} — which is every criterion except `est_elo`, the one that
 * is a rating rather than a score.
 *
 * Kept as an explicit list rather than "everything but `est_elo`": a new
 * criterion should have to say which it is, and the cost of getting it wrong is
 * a column that silently reads double.
 */
export const HALF_POINT_TIEBREAKS: ReadonlySet<Tiebreak> = new Set<Tiebreak>([
  "points",
  "sos_m",
  "sos_w",
  "sodos_m",
  "sodos_w",
  "sosos_m",
  "sosos_w",
  "sos_m1",
  "sos_m2",
  "sos_w1",
  "sos_w2",
  "cuss_m",
  "cuss_w",
  "dc",
  "board_wins",
]);
