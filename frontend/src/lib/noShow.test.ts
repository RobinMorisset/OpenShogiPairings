import { describe, it, expect } from "vitest";
import { winnerOf } from "./boardOutcome";
import { absent, combineNoShow, isDecided, toggledNoShow } from "./noShow";
import type { Board } from "./types";

describe("absent", () => {
  it("is true only for the named side", () => {
    expect(absent("player1", "player1")).toBe(true);
    expect(absent("player1", "player2")).toBe(false);
    expect(absent("player2", "player2")).toBe(true);
    expect(absent("player2", "player1")).toBe(false);
  });

  it("is true for both sides under `both`", () => {
    expect(absent("both", "player1")).toBe(true);
    expect(absent("both", "player2")).toBe(true);
  });

  it("is false when there is no no-show", () => {
    expect(absent(null, "player1")).toBe(false);
    expect(absent(undefined, "player2")).toBe(false);
  });
});

describe("combineNoShow", () => {
  it("maps the four flag combinations", () => {
    expect(combineNoShow(false, false)).toBe(null);
    expect(combineNoShow(true, false)).toBe("player1");
    expect(combineNoShow(false, true)).toBe("player2");
    expect(combineNoShow(true, true)).toBe("both");
  });
});

describe("toggledNoShow", () => {
  it("cycles a single side on and off", () => {
    expect(toggledNoShow(null, "player1")).toBe("player1");
    expect(toggledNoShow("player1", "player1")).toBe(null);
    expect(toggledNoShow(null, "player2")).toBe("player2");
    expect(toggledNoShow("player2", "player2")).toBe(null);
  });

  it("adds and removes the other side independently, reaching `both`", () => {
    // player1 already absent, now player2 fails to show too → both.
    expect(toggledNoShow("player1", "player2")).toBe("both");
    // From both, clearing one side leaves the other.
    expect(toggledNoShow("both", "player1")).toBe("player2");
    expect(toggledNoShow("both", "player2")).toBe("player1");
  });

  it("is its own inverse for a given side (toggle twice = no change)", () => {
    for (const start of ["player1", "player2", "both", null] as const) {
      for (const side of ["player1", "player2"] as const) {
        expect(toggledNoShow(toggledNoShow(start, side), side)).toBe(start ?? null);
      }
    }
  });
});

describe("isDecided", () => {
  // Only the fields the predicate reads; the rest of `Board` is irrelevant here.
  const board = (fields: Partial<Board>): Board =>
    ({ player1: 1, player2: 2, ...fields }) as Board;

  // The server omits `outcome` entirely while the board is an ordinary pending
  // one, so a missing field has to read as pending, not as an error.
  it("is false for a board nobody has played yet", () => {
    expect(isDecided(board({}))).toBe(false);
    expect(isDecided(board({ outcome: { kind: "pending" } }))).toBe(false);
    expect(isDecided(board({ outcome: { kind: "pending", drawn: true } }))).toBe(false);
  });

  it("is true once a result is recorded", () => {
    expect(isDecided(board({ outcome: { kind: "won", winner: "player1" } }))).toBe(true);
    expect(isDecided(board({ outcome: { kind: "won", winner: "player2" } }))).toBe(true);
  });

  // The regression: a forfeited board carries no winner, so a winner-only check
  // would call it undecided — leaving "force this pairing" enabled on a request
  // the server rejects with `round_has_results`.
  it("is true for a no-show even though no winner is recorded", () => {
    for (const side of ["player1", "player2", "both"] as const) {
      const b = board({ outcome: { kind: "forfeit", absent: side } });
      expect(winnerOf(b)).toBeNull();
      expect(isDecided(b)).toBe(true);
    }
  });

  it("matches the server: a round is re-pairable only if no board is decided", () => {
    // The confirmed repro — two boards, one forfeited, one unplayed.
    const boards = [board({ outcome: { kind: "forfeit", absent: "player1" } }), board({})];
    expect(boards.some(isDecided)).toBe(true);
  });
});
