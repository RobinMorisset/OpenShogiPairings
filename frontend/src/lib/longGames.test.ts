import { describe, expect, it } from "vitest";

import { inLongFlagUnit } from "./longGames";
import type { Board, Cup, Round } from "./types";
import { withRecord, type BoardFields } from "./boardFixture";

/**
 * Boards as the tests describe them — `{ outcome, long }` — folded into the
 * `record` sum the wire actually carries (see `GameRecord` in
 * `crates/core/src/round.rs`). Keeps the fixtures readable while the shape
 * under them is a tagged union.
 */
const board = (fields: BoardFields): Board =>
  ({ player1: 1, player2: 2, ...withRecord(fields) }) as Board;

const round = (number: number, boards: Board[]): Round =>
  ({ number, boards, completed: true }) as Round;

describe("inLongFlagUnit", () => {
  const cupBoard = (player1: number, player2: number, stage: string): Board =>
    ({ player1, player2, source: { kind: "cup", stage } }) as unknown as Board;
  const qualifierCup = {
    size: 8,
    format: "qualifier",
    seed_order: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
  } as unknown as Cup;

  it("takes in every cup board of the round, and nothing else", () => {
    const r = round(1, [cupBoard(1, 2, "quarterfinal"), board({ player1: 5, player2: 6 })]);
    expect(r.boards.map(inLongFlagUnit(r, null))).toEqual([true, false]);
  });

  // A qualifier cup's first round is one session: the play-off boards and the
  // pre-qualified players' games in the open go long together, or not at all.
  it("takes in the pre-qualified players' games in the qualification round", () => {
    const r = round(1, [
      cupBoard(5, 12, "qualification"),
      board({ player1: 1, player2: 20 }), // pre-qualified seed 1, facing an outsider
      board({ player1: 21, player2: 22 }), // an ordinary Swiss game
    ]);
    expect(r.boards.map(inLongFlagUnit(r, qualifierCup))).toEqual([true, true, false]);
  });

  it("leaves the pre-qualified alone once the bracket proper starts", () => {
    // No qualification board here, so this is a later round.
    const r = round(2, [cupBoard(1, 5, "quarterfinal"), board({ player1: 1, player2: 20 })]);
    expect(r.boards.map(inLongFlagUnit(r, qualifierCup))).toEqual([true, false]);
  });

  it("ignores the seeding of a direct cup", () => {
    const direct = { size: 8, format: "direct", seed_order: [1, 2, 3, 4, 5, 6, 7, 8] } as unknown as Cup;
    const r = round(1, [cupBoard(1, 8, "quarterfinal"), board({ player1: 2, player2: 20 })]);
    expect(r.boards.map(inLongFlagUnit(r, direct))).toEqual([true, false]);
  });
});
