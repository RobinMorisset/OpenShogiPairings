import { describe, expect, it } from "vitest";

import { attendingPlayers, MIN_PRESENT_PLAYERS } from "./roundDraft";
import type { Player } from "./types";

/** Only the fields the rule reads; the rest of `Player` is irrelevant. */
const player = (tournament_id: number | null): Player =>
  ({ id: `p${tournament_id}`, last_name: "X", tournament_id }) as Player;

const field = (...ids: number[]) => ids.map(player);

describe("attendingPlayers", () => {
  it("is everyone when nobody is absent", () => {
    expect(attendingPlayers(field(1, 2, 3), [])).toHaveLength(3);
  });

  it("drops the players marked absent", () => {
    const attending = attendingPlayers(field(1, 2, 3), [2]);
    expect(attending.map((p) => p.tournament_id)).toEqual([1, 3]);
  });

  // The bug this rule was getting wrong: cup and long-game players are not in
  // the Swiss pool, but they are very much in the round, so they count toward
  // the minimum. The old check measured the Swiss pool instead and let a draft
  // with nobody in it through.
  it("counts players the cup or a long game has taken", () => {
    // Whoever the bracket or a long board has claimed, this function neither
    // knows nor cares — it only asks who was marked absent.
    const wholeFieldInTheCup = field(1, 2, 3, 4);
    expect(attendingPlayers(wholeFieldInTheCup, [])).toHaveLength(4);
    expect(attendingPlayers(wholeFieldInTheCup, []).length).toBeGreaterThanOrEqual(
      MIN_PRESENT_PLAYERS,
    );
  });

  it("is empty when every player is marked absent", () => {
    expect(attendingPlayers(field(1, 2, 3, 4), [1, 2, 3, 4])).toEqual([]);
  });

  // A round only ever contains numbered players — the server filters on the
  // same thing, so an unnumbered registration must not prop the count up.
  it("ignores players with no tournament number", () => {
    const players = [player(1), player(null), player(null)];
    expect(attendingPlayers(players, []).map((p) => p.tournament_id)).toEqual([1]);
  });

  it("matches the server's minimum", () => {
    expect(MIN_PRESENT_PLAYERS).toBe(2);
  });
});
