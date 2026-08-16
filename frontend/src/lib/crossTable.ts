// How a player's row of the cross-table is laid out across the round columns.
//
// Almost always one cell per round. The exception is a long game: it is *one*
// game played over two rounds, so it is drawn as one cell straddling both of
// their columns, showing the result once.
//
// This is separated from the rendering because it is the part that has been
// wrong twice. Both times the mistake was the same shape — deciding what a
// column shows by looking at a *neighbouring* round — and both times it was
// invisible in review because the answer is only wrong for boards no fixture
// could build. It is pure, so here it can simply be tested.

import type { Board, Round } from "./types";

/**
 * One column of a player's row.
 *
 * `span` is the `colspan` to render, and `readRound` indexes the round whose
 * board or sit-out supplies the content. They differ only for a long game,
 * where the cell starts in the column of the round the game *began* but its
 * content comes from the round it *finished* in — the only place the result
 * exists.
 */
export type CrossTableColumn =
  /** Absorbed by the previous column's straddling cell: draw no `<td>`. */
  | { span: 0 }
  /** An ordinary round column. */
  | { span: 1; readRound: number }
  /**
   * A long game, straddling the two rounds it was played over. `readRound` is
   * the later of them (holding the `long_end` record), `startRound` the earlier
   * (holding the inert `long_carried` one) — the pair the carry writes.
   */
  | { span: 2; readRound: number; startRound: number };

/** The board this player is on in `round`, if any. */
function boardOf(round: Round, player: number): Board | undefined {
  return round.boards.find((b) => b.player1 === player || b.player2 === player);
}

/**
 * The column layout of one player's row, one entry per round in `rounds`.
 *
 * A long game contributes a single `span: 2` entry at the round it began,
 * followed by a `span: 0` entry for the round it ended in. Every other round is
 * `span: 1`.
 *
 * Keyed on the `long_carried`/`long_end` **pair** rather than on "is this board
 * long?", which is what the previous version asked. A long game holds three
 * records over its life (`long_start` before the carry, then `long_carried` and
 * `long_end`), and only the last two are a two-round game that has a result to
 * show. Asking the looser question made the ending round match as well, so the
 * result was drawn in the round after the one it belonged to, displacing that
 * round's own content — or vanishing when the tournament ended there.
 *
 * An un-carried `long_start` gets an ordinary single column: it occupies only
 * one round that exists so far, and the round that would complete it has not
 * been paired. It draws its own state, which is honest whether the game is
 * decided or still running.
 */
export function crossTableColumns(rounds: Round[], player: number): CrossTableColumn[] {
  const columns: CrossTableColumn[] = [];
  let i = 0;
  while (i < rounds.length) {
    const carried = boardOf(rounds[i], player);
    const next = i + 1 < rounds.length ? boardOf(rounds[i + 1], player) : undefined;
    if (
      carried?.record?.kind === "long_carried" &&
      next?.record?.kind === "long_end"
    ) {
      columns.push({ span: 2, readRound: i + 1, startRound: i });
      columns.push({ span: 0 });
      i += 2;
      continue;
    }
    columns.push({ span: 1, readRound: i });
    i += 1;
  }
  return columns;
}
