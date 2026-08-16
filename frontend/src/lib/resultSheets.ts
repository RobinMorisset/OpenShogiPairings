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

// The printed geometry of a slip, in centimetres. Every part of it is a fixed
// height: three rounds print three rows of exactly the size nine rounds print
// nine of, and the paper left over is left blank at the bottom of the page.
// (Stretching the table to the foot of the slip instead poured the spare
// centimetres into the column-title row, which looked absurd on a short
// tournament.)
//
// `ResultSheets.svelte` applies these numbers as the actual heights, so they are
// the layout rather than a description of it. The one thing it has to keep in
// step is that the header's three lines fit inside `SLIP_HEADER_CM`.

/** The block above the table: tournament name, the ruled number and name line,
 *  and the ELO and grade line. */
export const SLIP_HEADER_CM = 1.5;
/** The table's column-title row. */
export const SLIP_TITLES_CM = 0.4;
/** One round's row — about as short as a hand-written board number gets. */
export const SLIP_ROW_CM = 0.5;
/** The slip's own padding and cut border, plus the gap above the table. A
 *  millimetre more than they measure, so the collapsed cell borders round the
 *  last row's line down into the padding rather than off the bottom of a slip
 *  that clips what does not fit. */
export const SLIP_CHROME_CM = 0.95;

/** How much of an A4's height the slips may use: 25.2cm clears the widest
 *  default print margin. */
export const PAGE_CM = 25.2;

/** Slips print two to a row, and at most this many rows to a page: a one-round
 *  slip would otherwise pack seven rows of stamps onto an A4. Five rows is a
 *  4.9cm slip, still enough of one to write on and hand out. */
export const MAX_SLIP_ROWS_PER_PAGE = 5;

/** How tall a slip carrying `rows` body rows prints. */
export function slipHeightCm(rows: number): number {
  return SLIP_CHROME_CM + SLIP_HEADER_CM + SLIP_TITLES_CM + rows * SLIP_ROW_CM;
}

/** How many slips fit on an A4, for `rows` body rows (rounds, plus the MacMahon
 *  row when there is one). */
export function sheetsPerPage(rows: number): number {
  const height = slipHeightCm(rows);
  const perColumn = Math.floor(PAGE_CM / height);
  // `MAX_ROUNDS` keeps this out of reach — 31 rows is an 18.8cm slip — so it
  // means someone raised that bound past what a page of fixed rows can hold.
  if (perColumn < 1) {
    throw new Error(`a ${rows}-row slip is ${height}cm, taller than the ${PAGE_CM}cm page`);
  }
  return 2 * Math.min(perColumn, MAX_SLIP_ROWS_PER_PAGE);
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
