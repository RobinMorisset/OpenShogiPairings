import { describe, it, expect } from "vitest";
import {
  cleanThresholds,
  criterionEquals,
  criterionSortKey,
  eqThresholds,
  normExempt,
  type ThresholdRow,
} from "./thresholds";
import type { MacMahonThreshold } from "./types";

/** A terse ELO threshold row (no degressive round). */
function eloRow(value: number, dropsAfterRound: number | null = null): ThresholdRow {
  return { kind: "elo", value, gradeKind: "dan", gradeLevel: 1, dropsAfterRound };
}

/** A terse grade threshold row. */
function gradeRow(
  gradeKind: "dan" | "kyu",
  gradeLevel: number,
  dropsAfterRound: number | null = null,
): ThresholdRow {
  return { kind: "grade", value: 0, gradeKind, gradeLevel, dropsAfterRound };
}

const elo = (value: number, drops: number | null = null): MacMahonThreshold => ({
  criterion: { kind: "elo", value },
  drops_after_round: drops,
});

describe("criterionSortKey", () => {
  it("orders ELO before grade, then by value/strength", () => {
    // ELO thresholds sort first (tag 0), grades after (tag 1).
    expect(criterionSortKey({ kind: "elo", value: 1500 })).toEqual([0, 1500]);
    // 1 dan has rank 1, 5 kyu has rank -4 (weaker), so 5 kyu sorts before 1 dan.
    expect(criterionSortKey({ kind: "grade", grade: { kind: "dan", level: 1 } })).toEqual([1, 1]);
    expect(criterionSortKey({ kind: "grade", grade: { kind: "kyu", level: 5 } })).toEqual([1, -4]);
  });
});

describe("criterionEquals", () => {
  it("compares kind and value/grade", () => {
    expect(criterionEquals({ kind: "elo", value: 1500 }, { kind: "elo", value: 1500 })).toBe(true);
    expect(criterionEquals({ kind: "elo", value: 1500 }, { kind: "elo", value: 1600 })).toBe(false);
    expect(
      criterionEquals(
        { kind: "grade", grade: { kind: "dan", level: 3 } },
        { kind: "grade", grade: { kind: "dan", level: 3 } },
      ),
    ).toBe(true);
    // Different kinds are never equal.
    expect(
      criterionEquals({ kind: "elo", value: 1500 }, { kind: "grade", grade: { kind: "dan", level: 1 } }),
    ).toBe(false);
    expect(
      criterionEquals(
        { kind: "grade", grade: { kind: "dan", level: 1 } },
        { kind: "grade", grade: { kind: "kyu", level: 1 } },
      ),
    ).toBe(false);
  });
});

describe("cleanThresholds", () => {
  it("sorts ELO ascending and de-duplicates by value (first kept)", () => {
    const out = cleanThresholds([eloRow(1700), eloRow(1200), eloRow(1200, 3), eloRow(1500)]);
    expect(out).toEqual([elo(1200), elo(1500), elo(1700)]);
  });

  it("orders ELO thresholds before grade ones, grades by strength", () => {
    const out = cleanThresholds([gradeRow("dan", 5), eloRow(1500), gradeRow("dan", 1), gradeRow("kyu", 5)]);
    expect(out).toEqual([
      elo(1500),
      { criterion: { kind: "grade", grade: { kind: "kyu", level: 5 } }, drops_after_round: null },
      { criterion: { kind: "grade", grade: { kind: "dan", level: 1 } }, drops_after_round: null },
      { criterion: { kind: "grade", grade: { kind: "dan", level: 5 } }, drops_after_round: null },
    ]);
  });

  it("drops rows below 1 or non-finite and rounds the values", () => {
    expect(cleanThresholds([eloRow(0), eloRow(NaN), gradeRow("kyu", 0)])).toEqual([]);
    expect(cleanThresholds([eloRow(1499.6)])).toEqual([elo(1500)]);
  });

  it("normalizes a degressive round below 1 to null and rounds it", () => {
    expect(cleanThresholds([eloRow(1500, 0)])).toEqual([elo(1500, null)]);
    expect(cleanThresholds([eloRow(1500, 2.7)])).toEqual([elo(1500, 3)]);
    // A duplicate value keeps the *first* row's drop round.
    expect(cleanThresholds([eloRow(1500, 3), eloRow(1500, null)])).toEqual([elo(1500, 3)]);
  });
});

describe("eqThresholds", () => {
  it("compares criterion and drop round pairwise", () => {
    expect(eqThresholds([elo(1500, 3)], [elo(1500, 3)])).toBe(true);
    expect(eqThresholds([elo(1500)], [elo(1600)])).toBe(false);
    expect(eqThresholds([elo(1500, 3)], [elo(1500, 2)])).toBe(false);
    expect(eqThresholds([elo(1500)], [])).toBe(false);
  });
});

describe("normExempt", () => {
  it("trims, drops blanks, and de-duplicates case-insensitively (first spelling kept)", () => {
    expect(normExempt(["  Paris  ", "paris", "   ", "Lyon"])).toEqual(["Paris", "Lyon"]);
  });
});
