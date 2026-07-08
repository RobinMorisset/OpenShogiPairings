// Helpers for the compact free-text grade entry used in registration/editing
// ("3d" / "3 dan" / "5k" / "5 kyu"), shared between PlayerRegistration and
// PlayerList so the parse/format/sort rules stay in one place.

import type { Grade } from "./types";

/** Parse free-text grade entry ("3d", "3 dan", "5k", "5 kyu"), or null. */
export function parseGrade(raw: string): Grade | null {
  const m = raw.trim().match(/^(\d+)\s*(d(?:an)?|k(?:yu)?)$/i);
  if (!m) return null;
  const level = parseInt(m[1], 10);
  if (!Number.isInteger(level) || level < 1) return null;
  return { kind: m[2].toLowerCase().startsWith("d") ? "dan" : "kyu", level };
}

/** Render a Grade back to the compact text form the input accepts. */
export function formatGrade(g: Grade): string {
  return `${g.level}${g.kind === "dan" ? "d" : "k"}`;
}

/**
 * A contiguous strength scale for sorting: `n` dan is `n`; `n` kyu is `1 - n`,
 * so `1 kyu` sits directly below `1 dan` with no gap. Mirrors the server's
 * `Grade::rank`.
 */
export function gradeRank(g: Grade): number {
  return g.kind === "dan" ? g.level : 1 - g.level;
}
