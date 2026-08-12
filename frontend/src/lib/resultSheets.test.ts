import { describe, expect, it } from "vitest";

import {
  buildSheetPlayers,
  macMahonRowShown,
  paginate,
  sheetsPerPage,
  SIX_UP_MAX_ROWS,
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

describe("sheetsPerPage", () => {
  it("fits six to a page while the rows stay legible", () => {
    expect(sheetsPerPage(1)).toBe(6);
    expect(sheetsPerPage(SIX_UP_MAX_ROWS)).toBe(6);
  });

  it("drops to four rather than shrink the rows further", () => {
    expect(sheetsPerPage(SIX_UP_MAX_ROWS + 1)).toBe(4);
    expect(sheetsPerPage(30)).toBe(4);
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
