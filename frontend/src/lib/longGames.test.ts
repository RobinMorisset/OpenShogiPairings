import { describe, expect, it } from "vitest";

import { longPending, overrunLongRound } from "./longGames";
import type { Board, Round } from "./types";

/** Only the fields these predicates read; the rest of `Board` is irrelevant. */
const board = (fields: Partial<Board>): Board =>
  ({ player1: 1, player2: 2, ...fields }) as Board;

const round = (number: number, boards: Board[]): Round =>
  ({ number, boards, completed: true }) as Round;

const pendingLong = () => board({ long: true });

describe("longPending", () => {
  it("is true for a long board with no outcome yet", () => {
    expect(longPending(pendingLong())).toBe(true);
  });

  it("is false for an ordinary unplayed board", () => {
    expect(longPending(board({}))).toBe(false);
    expect(longPending(board({ long: false }))).toBe(false);
  });

  it("is false once the long game is decided", () => {
    expect(longPending(board({ long: true, result: "player1" }))).toBe(false);
  });

  // A forfeited long board frees its players just like a played one — the same
  // `result`-is-null-but-decided case that the force-pairing guard got wrong.
  it("is false for a long board resolved by forfeit", () => {
    const forfeited = board({ long: true, result: null, no_show: "player2" });
    expect(forfeited.result).toBeNull();
    expect(longPending(forfeited)).toBe(false);
  });
});

describe("overrunLongRound", () => {
  it("is null when there are no long games at all", () => {
    const rounds = [round(1, [board({})]), round(2, [board({})])];
    expect(overrunLongRound(rounds, 3)).toBeNull();
  });

  // The boundary the rule is built around: a long game started in round 2 is
  // *meant* to still be open while round 3 is prepared — it spans 2 and 3.
  it("does not block the round a long game legitimately spans", () => {
    const rounds = [round(1, [board({})]), round(2, [pendingLong()])];
    expect(overrunLongRound(rounds, 3)).toBeNull();
  });

  // One round later, the same board has overrun and must be resolved.
  it("blocks once that long game would straddle a third round", () => {
    const rounds = [round(1, [board({})]), round(2, [pendingLong()]), round(3, [board({})])];
    expect(overrunLongRound(rounds, 4)).toBe(2);
  });

  it("names the earliest offending round when several have overrun", () => {
    const rounds = [round(1, [pendingLong()]), round(2, [pendingLong()]), round(3, [board({})])];
    expect(overrunLongRound(rounds, 4)).toBe(1);
  });

  it("ignores a long game that was finished in time", () => {
    const rounds = [
      round(1, [board({ long: true, result: "player1" })]),
      round(2, [board({})]),
      round(3, [board({})]),
    ];
    expect(overrunLongRound(rounds, 4)).toBeNull();
  });

  it("is null before any round exists", () => {
    expect(overrunLongRound([], 1)).toBeNull();
  });
});
