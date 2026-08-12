// What the public page shows, derived from the projection the server sends.
//
// Two renderers consume this: the live reader page (`PublicView`, phase 1) and
// the static export (`PublicSnapshot`, phase 2). They differ only in how the
// sections are reached — tabs in one, one HTML file per section in the other —
// so the *wiring* between the projection and the leaf components lives here
// rather than twice over. See docs/public-access.md §5.

import type { Handicap, PublicTournamentResponse, Round, Winner } from "./types";

/** One round, with the two per-board arrays that belong to it. */
export interface PublicRound {
  round: Round;
  /** Suggested handicaps for this round's boards — empty unless the referee
   *  chose to publish the suggestion at all (the server decides; see
   *  `PublicTournamentView`). */
  suggestedHandicaps: (Handicap | null)[];
  /** Server-computed effective winners for this round's boards. */
  effectiveWinners: (Winner | null)[];
}

/**
 * Pair every published round with its slice of `suggested_handicaps` and
 * `effective_winners`.
 *
 * Both arrays are indexed like `tournament.rounds`, so this is a positional
 * join — and a missing or short row yields an empty slice rather than throwing:
 * the projection is allowed to omit the suggestions entirely.
 */
export function publicRounds(view: PublicTournamentResponse): PublicRound[] {
  const suggested = view.suggested_handicaps ?? [];
  const winners = view.effective_winners ?? [];
  return view.tournament.rounds.map((round, index) => ({
    round,
    suggestedHandicaps: suggested[index] ?? [],
    effectiveWinners: winners[index] ?? [],
  }));
}

/**
 * One page of the static export: what the live page shows in one tab.
 *
 * `file` is both the file name written to disk and the `href` the other pages
 * link to it by, so every page in an export must be saved side by side in one
 * directory for the navigation between them to work.
 */
export type PublicSection =
  | { kind: "standings"; file: string }
  | { kind: "cup"; file: string }
  | { kind: "round"; file: string; round: PublicRound };

/**
 * The pages one export writes, in the order the live page's tabs appear.
 *
 * Standings first — it is the entry point, so it gets the plain `<slug>` name
 * a referee would link to, and the others hang off it. Rounds ascend, as they
 * do in the app; nothing here reorders what the referee is used to.
 *
 * Before round 1 the standings page is the *only* page: it shows the entrant
 * list, and there is nothing else to look at yet.
 */
export function publicSections(
  view: PublicTournamentResponse,
  slug: string,
): PublicSection[] {
  const rounds = publicRounds(view);
  if (rounds.length === 0) return [{ kind: "standings", file: `${slug}.html` }];
  const cup: PublicSection[] =
    view.tournament.cup && view.cup_bracket
      ? [{ kind: "cup", file: `${slug}-cup.html` }]
      : [];
  return [
    { kind: "standings", file: `${slug}.html` },
    ...cup,
    ...rounds.map(
      (round): PublicSection => ({
        kind: "round",
        file: `${slug}-round-${round.round.number}.html`,
        round,
      }),
    ),
  ];
}

/**
 * svelte-i18n's translator — the value of its `$_` store, which the package
 * types internally but does not export, so it is spelled out here.
 */
export type Translate = (
  key: string,
  options?: { values?: Record<string, string | number | boolean | Date | null | undefined> },
) => string;

/**
 * A section's name, for its navigation link and its `<title>`.
 *
 * Takes the translator rather than importing it, so the exporter (plain
 * TypeScript) and the page component (which has `$_`) name a section
 * identically instead of each phrasing it its own way.
 */
export function sectionLabel(section: PublicSection, t: Translate): string {
  switch (section.kind) {
    case "standings":
      return t("app.tabResults");
    case "cup":
      return t("app.tabCup");
    case "round":
      return section.round.round.completed
        ? t("app.tabRoundCompleted", { values: { number: section.round.round.number } })
        : t("app.tabRound", { values: { number: section.round.round.number } });
  }
}
