// Who a round draft will actually include.
//
// Two different populations are easy to confuse here, and confusing them is
// what let a draft with nobody in it reach the server:
//
//   - the *Swiss pool* — players the engine will pair, which excludes those the
//     cup bracket has already taken and those still busy on a long game;
//   - the *attending* field — everyone the round includes at all, cup and
//     long-game players among them.
//
// The minimum-players rule is about the second. A cup round can legitimately
// have an empty Swiss pool (every player is in the bracket) and still be a
// perfectly valid round.

import type { Player } from "./types";

/**
 * The fewest players a round can be confirmed with. Mirrors
 * `MIN_PLAYERS_PER_ROUND` in `crates/core/src/tournament.rs`.
 */
export const MIN_PRESENT_PLAYERS = 2;

/**
 * Everyone a confirmed round would include: registered players not marked
 * absent in the draft. Mirrors the `present` set in `confirm_round`
 * (`crates/core/src/tournament.rs`) — including the `tournament_id` filter,
 * which is what makes a player part of a round at all.
 */
export function attendingPlayers(players: Player[], absent: number[]): Player[] {
  const away = new Set(absent);
  return players.filter((p) => p.tournament_id != null && !away.has(p.tournament_id));
}
