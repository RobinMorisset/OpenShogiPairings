import { describe, expect, it } from "vitest";

import {
  buildSheetPlayers,
  macMahonRowShown,
  MAX_ROUNDS,
  MAX_SLIP_ROWS_PER_PAGE,
  PAGE_CM,
  paginate,
  sheetsPerPage,
  slipHeightCm,
  SLIP_ROW_CM,
} from "./resultSheets";
import type { Player, Standing, TournamentSettings } from "./types";

/** Only the fields the sheets read; the rest of `Player` is irrelevant. */
const player = (tournament_id: number | null, extra: Partial<Player> = {}): Player =>
  ({ id: `p${tournament_id}`, last_name: "X", first_name: "", tournament_id, ...extra }) as Player;

const standing = (id: string, macmahon: number): Standing => ({ player_id: id, macmahon }) as Standing;

/** Swiss settings with `n` MacMahon thresholds, from the registration ratings. */
const swiss = (thresholds: number, fromEstimate = false): TournamentSettings =>
  ({
    pairing: {
      kind: "swiss",
      macmahon: {
        thresholds: Array.from({ length: thresholds }, (_v, i) => ({
          criterion: { kind: "elo", value: 1000 + i },
          drops_after_round: null,
        })),
        source: fromEstimate
          ? { kind: "from_estimate", estimator: {} }
          : { kind: "static" },
      },
    },
  }) as TournamentSettings;

const elo = (): TournamentSettings =>
  ({ pairing: { kind: "elo", estimator: {} } }) as TournamentSettings;

describe("macMahonRowShown", () => {
  it("prints the row when there are thresholds and the points are fixed", () => {
    expect(macMahonRowShown(swiss(2))).toBe(true);
  });

  it("prints no row when everyone starts at zero", () => {
    expect(macMahonRowShown(swiss(0))).toBe(false);
  });

  // The number would be stale by round 2, and there is nothing stable to print.
  it("prints no row when the start comes from the live ELO estimate", () => {
    expect(macMahonRowShown(swiss(2, true))).toBe(false);
  });

  it("prints no row in ELO pairing mode, which has no MacMahon at all", () => {
    expect(macMahonRowShown(elo())).toBe(false);
  });
});

describe("buildSheetPlayers", () => {
  it("is one slip per player, in tournament-number order", () => {
    const players = [player(3), player(1), player(2)];
    const sheets = buildSheetPlayers(players, swiss(0), []);
    expect(sheets.map((s) => s.tournamentId)).toEqual([1, 2, 3]);
  });

  it("carries the header data, with nulls where the referee must write it in", () => {
    const players = [
      player(1, { last_name: "Habu", first_name: "Yoshiharu", rating: 2100 }),
      player(2, { last_name: "Newcomer" }),
    ];
    const sheets = buildSheetPlayers(players, swiss(0), []);
    expect(sheets[0]).toEqual({
      tournamentId: 1,
      name: "Habu Yoshiharu",
      rating: 2100,
      grade: null,
      macmahon: null,
    });
    expect(sheets[1].name).toBe("Newcomer");
    expect(sheets[1].rating).toBeNull();
  });

  it("takes the MacMahon start from the standings when the row is printed", () => {
    const players = [player(1), player(2)];
    // Half units: 4 is two points, 5 is two and a half.
    const standings = [standing("p1", 4), standing("p2", 5)];
    const sheets = buildSheetPlayers(players, swiss(2), standings);
    expect(sheets.map((s) => s.macmahon)).toEqual([4, 5]);
  });

  it("leaves the MacMahon start out — and does not need the standings — otherwise", () => {
    const sheets = buildSheetPlayers([player(1)], swiss(0), []);
    expect(sheets[0].macmahon).toBeNull();
  });

  // Both of these mean registration was not finalized, which the round tabs
  // guarantee it was. Loudly, so it can't print a slip nobody can be handed.
  it("throws on a player with no tournament number", () => {
    expect(() => buildSheetPlayers([player(null)], swiss(0), [])).toThrow(/not finalized/);
  });

  it("throws when a MacMahon row is wanted and a player has no standing", () => {
    expect(() => buildSheetPlayers([player(1)], swiss(2), [])).toThrow(/MacMahon/);
  });
});

describe("slipHeightCm", () => {
  // The point of the fixed geometry: a round costs one row, always the same
  // one, instead of the slip's contents being blown up to fill the paper.
  it("grows by exactly one row per row", () => {
    expect(slipHeightCm(4) - slipHeightCm(3)).toBeCloseTo(SLIP_ROW_CM);
    expect(slipHeightCm(31) - slipHeightCm(30)).toBeCloseTo(SLIP_ROW_CM);
  });

  it("keeps even the longest tournament's slip on the page", () => {
    expect(slipHeightCm(MAX_ROUNDS + 1)).toBeLessThan(PAGE_CM);
  });
});

describe("sheetsPerPage", () => {
  it("fits ten to a page at most, however short the slips get", () => {
    // A one-row slip is 3.35cm: seven rows of them would fit, and be stamps.
    expect(sheetsPerPage(1)).toBe(2 * MAX_SLIP_ROWS_PER_PAGE);
    expect(sheetsPerPage(4)).toBe(2 * MAX_SLIP_ROWS_PER_PAGE);
  });

  it("drops a row of slips at a time as the slips get taller", () => {
    expect(sheetsPerPage(5)).toBe(8);
    expect(sheetsPerPage(6)).toBe(8);
    expect(sheetsPerPage(7)).toBe(6);
    expect(sheetsPerPage(11)).toBe(6);
    expect(sheetsPerPage(12)).toBe(4);
    expect(sheetsPerPage(19)).toBe(4);
    expect(sheetsPerPage(20)).toBe(2);
    expect(sheetsPerPage(MAX_ROUNDS + 1)).toBe(2);
  });

  // Only reachable by raising `MAX_ROUNDS` past what a page can hold, which
  // should say so rather than print slips with their last rounds cut off.
  it("throws rather than page a slip that cannot fit", () => {
    expect(() => sheetsPerPage(100)).toThrow(/taller than/);
  });
});

describe("paginate", () => {
  it("fills pages in order and leaves the last one part-full", () => {
    expect(paginate([1, 2, 3, 4, 5, 6, 7], 6)).toEqual([[1, 2, 3, 4, 5, 6], [7]]);
  });

  it("is no pages at all for no slips", () => {
    expect(paginate([], 6)).toEqual([]);
  });
});
