import { describe, it, expect } from "vitest";
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

  it("is false for a board nobody has played yet", () => {
    expect(isDecided(board({}))).toBe(false);
    expect(isDecided(board({ result: null, no_show: null }))).toBe(false);
  });

  it("is true once a result is recorded", () => {
    expect(isDecided(board({ result: "player1" }))).toBe(true);
    expect(isDecided(board({ result: "player2" }))).toBe(true);
  });

  // The regression: `set_board_no_show` clears `result` when it sets `no_show`,
  // so a forfeited board has `result == null` and a `result`-only check would
  // call it undecided — leaving "force this pairing" enabled on a request the
  // server rejects with `round_has_results`.
  it("is true for a no-show even though its result is null", () => {
    for (const side of ["player1", "player2", "both"] as const) {
      const b = board({ result: null, no_show: side });
      expect(b.result).toBeNull();
      expect(isDecided(b)).toBe(true);
    }
  });

  it("matches the server: a round is re-pairable only if no board is decided", () => {
    // The confirmed repro — two boards, one forfeited, one unplayed.
    const boards = [board({ no_show: "player1" }), board({})];
    expect(boards.some(isDecided)).toBe(true);
  });
});
