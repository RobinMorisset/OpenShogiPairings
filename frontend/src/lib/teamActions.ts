/**
 * The team-roster actions, as one group.
 *
 * Each is a single request whose response carries the whole updated tournament,
 * so the panel re-renders from the server's own view of the rosters rather than
 * a local guess at them — which is why they are all the same two lines, and why
 * they belong together rather than among the app's other glue.
 */
import * as api from "./api";
import type { TournamentResponse } from "./types";

interface Deps {
  /** Run an async action with the app's shared busy/error handling. */
  run: (action: () => Promise<void>) => void;
  /** Apply the response to the displayed tournament. */
  apply: (res: TournamentResponse) => void;
}

export function createTeamActions({ run, apply }: Deps) {
  return {
    add: (name: string) => run(async () => apply(await api.addTeam(name))),
    rename: (teamId: string, name: string) =>
      run(async () => apply(await api.renameTeam(teamId, name))),
    remove: (teamId: string) => run(async () => apply(await api.removeTeam(teamId))),
    addMember: (teamId: string, playerId: string) =>
      run(async () => apply(await api.addTeamMember(teamId, playerId))),
    removeMember: (teamId: string, playerId: string) =>
      run(async () => apply(await api.removeTeamMember(teamId, playerId))),
    setBoardOrder: (teamId: string, order: string[]) =>
      run(async () => apply(await api.setTeamBoardOrder(teamId, order))),
    sortByRating: (teamId: string) =>
      run(async () => apply(await api.sortTeamByRating(teamId))),
    addAdjustment: (teamId: string, delta: number, reason: string) =>
      run(async () => apply(await api.addTeamAdjustment(teamId, delta, reason))),
    removeAdjustment: (teamId: string, adjustmentId: string) =>
      run(async () => apply(await api.removeTeamAdjustment(teamId, adjustmentId))),
    /** A player's rating *for team pairing*, which the referee may override —
     *  hence a team action rather than a registration one. */
    setPairingRating: (playerId: string, rating: number | null) =>
      run(async () => apply(await api.setPairingRating(playerId, rating))),
  };
}
