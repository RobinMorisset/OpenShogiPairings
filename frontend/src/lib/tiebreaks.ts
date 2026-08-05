// Translation helpers for TIEBREAKS (types.ts) — shared by the Settings picker
// and the Results headers.

import { TIEBREAKS, type Tiebreak } from "./types";

type Translate = (id: string, opts?: Record<string, unknown>) => string;

// Most codes (SOSM, SOSW, ...) are standardized abbreviations kept as-is in
// every language; only the two prose labels are actually translated.
// Exported so `i18n/keyUsage.test.ts` can tell which `tiebreaks.*.label` keys
// are actually requested — the rest would be dead weight in every catalogue.
export const TRANSLATED_LABELS = new Set<Tiebreak>(["points", "est_elo"]);

export function tiebreakLabel(code: Tiebreak, t: Translate): string {
  if (TRANSLATED_LABELS.has(code)) return t(`tiebreaks.${code}.label`);
  return TIEBREAKS.find((c) => c.code === code)?.label ?? code;
}

export function tiebreakTitle(code: Tiebreak, t: Translate): string {
  return t(`tiebreaks.${code}.title`);
}
