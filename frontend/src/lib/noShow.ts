// Forfeit bit-logic for a board's two sides.
//
// A board's forfeit is a single value covering which side(s) missed it and why,
// but the UI works one side at a time. These helpers convert between the two
// per-side reasons and that single value, so the two "absent" controls together
// cover every state. Kept as a standalone module (out of RoundView) so the logic
// is unit-tested.
//
// Mirrors `Forfeit` in `crates/core/src/round.rs`.

import { outcomeOf } from "./boardOutcome";
import type { AbsenceKind, Board, Forfeit, Winner } from "./types";

/**
 * Whether a board's outcome is settled: a recorded result *or* a forfeit.
 *
 * Mirrors `Board::is_decided`. A forfeited board carries no winner — the two are
 * variants of one sum — so testing for a winner alone silently treats every
 * forfeited board as still open. Server-side guards (re-pairing a round,
 * flagging a long game) are written against `is_decided`, so any client guard
 * that mirrors one has to use this, or it enables a control whose request the
 * server rejects.
 */
export function isDecided(board: Board): boolean {
  return outcomeOf(board).kind !== "pending";
}

/** Why `side` missed the board, or `null` if they turned up. */
export function absenceKind(
  forfeit: Forfeit | null | undefined,
  side: Winner,
): AbsenceKind | null {
  if (!forfeit) return null;
  if ("both" in forfeit) return forfeit.both[side === "player1" ? 0 : 1];
  if (side === "player1") return "player1" in forfeit ? forfeit.player1 : null;
  return "player2" in forfeit ? forfeit.player2 : null;
}

/**
 * Whether `side` missed this board — true for that side, and for both sides
 * under `both`.
 */
export function absent(forfeit: Forfeit | null | undefined, side: Winner): boolean {
  return absenceKind(forfeit, side) != null;
}

/** Combine the two per-side reasons into the single forfeit value, `null` when
 *  both sides turned up (which is not a forfeit at all). Mirrors `Forfeit::of`. */
export function combineForfeit(
  player1: AbsenceKind | null,
  player2: AbsenceKind | null,
): Forfeit | null {
  if (player1 && player2) return { both: [player1, player2] };
  if (player1) return { player1 };
  if (player2) return { player2 };
  return null;
}

/**
 * The forfeit after cycling `side` one step, leaving the other side untouched.
 *
 * One control per side, cycling through the states that side can be in: present
 * → didn't turn up → (in team mode only) absent for a reason → present. Team
 * mode is where the third state exists at all — an individual tournament
 * excludes an absent player before a board exists, so it never has a justified
 * absence to record, and the server rejects one.
 */
export function cycledForfeit(
  current: Forfeit | null | undefined,
  side: Winner,
  allowJustified: boolean,
): Forfeit | null {
  const next = (kind: AbsenceKind | null): AbsenceKind | null => {
    if (kind == null) return "no_show";
    if (kind === "no_show") return allowJustified ? "justified" : null;
    return null;
  };
  const p1 = absenceKind(current, "player1");
  const p2 = absenceKind(current, "player2");
  return side === "player1"
    ? combineForfeit(next(p1), p2)
    : combineForfeit(p1, next(p2));
}
