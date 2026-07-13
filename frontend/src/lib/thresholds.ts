// Client-side mirror of the server's MacMahon-threshold and exempt-club
// normalization (osp-core's `TournamentSettings::normalize_thresholds` /
// `normalized`). The server stays authoritative — it re-normalizes on save — but
// the settings UI needs the same rules synchronously to preview the canonical
// form and to detect whether the local edits still match what's stored. Kept out
// of the component so this parity logic is unit-tested (see thresholds.test.ts).

import type { GradeKind, MacMahonThreshold, ThresholdCriterion } from "./types";
import { gradeRank } from "./grade";

/**
 * A threshold row being edited: either an ELO value or a dan/kyu grade (only the
 * fields for the active `kind` are meaningful), plus its optional degressive
 * stopping round (`null` = never drops).
 */
export type ThresholdRow = {
  kind: "elo" | "grade";
  value: number;
  gradeKind: GradeKind;
  gradeLevel: number;
  dropsAfterRound: number | null;
};

/**
 * A key that sorts ELO thresholds by value, then grade thresholds by strength —
 * mirrors the server's `ThresholdCriterion::sort_key`.
 */
export function criterionSortKey(c: ThresholdCriterion): [number, number] {
  return c.kind === "elo" ? [0, c.value] : [1, gradeRank(c.grade)];
}

/** Whether two criteria denote the same threshold (same kind and value/grade). */
export function criterionEquals(a: ThresholdCriterion, b: ThresholdCriterion): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === "elo") return a.value === (b as { kind: "elo"; value: number }).value;
  const bg = (b as { kind: "grade"; grade: { kind: GradeKind; level: number } }).grade;
  return a.grade.kind === bg.kind && a.grade.level === bg.level;
}

/**
 * Clean, sort and de-duplicate the editable rows into the server's canonical
 * form: drop rows whose value/level is below 1 or non-finite, round the numbers,
 * normalize a `dropsAfterRound` below 1 to `null`, sort by criterion, and keep
 * the first of each duplicate criterion. Used both to persist and to compare the
 * local rows against what's stored.
 */
export function cleanThresholds(rows: ThresholdRow[]): MacMahonThreshold[] {
  return rows
    .filter((r) =>
      r.kind === "elo"
        ? Number.isFinite(r.value) && r.value >= 1
        : Number.isFinite(r.gradeLevel) && r.gradeLevel >= 1,
    )
    .map((r) => ({
      criterion:
        r.kind === "elo"
          ? ({ kind: "elo", value: Math.round(r.value) } as const)
          : ({
              kind: "grade",
              grade: { kind: r.gradeKind, level: Math.round(r.gradeLevel) },
            } as const),
      drops_after_round:
        r.dropsAfterRound != null && Number.isFinite(r.dropsAfterRound) && r.dropsAfterRound >= 1
          ? Math.round(r.dropsAfterRound)
          : null,
    }))
    .sort((a, b) => {
      const [at, av] = criterionSortKey(a.criterion);
      const [bt, bv] = criterionSortKey(b.criterion);
      return at - bt || av - bv;
    })
    .filter((v, i, arr) => i === 0 || !criterionEquals(v.criterion, arr[i - 1].criterion));
}

/** Whether two normalized threshold lists are equal (criterion + drop round). */
export function eqThresholds(a: MacMahonThreshold[], b: MacMahonThreshold[]): boolean {
  return (
    a.length === b.length &&
    a.every(
      (v, i) =>
        criterionEquals(v.criterion, b[i].criterion) &&
        (v.drops_after_round ?? null) === (b[i].drops_after_round ?? null),
    )
  );
}

/**
 * Mirror the server's exempt-club normalization: trim, drop empties, and
 * de-duplicate case-insensitively keeping the first spelling.
 */
export function normExempt(list: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of list) {
    const c = raw.trim();
    if (c && !seen.has(c.toLowerCase())) {
      seen.add(c.toLowerCase());
      out.push(c);
    }
  }
  return out;
}
