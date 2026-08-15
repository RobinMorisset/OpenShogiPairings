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
 * A long board that was decided early — finished ahead of time, or resolved by
 * forfeit — is *not* pending, and its players are free again.
 */
export function longPending(board: Board): boolean {
  return board.long === true && !isDecided(board);
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
