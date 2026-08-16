import { describe, expect, it } from "vitest";

import { crossTableColumns } from "./crossTable";
import { carriedLongGame, withRecord, type BoardFields } from "./boardFixture";
import type { Board, Round } from "./types";

const board = (fields: BoardFields): Board =>
  ({ player1: 1, player2: 2, ...withRecord(fields) }) as Board;

const round = (number: number, boards: Partial<Board>[]): Round =>
  ({ number, boards: boards as Board[], sitouts: [], completed: true }) as unknown as Round;

/** The two halves of one long game, ready to drop into consecutive rounds. */
const carried = (fields: BoardFields = {}) =>
  carriedLongGame({ player1: 1, player2: 2, ...fields });

describe("crossTableColumns", () => {
  it("gives an ordinary round one column each", () => {
    const rounds = [round(1, [board({})]), round(2, [board({})])];
    expect(crossTableColumns(rounds, 1)).toEqual([
      { span: 1, readRound: 0 },
      { span: 1, readRound: 1 },
    ]);
  });

  // The point of the feature: one game played over two rounds is drawn once,
  // in a cell covering both of their columns, reading the round that actually
  // holds the result.
  it("straddles the two rounds a carried long game was played over", () => {
    const { started, ended } = carried({ outcome: { kind: "won", winner: "player1" } });
    const rounds = [round(1, [started]), round(2, [ended])];
    expect(crossTableColumns(rounds, 1)).toEqual([
      { span: 2, readRound: 1, startRound: 0 },
      { span: 0 },
    ]);
  });

  // The bug this replaces: keying on "is this board long?" made the *ending*
  // round match too, so the result was drawn one column too far and displaced
  // whatever really belonged there.
  it("does not spill into the round after the game ended", () => {
    const { started, ended } = carried({ outcome: { kind: "won", winner: "player1" } });
    const rounds = [round(1, [started]), round(2, [ended]), round(3, [board({})])];
    expect(crossTableColumns(rounds, 1)).toEqual([
      { span: 2, readRound: 1, startRound: 0 },
      { span: 0 },
      { span: 1, readRound: 2 },
    ]);
  });

  // A long game still in its second round has no result yet, but it is still one
  // game over two rounds — the straddle is about the rounds, not the outcome.
  it("straddles a long game that has not been decided yet", () => {
    const { started, ended } = carried();
    const rounds = [round(1, [started]), round(2, [ended])];
    expect(crossTableColumns(rounds, 1)).toEqual([
      { span: 2, readRound: 1, startRound: 0 },
      { span: 0 },
    ]);
  });

  // Before the carry there is only one round to draw, so there is nothing to
  // straddle: the tournament ended on it, or the next round is not paired yet.
  it("leaves an un-carried long game in its own column", () => {
    const rounds = [round(1, [board({})]), round(2, [board({ long: true })])];
    expect(crossTableColumns(rounds, 1)).toEqual([
      { span: 1, readRound: 0 },
      { span: 1, readRound: 1 },
    ]);
  });

  it("handles back-to-back long games", () => {
    const first = carried({ outcome: { kind: "won", winner: "player1" } });
    const second = carried({ outcome: { kind: "won", winner: "player2" } });
    const rounds = [
      round(1, [first.started]),
      round(2, [first.ended]),
      round(3, [second.started]),
      round(4, [second.ended]),
    ];
    expect(crossTableColumns(rounds, 1)).toEqual([
      { span: 2, readRound: 1, startRound: 0 },
      { span: 0 },
      { span: 2, readRound: 3, startRound: 2 },
      { span: 0 },
    ]);
  });

  // The straddle belongs to the two players of that board and nobody else: the
  // rest of the field plays both rounds normally, which is the whole feature.
  it("leaves the other players' rows alone", () => {
    const { started, ended } = carried();
    const rounds = [
      round(1, [started, board({ player1: 3, player2: 4 })]),
      round(2, [ended, board({ player1: 3, player2: 4 })]),
    ];
    expect(crossTableColumns(rounds, 3)).toEqual([
      { span: 1, readRound: 0 },
      { span: 1, readRound: 1 },
    ]);
  });

  it("copes with a player who is in no round at all", () => {
    const rounds = [round(1, [board({})])];
    expect(crossTableColumns(rounds, 99)).toEqual([{ span: 1, readRound: 0 }]);
    expect(crossTableColumns([], 1)).toEqual([]);
  });
});
