// Team helpers shared by the Teams panel, the round view and the standings.
//
// These mirror `crates/core/src/team.rs` and `team_scoring.rs`: the same
// derivations, done client-side so a panel can show a team's average or group a
// round's boards into matches without a round-trip. The server stays the
// authority — it re-derives all of this when it pairs and when it ranks.

import type { Board, Player, Round, Team } from "./types";

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
 * The average pairing rating over a roster, rounded to nearest, or `null` when
 * no member has one. Unrated members are left out rather than dragging it down
 * (mirrors `osp_core::average_pairing_rating`).
 */
export function teamAverageRating(members: Player[]): number | null {
  const rated = members.map(pairingRating).filter((r): r is number => r != null);
  if (rated.length === 0) return null;
  const sum = rated.reduce((a, b) => a + b, 0);
  return Math.round(sum / rated.length);
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
