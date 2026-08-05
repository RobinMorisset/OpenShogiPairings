// No-show bit-logic for a board's two sides.
//
// A board's `no_show` is a single value covering none / player1 / player2 /
// both, but the UI toggles each side independently. These helpers convert
// between the two per-side flags and that single value, so the two "didn't show
// up" buttons together cover all four states. Kept as a standalone module (out
// of RoundView) so the logic is unit-tested.

import type { Board, NoShow, Winner } from "./types";

/**
 * Whether a board's outcome is settled: a recorded result *or* a no-show.
 *
 * Mirrors `Board::is_decided` in `crates/core/src/round.rs`. The two fields are
 * mutually exclusive by construction — `set_board_no_show` clears `result` when
 * it sets `no_show`, because no game was played — so testing `result` alone
 * silently treats every forfeited board as still open. Server-side guards
 * (re-pairing a round, flagging a long game) are written against `is_decided`,
 * so any client guard that mirrors one has to use this, or it enables a control
 * whose request the server rejects.
 */
export function isDecided(board: Board): boolean {
  return board.result != null || board.no_show != null;
}

/**
 * Whether `side` is currently a no-show on this board — true for that side, and
 * for either side under `"both"`.
 */
export function absent(noShow: NoShow | null | undefined, side: Winner): boolean {
  return noShow === side || noShow === "both";
}

/** Combine the two per-side absence flags into the single no-show value. */
export function combineNoShow(p1: boolean, p2: boolean): NoShow | null {
  if (p1 && p2) return "both";
  if (p1) return "player1";
  if (p2) return "player2";
  return null;
}

/**
 * The new no-show value after flipping `side`'s absence, leaving the other side
 * unchanged — so toggling each button in turn reaches every state (none →
 * player1 → both → player2 → …).
 */
export function toggledNoShow(current: NoShow | null | undefined, side: Winner): NoShow | null {
  const p1 = side === "player1" ? !absent(current, "player1") : absent(current, "player1");
  const p2 = side === "player2" ? !absent(current, "player2") : absent(current, "player2");
  return combineNoShow(p1, p2);
}
