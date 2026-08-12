import { describe, expect, it } from "vitest";

import { pairingRating, sortedByRating, teamAverageRating } from "./teams";
import type { Player } from "./types";

/** Only the fields these helpers read; the rest of `Player` is irrelevant. */
const player = (id: string, tid: number, rating: number | null, pairing?: number): Player =>
  ({ id, tournament_id: tid, rating, pairing_rating: pairing }) as Player;

describe("pairingRating", () => {
  it("prefers the real rating, and falls back to the referee's", () => {
    expect(pairingRating(player("a", 1, 1800, 1200))).toBe(1800);
    expect(pairingRating(player("a", 1, null, 1200))).toBe(1200);
    expect(pairingRating(player("a", 1, null))).toBe(null);
  });
});

describe("teamAverageRating", () => {
  it("rounds to nearest, counting a referee's pairing ELO", () => {
    expect(teamAverageRating([player("a", 1, 2000), player("b", 2, 1801)])).toBe(1901);
    expect(teamAverageRating([player("a", 1, 2000), player("b", 2, null, 1600)])).toBe(1800);
  });

  it("has no average unless every member is rated", () => {
    const members = [player("a", 1, 2000), player("b", 2, 1801), player("c", 3, null)];
    expect(teamAverageRating(members)).toBe(null);
    expect(teamAverageRating([player("a", 1, null)])).toBe(null);
    expect(teamAverageRating([])).toBe(null);
  });
});

describe("sortedByRating", () => {
  it("is descending pairing rating, unrated last", () => {
    const a = player("a", 1, 2000);
    const b = player("b", 2, null, 1900);
    const c = player("c", 3, null);
    expect(sortedByRating([a, b, c])).toBe(true);
    expect(sortedByRating([b, a, c])).toBe(false);
    expect(sortedByRating([a, c, b])).toBe(false);
  });

  it("holds for rosters the sort would not move", () => {
    // Nothing to sort, and ties are in order either way round (stable sort).
    expect(sortedByRating([])).toBe(true);
    expect(sortedByRating([player("a", 1, 1500)])).toBe(true);
    expect(sortedByRating([player("a", 1, 1500), player("b", 2, 1500)])).toBe(true);
    expect(sortedByRating([player("a", 1, null), player("b", 2, null)])).toBe(true);
  });
});
