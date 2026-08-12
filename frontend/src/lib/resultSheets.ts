// The per-player result-reporting sheets a referee cuts out of an A4 and hands
// round at the start of a tournament: one small slip per player, carrying their
// tournament number and one row per planned round for the board, the opponent
// and the running score. The referee fills the board and opponent numbers in at
// the start of each round, the players write the handicap and the result after
// their opponent's number and drop the slips back on the desk, and the referee
// writes the new point total. That makes the tournament reportable while the
// referee is busy, recoverable if the computer dies, and — since the slips can
// be sorted by score on a table — pairable by hand in the meantime.
//
// This module holds the parts worth testing away from the DOM: who gets a sheet
// and what its header carries, whether a MacMahon row is printed, and how many
// slips fit on a page.

import type { Grade, Player, Standing, TournamentSettings } from "./types";

/** The header data of one printed slip. */
export interface SheetPlayer {
  tournamentId: number;
  /** Last name then first name, the order a player looks themselves up in. */
  name: string;
  rating: number | null;
  grade: Grade | null;
  /**
   * MacMahon starting points, in half units (see `score.ts`), or `null` when
   * this tournament prints no MacMahon row — see {@link macMahonRowShown}.
   */
  macmahon: number | null;
}

/** The default offered for "how many rounds is this tournament?", since nothing
 *  in the tournament data records a planned round count. */
export const DEFAULT_ROUNDS = 9;

/** Bounds on what the referee may ask for, so a typo can't ask for a thousand
 *  pages of slips. */
export const MAX_ROUNDS = 30;
export const MAX_BLANK_SHEETS = 100;

/**
 * Whether the slips carry a MacMahon starting-points row.
 *
 * Only when there are starting groups at all *and* the starting points are
 * fixed for the tournament. Drawn from a live ELO estimate they are not: the
 * estimate moves every round, so a number printed at round 1 would be a lie by
 * round 2 — and there is nothing stable to hand the player instead.
 */
export function macMahonRowShown(settings: TournamentSettings): boolean {
  const p = settings.pairing;
  return (
    p.kind === "swiss" && p.macmahon.thresholds.length > 0 && p.macmahon.source.kind === "static"
  );
}

/**
 * One sheet per registered player, ordered by tournament number — the order the
 * referee hands them out and files them back in.
 *
 * Only callable once registration is finalized: before that nobody has a
 * tournament number and the server computes no standings, so both lookups below
 * throw rather than print a slip with a blank identity on it.
 */
export function buildSheetPlayers(
  players: Player[],
  settings: TournamentSettings,
  standings: Standing[],
): SheetPlayer[] {
  const withMacMahon = macMahonRowShown(settings);
  const byPlayer = new Map(standings.map((s) => [s.player_id, s]));
  return players
    .map((p) => {
      if (p.tournament_id == null) {
        throw new Error(`no tournament number for ${p.last_name}: registration is not finalized`);
      }
      const standing = byPlayer.get(p.id);
      if (withMacMahon && !standing) {
        throw new Error(`no standing for ${p.last_name}: cannot print their MacMahon points`);
      }
      return {
        tournamentId: p.tournament_id,
        name: `${p.last_name} ${p.first_name}`.trim(),
        rating: p.rating ?? null,
        grade: p.grade ?? null,
        macmahon: withMacMahon ? standing!.macmahon : null,
      };
    })
    .sort((a, b) => a.tournamentId - b.tournamentId);
}

/**
 * The most rows a slip can carry and still be legible at six to an A4 page.
 *
 * A 6-up slip is 8.4cm tall, of which the header and the column titles take
 * about 2.4cm; eleven rows then get ~5.5mm each, which is about as small as a
 * hand-written board number gets. Past that the sheets go 4-up (12.6cm) rather
 * than shrink further.
 */
export const SIX_UP_MAX_ROWS = 11;

/** How many slips fit on an A4, for `rows` body rows (rounds, plus the MacMahon
 *  row when there is one). */
export function sheetsPerPage(rows: number): 6 | 4 {
  return rows <= SIX_UP_MAX_ROWS ? 6 : 4;
}

/** Split the slips into pages of `perPage`, so each page is its own printed
 *  block and the page breaks fall where the grid says rather than wherever the
 *  browser decides to cut a tall flow. */
export function paginate<T>(sheets: T[], perPage: number): T[][] {
  const pages: T[][] = [];
  for (let i = 0; i < sheets.length; i += perPage) {
    pages.push(sheets.slice(i, i + perPage));
  }
  return pages;
}
