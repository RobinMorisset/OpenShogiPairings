import { describe, it, expect } from "vitest";
import { formatScore } from "./score";

describe("formatScore", () => {
  it("renders whole points from even half-unit totals", () => {
    expect(formatScore(0)).toBe("0");
    expect(formatScore(2)).toBe("1");
    expect(formatScore(6)).toBe("3");
  });

  it("renders halves with a ½ glyph", () => {
    expect(formatScore(1)).toBe("½");
    expect(formatScore(3)).toBe("1½");
    expect(formatScore(7)).toBe("3½");
  });
});
