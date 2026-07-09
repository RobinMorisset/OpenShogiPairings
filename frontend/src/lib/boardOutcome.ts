// Board result helpers, shared by the Results cross-table and the round view.
//
// The "counts as a win for standings/pairing" (Wiel-rule-aware) outcome is
// computed server-side only — see `TournamentResponse.effective_winners` — so
// it isn't re-derived here. This file just resolves the plain, rule-agnostic
// facts about a board: who actually won, and who conceded the odds.

import type { Board, Winner } from "./types";

export interface BoardOutcome {
  /** This side actually won the game — drives the +/− sign and win/loss colour. */
  actualWon: boolean;
  /** This side conceded the odds in a handicap game. */
  gave: boolean;
}

/** The plain (non-effective) outcome of a board from one side's perspective. */
export function boardOutcome(board: Board, side: Winner): BoardOutcome {
  const actualWon = board.result === side;
  const gave = board.handicap?.giver === side;
  return { actualWon, gave };
}

/** The id of the player who conceded the odds, or `null` if the board has no
 *  handicap. */
export function handicapGiverId(board: Board): string | null {
  if (!board.handicap) return null;
  return board.handicap.giver === "player1" ? board.player1 : board.player2;
}
