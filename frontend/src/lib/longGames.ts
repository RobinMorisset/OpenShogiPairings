// What the UI needs to know about "long" (two-round) games — see
// `docs/reference/two-round-boards.md`.
//
// A long board spans exactly two rounds: started in R, it is played while the
// rest of the field plays R and R+1, and its two players sit out R+1's pairing.
//
// Deliberately thin. Everything here is about *drawing* a board — is it long,
// does its checkbox move with its neighbours' — and nothing here decides
// anything the server also decides. The rules with consequences (who is kept out
// of a pairing, whether a round can start, whether the grid can export) arrive
// as answers on the response instead: `draft_long_players`,
// `next_round_blocked`, `grid_export_blocked`. Every predicate this file has
// held that duplicated a Rust one has eventually disagreed with it.

import type { Board, Cup, Round } from "./types";


/**
 * Whether this board is a long (two-round) game, in any of its states. Mirrors
 * `Board::is_long` in `crates/core/src/round.rs`.
 *
 * Presentation only — the checkbox's state and the badge on the board. Who a
 * long game keeps out of the next round is not derived here: the server ships
 * that as `draft_long_players`, because this file used to answer it too and
 * answered it wrongly (`LongEnd` is `is_long` as well, so the round *after* a
 * long game lost its players a second time).
 */
export function isLong(board: Board): boolean {
  return board.record !== undefined && board.record.kind !== "short";
}


/**
 * Which boards a long flag moves together with, in `round`. Mirrors
 * `in_cup_unit` in `Tournament::set_board_long` (`crates/core`).
 *
 * A cup bracket round is long or short as a *whole*, and in a qualifier cup's
 * qualification round that unit also takes in the pre-qualified players' games
 * in the open — they are playing the same session of the same cup. Everything
 * else toggles on its own.
 *
 * The UI needs this to grey the checkbox rather than let the referee tick a box
 * the server will refuse: a flip is refused once *any* game in the unit has a
 * result, not just the board clicked.
 */
export function inLongFlagUnit(round: Round, cup?: Cup | null): (board: Board) => boolean {
  const prequalified = prequalifiedInRound(round, cup);
  return (board) =>
    board.source?.kind === "cup" ||
    prequalified.has(board.player1) ||
    prequalified.has(board.player2);
}

/**
 * The pre-qualified players, but only in the round their qualification play-off
 * is being held — which is the one round their games are coupled to it. Empty
 * for a direct cup, and for every other round.
 */
function prequalifiedInRound(round: Round, cup?: Cup | null): Set<number> {
  if (!cup || cup.format !== "qualifier") return new Set();
  const isQualificationRound = round.boards.some(
    (b) => b.source?.kind === "cup" && b.source.stage === "qualification",
  );
  if (!isQualificationRound) return new Set();
  // `seed_order` is pre-qualified first, then the qualification field.
  return new Set(cup.seed_order.slice(0, Math.floor(cup.size / 2)));
}

