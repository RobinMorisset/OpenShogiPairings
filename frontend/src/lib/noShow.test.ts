import { describe, expect, it } from "vitest";

import { winnerOf } from "./boardOutcome";
import { absent, absenceKind, combineForfeit, cycledForfeit, isDecided } from "./noShow";
import type { Board, Forfeit } from "./types";
import { withRecord, type BoardFields } from "./boardFixture";

describe("absent", () => {
  it("is true only for the named side", () => {
    expect(absent({ player1: "no_show" }, "player1")).toBe(true);
    expect(absent({ player1: "no_show" }, "player2")).toBe(false);
    expect(absent({ player2: "no_show" }, "player2")).toBe(true);
    expect(absent({ player2: "no_show" }, "player1")).toBe(false);
  });

  it("is true for both sides under `both`", () => {
    const both: Forfeit = { both: ["no_show", "justified"] };
    expect(absent(both, "player1")).toBe(true);
    expect(absent(both, "player2")).toBe(true);
    // ...and each side keeps its own reason.
    expect(absenceKind(both, "player1")).toBe("no_show");
    expect(absenceKind(both, "player2")).toBe("justified");
  });

  it("is false when there is no forfeit", () => {
    expect(absent(null, "player1")).toBe(false);
    expect(absent(undefined, "player2")).toBe(false);
  });
});

describe("combineForfeit", () => {
  it("maps the per-side reasons, and no forfeit at all when both turned up", () => {
    expect(combineForfeit(null, null)).toBe(null);
    expect(combineForfeit("no_show", null)).toEqual({ player1: "no_show" });
    expect(combineForfeit(null, "justified")).toEqual({ player2: "justified" });
    expect(combineForfeit("no_show", "justified")).toEqual({
      both: ["no_show", "justified"],
    });
  });
});

describe("cycledForfeit", () => {
  // One control per side. In an individual tournament there are two states to
  // cycle through, because a justified absence never reaches a board there.
  it("cycles a side present → no-show → present outside team mode", () => {
    expect(cycledForfeit(null, "player1", false)).toEqual({ player1: "no_show" });
    expect(cycledForfeit({ player1: "no_show" }, "player1", false)).toBe(null);
  });

  it("adds the justified step in team mode", () => {
    expect(cycledForfeit(null, "player1", true)).toEqual({ player1: "no_show" });
    expect(cycledForfeit({ player1: "no_show" }, "player1", true)).toEqual({
      player1: "justified",
    });
    expect(cycledForfeit({ player1: "justified" }, "player1", true)).toBe(null);
  });

  it("leaves the other side untouched, reaching `both`", () => {
    expect(cycledForfeit({ player1: "no_show" }, "player2", false)).toEqual({
      both: ["no_show", "no_show"],
    });
    // From both, clearing one side leaves the other with its own reason.
    expect(cycledForfeit({ both: ["justified", "no_show"] }, "player2", false)).toEqual({
      player1: "justified",
    });
  });
});

describe("isDecided", () => {
  // Only the fields the predicate reads; the rest of `Board` is irrelevant here.
  const board = (fields: BoardFields): Board =>
    ({ player1: 1, player2: 2, ...withRecord(fields) }) as Board;

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
  it("is true for a forfeit even though no winner is recorded", () => {
    for (const side of ["no_show", "justified"] as const) {
      const b = board({ outcome: { kind: "forfeit", absent: { player1: side } } });
      expect(winnerOf(b)).toBeNull();
      expect(isDecided(b)).toBe(true);
    }
  });

  it("matches the server: a round is re-pairable only if no board is decided", () => {
    // The confirmed repro — two boards, one forfeited, one unplayed.
    const boards = [
      board({ outcome: { kind: "forfeit", absent: { player1: "no_show" } } }),
      board({}),
    ];
    expect(boards.some(isDecided)).toBe(true);
  });
});
