// Team helpers shared by the Teams panel, the round view and the standings.
//
// These mirror `crates/core/src/team.rs` and `team_scoring.rs`: the same
// derivations, done client-side so a panel can show a team's average or group a
// round's boards into matches without a round-trip. The server stays the
// authority — it re-derives all of this when it pairs and when it ranks.

import { forfeitOf } from "./boardOutcome";
import { absent, isDecided } from "./noShow";
import type { Board, Player, Round, Team, Winner } from "./types";

/**
 * A player's **pairing rating**: their real rating, or the referee-assigned
 * stand-in for an unrated team member.
 *
 * Mirrors `osp_core::pairing_rating`. It is what team averages and the board
 * order are computed from — never what an export shows, so a player carrying
 * only a pairing ELO stays unrated everywhere user-facing.
 */
export function pairingRating(player: Player): number | null {
  return player.rating ?? player.pairing_rating ?? null;
}

/**
 * The average pairing rating over a roster, rounded to nearest — `null` unless
 * every member has one, and for an empty roster.
 *
 * Mirrors `average_pairing_rating` in `crates/core/src/team.rs`: a mean over the rated members only
 * would describe part of the team as if it were the whole, so a roster with
 * anyone unrated shows no average at all.
 */
export function teamAverageRating(members: Player[]): number | null {
  if (members.length === 0) return null;
  const rated = members.map(pairingRating).filter((r): r is number => r != null);
  if (rated.length !== members.length) return null;
  const sum = rated.reduce((a, b) => a + b, 0);
  return Math.round(sum / rated.length);
}

/**
 * Is this roster already in board order — descending pairing rating, unrated
 * last?
 *
 * Mirrors the order `sort_team_by_rating` produces, so a team the button would
 * leave untouched can say so by greying it out. Equal ratings are in order
 * either way round: that sort is stable, so it wouldn't move them.
 */
export function sortedByRating(members: Player[]): boolean {
  // `null` is weaker than any rating, and equal to itself.
  const rank = (p: Player) => pairingRating(p) ?? -1;
  return members.every((p, i) => i === 0 || rank(members[i - 1]) >= rank(p));
}

/** One team match of a round: the two teams, and the boards it is made of. */
export interface TeamMatch {
  team1: Team;
  team2: Team;
  /** Indices into `round.boards`, in board order. */
  boards: number[];
}

/**
 * Group a round's boards into team matches, from the rosters alone.
 *
 * Mirrors `matches_in_round`: a board's two players name their teams, and the
 * match a board belongs to needs no storage at all. `team1` is the team on
 * `player1`, matching the server's orientation. Boards whose players aren't both
 * in known teams are skipped (they belong to no match).
 */
export function teamMatches(round: Round, teams: Team[], players: Player[]): TeamMatch[] {
  const teamOfPlayer = new Map<number, Team>();
  const boardOfPlayer = new Map<number, number>();
  const tidOf = new Map<string, number>();
  for (const p of players) if (p.tournament_id != null) tidOf.set(p.id, p.tournament_id);
  for (const team of teams) {
    team.members.forEach((member, board) => {
      const tid = tidOf.get(member);
      if (tid == null) return;
      teamOfPlayer.set(tid, team);
      boardOfPlayer.set(tid, board);
    });
  }

  const byPair = new Map<string, TeamMatch>();
  round.boards.forEach((board: Board, index: number) => {
    const t1 = teamOfPlayer.get(board.player1);
    const t2 = teamOfPlayer.get(board.player2);
    if (!t1 || !t2) return;
    const key = [t1.id, t2.id].sort().join("|");
    let match = byPair.get(key);
    if (!match) {
      match = { team1: t1, team2: t2, boards: [] };
      byPair.set(key, match);
    }
    match.boards.push(index);
  });

  const matches = [...byPair.values()];
  for (const m of matches) {
    m.boards.sort(
      (a, b) =>
        (boardOfPlayer.get(round.boards[a].player1) ?? 0) -
        (boardOfPlayer.get(round.boards[b].player1) ?? 0),
    );
  }
  // Stable order: by the lower team number, then the higher — the same ordering
  // the server produces, so board numbering agrees between the two.
  const rank = (t: Team) => t.tournament_id ?? Number.MAX_SAFE_INTEGER;
  matches.sort((x, y) => {
    const kx: [number, number] = [
      Math.min(rank(x.team1), rank(x.team2)),
      Math.max(rank(x.team1), rank(x.team2)),
    ];
    const ky: [number, number] = [
      Math.min(rank(y.team1), rank(y.team2)),
      Math.max(rank(y.team1), rank(y.team2)),
    ];
    return kx[0] - ky[0] || kx[1] - ky[1];
  });
  return matches;
}

/** A match's board wins on each side, and whether every board of it is decided. */
export interface MatchScore {
  wins1: number;
  wins2: number;
  decided: boolean;
}

/**
 * Count a match's board wins, from `team1`'s side first.
 *
 * Mirrors `match_result`: a board win is the board's **effective** winner — so
 * the Wiel handicap rule applies exactly as it does to a player's own score,
 * which is why the server-computed winners are passed in rather than re-derived
 * — plus a forfeit where only one side turned up. A board nobody turned up for
 * has no winner, which is how an odd-sized match can still end level.
 */
export function matchScore(
  round: Round,
  match: TeamMatch,
  effectiveWinners: (Winner | null)[],
): MatchScore {
  const score: MatchScore = { wins1: 0, wins2: 0, decided: true };
  for (const index of match.boards) {
    const board = round.boards[index];
    const forfeit = forfeitOf(board);
    if (!isDecided(board)) {
      score.decided = false;
      continue;
    }
    // On a forfeit the point goes to whoever turned up, whichever reason the
    // missing side had; under `both` nobody did, so the board decides nothing.
    const side: Winner | null = forfeit
      ? absent(forfeit, "player1")
        ? absent(forfeit, "player2")
          ? null
          : "player2"
        : "player1"
      : (effectiveWinners[index] ?? null);
    if (side === "player1") score.wins1 += 1;
    else if (side === "player2") score.wins2 += 1;
  }
  return score;
}
