import { describe, it, expect } from "vitest";
import { absent, combineNoShow, toggledNoShow } from "./noShow";

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
