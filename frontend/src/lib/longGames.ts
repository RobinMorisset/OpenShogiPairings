// Rules for "long" (two-round) games — see `docs/archive/two-round-boards.md`.
//
// A long board spans exactly two rounds: started in R, it is played while the
// rest of the field plays R and R+1, and its two players sit out R+1's pairing.
// That makes "pending" a state the UI has to reason about in several places, so
// the predicate lives here once instead of being spelled out at each call site.

import { isDecided } from "./noShow";
import type { Board, Round } from "./types";

/**
 * A long board whose game hasn't finished yet. Mirrors `Board::long_pending` in
 * `crates/core/src/round.rs`.
 *
 * Use this for "is there still a result to record" — the carried-games widget,
 * the `0-` placeholder, the overrun guard below. It is *not* what decides who
 * sits out the next round: see {@link busyOnLongGame}.
 */
export function longPending(board: Board): boolean {
  return board.long === true && !isDecided(board);
}

/**
 * The players a long game keeps out of round `roundNumber`'s pairing — those on
 * a long board in the immediately preceding round. Mirrors `busy_long` in
 * `confirm_round_inner` (`crates/core/src/tournament.rs`).
 *
 * Keyed on `long`, not {@link longPending}: a long game is one game played
 * across two rounds — which is why its winner scores two points — so it occupies
 * both rounds whichever round it actually finishes in. A referee who wants those
 * players paired here unticks the box before the round advances, demoting the
 * board to an ordinary one-point game.
 *
 * Only the previous round is scanned, matching the server: a long game can never
 * be two rounds behind and unresolved, because `prepare_round` refuses to advance
 * past R+1 while one is pending.
 */
/**
 * The round holding *any* still-unresolved long game, or `null` if none is.
 * Mirrors the guard in `american_grid::to_grid` (`crates/core`).
 *
 * Wider than {@link overrunLongRound}, deliberately: a long game that has not
 * overrun still has no result, and a long board carries its result into the
 * *next* round's column — so exporting now would write a loss for both players
 * into a document bound for a rating body. Any pending long game blocks the
 * export, not just a late one.
 */
export function pendingLongRound(rounds: Round[]): number | null {
  const round = rounds.find((r) => r.boards.some(longPending));
  return round?.number ?? null;
}

export function busyOnLongGame(rounds: Round[], roundNumber: number): number[] {
  const previous = rounds.find((r) => r.number + 1 === roundNumber);
  if (!previous) return [];
  return previous.boards
    .filter((b) => b.long === true)
    .flatMap((b) => [b.player1, b.player2]);
}

/**
 * The round holding a long game that has overrun, or `null` if none has.
 *
 * Mirrors the guard in `prepare_round` (`crates/core/src/tournament.rs`): a long
 * game may straddle its own round and the next, so preparing round N is blocked
 * by a pending long board in round N-2 or earlier — never by one in N-1, which
 * is still legitimately in flight.
 *
 * `nextRoundNumber` is the round about to be prepared, i.e. `rounds.length + 1`.
 */
export function overrunLongRound(rounds: Round[], nextRoundNumber: number): number | null {
  const stale = rounds.find(
    (r) => r.number + 1 < nextRoundNumber && r.boards.some(longPending),
  );
  return stale?.number ?? null;
}
