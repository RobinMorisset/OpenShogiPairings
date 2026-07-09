// Board result helpers, shared by the Results cross-table and the round view so
// the (subtly handicap-aware) "did this side win / who conceded the odds" logic
// lives in exactly one place.

import type { Board, Winner } from "./types";

export interface BoardOutcome {
  /** This side actually won the game — drives the +/− sign and win/loss colour. */
  actualWon: boolean;
  /** This side counts as the winner in the standings. With the "Wiel" rule on,
   *  a handicap game always credits the giver, whoever actually won; otherwise
   *  it is the actual winner. */
  effectiveWon: boolean;
  /** This side conceded the odds in a handicap game. */
  gave: boolean;
}

/**
 * The outcome of a board from one side's perspective. On an unplayed board
 * `actualWon`/`effectiveWon` are both false (`gave` still reflects the frozen
 * giver, if any). `wielRule` mirrors the server's
 * `TournamentSettings.handicap_wiel_rule`.
 */
export function boardOutcome(board: Board, side: Winner, wielRule: boolean): BoardOutcome {
  const decided = board.result != null;
  const actualWon = board.result === side;
  const gave = board.handicap?.giver === side;
  const effectiveWon = board.handicap && wielRule ? decided && gave : actualWon;
  return { actualWon, effectiveWon, gave };
}

/** The id of the player who conceded the odds, or `null` if the board has no
 *  handicap. */
export function handicapGiverId(board: Board): string | null {
  if (!board.handicap) return null;
  return board.handicap.giver === "player1" ? board.player1 : board.player2;
}
