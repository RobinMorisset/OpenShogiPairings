import { describe, expect, it } from "vitest";

import { matchScore, pairingRating, teamAverageRating, teamMatches } from "./teams";
import type { Board, Player, Round, Team } from "./types";

/** Only the fields these helpers read; the rest of `Player` is irrelevant. */
const player = (id: string, tid: number, rating: number | null, pairing?: number): Player =>
  ({ id, tournament_id: tid, rating, pairing_rating: pairing }) as Player;

const board = (player1: number, player2: number, fields: Partial<Board> = {}): Board =>
  ({ player1, player2, ...fields }) as Board;

const round = (boards: Board[]): Round => ({ number: 1, boards, completed: false }) as Round;

const team = (id: string, tid: number, members: string[]): Team =>
  ({ id, tournament_id: tid, name: id, members }) as Team;

describe("pairingRating", () => {
  it("prefers the real rating, and falls back to the referee's", () => {
    expect(pairingRating(player("a", 1, 1800, 1200))).toBe(1800);
    expect(pairingRating(player("a", 1, null, 1200))).toBe(1200);
    expect(pairingRating(player("a", 1, null))).toBe(null);
  });
});

describe("teamAverageRating", () => {
  it("skips unrated members and rounds to nearest", () => {
    const members = [player("a", 1, 2000), player("b", 2, 1801), player("c", 3, null)];
    expect(teamAverageRating(members)).toBe(1901);
  });

  it("has no average when nobody is rated — rather than a fake zero", () => {
    expect(teamAverageRating([player("a", 1, null)])).toBe(null);
  });
});

describe("teamMatches", () => {
  const players = [
    player("a1", 1, 2000),
    player("a2", 2, 1900),
    player("b1", 3, 1800),
    player("b2", 4, 1700),
  ];
  const teams = [team("A", 1, ["a1", "a2"]), team("B", 2, ["b1", "b2"])];

  it("groups a round's boards into the match they belong to", () => {
    const r = round([board(1, 3), board(2, 4)]);
    const matches = teamMatches(r, teams, players);
    expect(matches).toHaveLength(1);
    expect(matches[0].team1.id).toBe("A");
    expect(matches[0].team2.id).toBe("B");
    expect(matches[0].boards).toEqual([0, 1]);
  });

  // The grouping is derived from the rosters alone, so the boards need not be
  // stored in board order — they are sorted into it here.
  it("puts a match's boards in board order whatever order they are stored in", () => {
    const r = round([board(2, 4), board(1, 3)]);
    expect(teamMatches(r, teams, players)[0].boards).toEqual([1, 0]);
  });

  it("skips a board whose players aren't both in known teams", () => {
    const r = round([board(1, 99)]);
    expect(teamMatches(r, teams, players)).toEqual([]);
  });
});

describe("matchScore", () => {
  const players = [
    player("a1", 1, 2000),
    player("a2", 2, 1900),
    player("a3", 5, 1850),
    player("b1", 3, 1800),
    player("b2", 4, 1700),
    player("b3", 6, 1650),
  ];
  const teams = [team("A", 1, ["a1", "a2", "a3"]), team("B", 2, ["b1", "b2", "b3"])];

  it("counts the effective winner of each board, from team1's side first", () => {
    // The board's own outcome says *whether* it is decided; the effective winner
    // (server-computed, so the Wiel rule is already applied) says for whom — on
    // a handicap board under that rule the two can disagree, which is the point
    // of taking the winners from the server rather than re-deriving them.
    const r = round([
      board(1, 3, { outcome: { kind: "won", winner: "player2" } }),
      board(2, 4, { outcome: { kind: "won", winner: "player2" } }),
      board(5, 6, { outcome: { kind: "won", winner: "player1" } }),
    ]);
    const m = teamMatches(r, teams, players)[0];
    const score = matchScore(r, m, ["player1", "player2", "player1"]);
    expect(score).toEqual({ wins1: 2, wins2: 1, decided: true });
  });

  it("credits a forfeit to the side that turned up", () => {
    const r = round([
      board(1, 3, { outcome: { kind: "forfeit", absent: "player2" } }),
      board(2, 4, { outcome: { kind: "won", winner: "player2" } }),
      board(5, 6, { outcome: { kind: "won", winner: "player2" } }),
    ]);
    const m = teamMatches(r, teams, players)[0];
    const score = matchScore(r, m, [null, "player2", "player2"]);
    expect(score).toEqual({ wins1: 1, wins2: 2, decided: true });
  });

  // A board nobody turned up for decides nothing, which is how a match of odd
  // size can still end level.
  it("gives a double no-show to neither side", () => {
    const r = round([
      board(1, 3, { outcome: { kind: "won", winner: "player1" } }),
      board(2, 4, { outcome: { kind: "won", winner: "player2" } }),
      board(5, 6, { outcome: { kind: "forfeit", absent: "both" } }),
    ]);
    const m = teamMatches(r, teams, players)[0];
    const score = matchScore(r, m, ["player1", "player2", null]);
    expect(score).toEqual({ wins1: 1, wins2: 1, decided: true });
  });

  it("is undecided while any board of the match is unplayed", () => {
    const r = round([
      board(1, 3, { outcome: { kind: "won", winner: "player1" } }),
      board(2, 4),
      board(5, 6),
    ]);
    const m = teamMatches(r, teams, players)[0];
    expect(matchScore(r, m, ["player1", null, null])).toEqual({
      wins1: 1,
      wins2: 0,
      decided: false,
    });
  });
});
