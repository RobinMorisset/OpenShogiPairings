// Board result helpers, shared by the Results cross-table and the round view.
//
// The "counts as a win for standings/pairing" (Wiel-rule-aware) outcome is
// computed server-side only — see `TournamentResponse.effective_winners` — so
// it isn't re-derived here. This file just resolves the plain, rule-agnostic
// facts about a board: what happened on it, who actually won, and who conceded
// the odds.

import type { Board, Forfeit, Outcome, Winner } from "./types";

/**
 * A board's outcome. The server omits the `record` entirely while the board is
 * an ordinary pending one (`skip_serializing_if` on `Board::record`), so that is
 * what a missing value means.
 *
 * A `long_carried` record holds no outcome at all — the live record of that game
 * is the `long_end` board in the following round — so it reads as pending, which
 * is also what it renders as (`0-`).
 *
 * Mirrors `Board::outcome` in `crates/core/src/round.rs`; the accessors below
 * are the TS twins of `Outcome`'s methods.
 */
export function outcomeOf(board: Board): Outcome {
  const record = board.record;
  if (!record || record.kind === "long_carried") return { kind: "pending" };
  return record.outcome;
}

/**
 * The *actual* winner of the game, ignoring the Wiel rule. `null` unless the
 * board was played to a decision — a forfeit has no winner here, only a
 * beneficiary (see `forfeitOf`).
 */
export function winnerOf(board: Board): Winner | null {
  const outcome = outcomeOf(board);
  return outcome.kind === "won" ? outcome.winner : null;
}

/** Whether a draw occurred before the decisive replay. Never true on a forfeit. */
export function drawnOf(board: Board): boolean {
  const outcome = outcomeOf(board);
  return outcome.kind === "forfeit" ? false : (outcome.drawn ?? false);
}

/** Which side(s) missed this board, and why, if it was forfeited. */
export function forfeitOf(board: Board): Forfeit | null {
  const outcome = outcomeOf(board);
  return outcome.kind === "forfeit" ? outcome.absent : null;
}

interface BoardOutcome {
  /** This side actually won the game — drives the +/− sign and win/loss colour. */
  actualWon: boolean;
  /** This side conceded the odds in a handicap game. */
  gave: boolean;
}

/** The plain (non-effective) outcome of a board from one side's perspective. */
export function boardOutcome(board: Board, side: Winner): BoardOutcome {
  const actualWon = winnerOf(board) === side;
  const gave = board.handicap?.giver === side;
  return { actualWon, gave };
}

/** The tournament number of the player who conceded the odds, or `null` if the
 *  board has no handicap. */
export function handicapGiverId(board: Board): number | null {
  if (!board.handicap) return null;
  return board.handicap.giver === "player1" ? board.player1 : board.player2;
}
