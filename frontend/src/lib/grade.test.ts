import { describe, it, expect } from "vitest";
import { parseGrade, formatGrade, gradeRank } from "./grade";

describe("parseGrade", () => {
  it("accepts the compact and spelled-out forms", () => {
    expect(parseGrade("3d")).toEqual({ kind: "dan", level: 3 });
    expect(parseGrade("3 dan")).toEqual({ kind: "dan", level: 3 });
    expect(parseGrade("5k")).toEqual({ kind: "kyu", level: 5 });
    expect(parseGrade("5 kyu")).toEqual({ kind: "kyu", level: 5 });
    expect(parseGrade("  12DAN ")).toEqual({ kind: "dan", level: 12 });
  });

  it("rejects junk", () => {
    expect(parseGrade("")).toBe(null);
    expect(parseGrade("dan")).toBe(null); // no level
    expect(parseGrade("5")).toBe(null); // no kind
    expect(parseGrade("0d")).toBe(null); // level must be ≥ 1
    expect(parseGrade("3 danger")).toBe(null); // suffix must be exact
  });
});

describe("formatGrade", () => {
  it("renders the compact form the input accepts", () => {
    expect(formatGrade({ kind: "dan", level: 3 })).toBe("3d");
    expect(formatGrade({ kind: "kyu", level: 5 })).toBe("5k");
  });

  it("round-trips through parseGrade", () => {
    for (const g of [
      { kind: "dan", level: 1 },
      { kind: "dan", level: 7 },
      { kind: "kyu", level: 1 },
      { kind: "kyu", level: 20 },
    ] as const) {
      expect(parseGrade(formatGrade(g))).toEqual(g);
    }
  });
});

describe("gradeRank", () => {
  it("puts a bigger value on stronger grades, contiguous across the boundary", () => {
    // Ascending strength: 20 kyu < 1 kyu < 1 dan < 5 dan.
    expect(gradeRank({ kind: "kyu", level: 20 })).toBeLessThan(gradeRank({ kind: "kyu", level: 1 }));
    expect(gradeRank({ kind: "kyu", level: 1 })).toBeLessThan(gradeRank({ kind: "dan", level: 1 }));
    expect(gradeRank({ kind: "dan", level: 1 })).toBeLessThan(gradeRank({ kind: "dan", level: 5 }));
    // 1 kyu sits directly below 1 dan with no gap (mirrors Grade::rank).
    expect(gradeRank({ kind: "kyu", level: 1 }) + 1).toBe(gradeRank({ kind: "dan", level: 1 }));
  });
});
