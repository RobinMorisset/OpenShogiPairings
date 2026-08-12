// The ratings a roster is built and ordered by — what the Teams panel needs
// while a roster is still being edited.
//
// These mirror `pairing_rating` / `average_pairing_rating` / `sort_team_by_rating`
// in `crates/core/src/team.rs`, and they are client-side for one reason: they
// answer questions about a roster the referee is *changing* — the average this
// team would have with that member added, whether the sort button would move
// anything — which the server has not been asked about and should not be, once
// per keystroke.
//
// Anything derived from a roster that is already frozen comes from the server
// instead: a round's boards arrive already grouped into their team matches, each
// with its score (`TeamMatchView`, see `Tournament::team_matches`), so no client
// re-derives that grouping.

import type { Player } from "./types";

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
