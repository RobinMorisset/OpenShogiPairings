import { describe, expect, it } from "vitest";

import { registeredNationalities, withoutNationality } from "./nationalities";
import type { Player } from "./types";

/** A registered player carrying only the fields these helpers read. */
function player(id: string, nationality?: string): Player {
  return { id, last_name: `P${id}`, first_name: "", nationality, categories: [], adjustments: [] };
}

describe("registeredNationalities", () => {
  it("counts each nationality and sorts them alphabetically", () => {
    const players = [player("1", "JP"), player("2", "FR"), player("3", "JP")];
    expect(registeredNationalities(players)).toEqual([
      ["FR", 1],
      ["JP", 2],
    ]);
  });

  it("ignores a missing or blank nationality", () => {
    // Neither belongs to any country's list, so neither may be picked — the
    // licence check would report every one of them as unlicensed.
    const players = [player("1"), player("2", "   "), player("3", "FR")];
    expect(registeredNationalities(players)).toEqual([["FR", 1]]);
  });

  it("is empty for a roster with no nationalities at all", () => {
    expect(registeredNationalities([player("1"), player("2")])).toEqual([]);
  });

  it("trims the stored value, so a stray space is not a second nationality", () => {
    expect(registeredNationalities([player("1", "FR"), player("2", " FR ")])).toEqual([["FR", 2]]);
  });
});

describe("withoutNationality", () => {
  it("counts the players no nationality picker can reach", () => {
    const players = [player("1"), player("2", "  "), player("3", "FR")];
    expect(withoutNationality(players)).toBe(2);
  });
});
