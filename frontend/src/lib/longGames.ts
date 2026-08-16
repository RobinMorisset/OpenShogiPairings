// Rules for "long" (two-round) games — see `docs/archive/two-round-boards.md`.
//
// A long board spans exactly two rounds: started in R, it is played while the
// rest of the field plays R and R+1, and its two players sit out R+1's pairing.
// That makes "pending" a state the UI has to reason about in several places, so
// the predicate lives here once instead of being spelled out at each call site.

import type { Board, Cup, Round } from "./types";


/**
 * Whether this board is a long (two-round) game, in any of its states. Mirrors
 * `Board::is_long` in `crates/core/src/round.rs`.
 *
 * This — not {@link longPending} — is what decides who sits out the next round,
 * because a long game occupies two rounds whichever round it finishes in.
 */
export function isLong(board: Board): boolean {
  return board.record !== undefined && board.record.kind !== "short";
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
export function busyOnLongGame(rounds: Round[], roundNumber: number): number[] {
  const previous = rounds.find((r) => r.number + 1 === roundNumber);
  if (!previous) return [];
  return previous.boards
    .filter(isLong)
    .flatMap((b) => [b.player1, b.player2]);
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

