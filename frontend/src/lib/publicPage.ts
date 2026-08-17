// What the public page shows, derived from the projection the server sends.
//
// Two renderers consume this: the live reader page (`PublicView`, phase 1) and
// the static export (`PublicSnapshot`, phase 2). They differ only in how the
// sections are reached — tabs in one, one HTML file per section in the other —
// so the *wiring* between the projection and the leaf components lives here
// rather than twice over. See docs/archive/public-access.md §5.

import type { Handicap, PublicTournamentResponse, Round, TeamMatchView } from "./types";

/** One round, with the per-round slices of the projection that belong to it. */
interface PublicRound {
  round: Round;
  /** Suggested handicaps for this round's boards — empty unless the referee
   *  chose to publish the suggestion at all (the server decides; see
   *  `PublicTournamentView`). */
  suggestedHandicaps: (Handicap | null)[];
  /** This round's team matches, scored, as the server derived them — empty
   *  outside team mode. */
  teamMatches: TeamMatchView[];
}

/**
 * Pair every published round with its slice of `suggested_handicaps` and
 * `team_matches`.
 *
 * Both are indexed like `tournament.rounds`, so this is a positional join — and
 * a missing or short row yields an empty slice rather than throwing: the
 * projection is allowed to omit the suggestions entirely, and carries no team
 * matches outside team mode.
 *
 * (`effective_winners` needs no slice: the round page shows each board's *own*
 * winner, and the one view that wants the Wiel-rule winner — the standings
 * cross-table — takes the whole array.)
 */
export function publicRounds(view: PublicTournamentResponse): PublicRound[] {
  const suggested = view.suggested_handicaps ?? [];
  const matches = view.team_matches ?? [];
  return view.tournament.rounds.map((round, index) => ({
    round,
    suggestedHandicaps: suggested[index] ?? [],
    teamMatches: matches[index] ?? [],
  }));
}

/**
 * One section of the public page: a tab on the live page, a file in the export.
 *
 * The two readers build their section list from the same {@link publicSections},
 * and render each one through the same `PublicSectionBody` — so a section cannot
 * exist on one and be missing from the other, and cannot show two different
 * things. They used to decide separately, and disagreed: a cup seeded before
 * round 1 gave the live page a bracket and no entrant list, and the export an
 * entrant list and no bracket.
 */
export type PublicSection =
  | { kind: "standings" }
  | { kind: "cup" }
  | { kind: "round"; round: PublicRound };

/**
 * The sections a public page has, in the order they are shown — the live page's
 * tabs, and the pages one export writes.
 *
 * Standings first: it is the live page's first tab and the export's entry point,
 * the file a referee links to. It is always there, because it is also the page
 * that answers "am I registered?" — before round 1 there is nothing to rank and
 * it shows the entrant list instead (docs/archive/public-access.md §2). Then the cup, if
 * there is one; then the rounds, ascending, as in the app.
 *
 * A cup with no round behind it is not an odd state to guard against: the
 * bracket is frozen at finalization, *before* round 1 is confirmed, so it is
 * exactly what the room has to look at while the first round is being paired.
 */
export function publicSections(view: PublicTournamentResponse): PublicSection[] {
  const cup: PublicSection[] =
    view.tournament.cup && view.cup_bracket ? [{ kind: "cup" }] : [];
  return [
    { kind: "standings" },
    ...cup,
    ...publicRounds(view).map((round): PublicSection => ({ kind: "round", round })),
  ];
}

/**
 * Whether a section's content is a table that will not wrap, so its card
 * should grow to stay behind it (`ContentCard`'s `wide`).
 *
 * Here rather than in either renderer because the two must agree: the live
 * page and the export show the same tables to the same room, and the reason a
 * standings table overflows a narrow window is not one of them. They did not
 * agree before — the export widened its standings page and the live page had
 * no wide card at all, so the reader's standings painted its zebra rows past
 * the rounded border onto the page background.
 *
 * The same list as the referee app's `tabHasWideTable`, minus the tabs a
 * reader has no equivalent of: standings (a column per round plus the
 * tie-breaks) and the rounds (two named players and their ratings per board).
 * The cup bracket is not a table and sizes itself.
 */
export function sectionNeedsWideCard(section: PublicSection): boolean {
  return section.kind === "standings" || section.kind === "round";
}

/**
 * A section's identity: the live page's tab id, and what tells the export's
 * "you are here" tab from the links around it.
 */
export function sectionId(section: PublicSection): string {
  switch (section.kind) {
    case "standings":
      return "standings";
    case "cup":
      return "cup";
    case "round":
      return `round-${section.round.round.number}`;
  }
}

/**
 * The file a section is written to in a static export — both the name on disk
 * and the `href` the other pages link to it by, so every page of one export must
 * be saved side by side in a single directory.
 *
 * The standings page gets the bare `<slug>` name: it is the entry point, so it
 * is the one a referee pastes into their club's website, and the others hang
 * off it.
 */
export function sectionFile(section: PublicSection, slug: string): string {
  return section.kind === "standings"
    ? `${slug}.html`
    : `${slug}-${sectionId(section)}.html`;
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
