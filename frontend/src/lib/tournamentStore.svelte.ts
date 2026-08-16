/**
 * The open tournament as this client sees it, and the two ways it gets there:
 * applying the response of an edit we made, and resyncing the server's own view
 * after somebody else's.
 *
 * Every mutating API call answers with the whole updated tournament, so there is
 * exactly one write path — {@link TournamentStore.apply} — and the fields below
 * are read-only to everything else. That is what makes the resync safe: there is
 * no half-edited local state a refetch could clobber.
 */
import { fetchTournament } from "./api";
import type {
  BlockedReason,
  CupBracketView,
  CupPodium,
  Handicap,
  Standing,
  TeamMatchView,
  TeamStanding,
  Tournament,
  TournamentResponse,
  UndoLabel,
  Winner,
} from "./types";

export class TournamentStore {
  tournament = $state<Tournament | null>(null);
  standings = $state<Standing[]>([]);
  /** Ranked team standings, in team mode only — the primary table there, with
   *  the player standings staying as the per-player breakdown. */
  teamStandings = $state<TeamStanding[]>([]);
  /** Each round's team matches, scored, server-derived (see
   *  `TournamentResponse.team_matches`), indexed like `tournament.rounds`. */
  teamMatches = $state<TeamMatchView[][]>([]);
  cupPodium = $state<CupPodium | null>(null);
  cupBracket = $state<CupBracketView | null>(null);
  /** Players the cup pairs in the round being drafted, and the players a long
   *  game from the previous round keeps out of it — both server-computed, so
   *  the draft UI hides exactly who the pairing will refuse to place. */
  draftCupPlayers = $state<number[]>([]);
  draftLongPlayers = $state<number[]>([]);
  suggestedHandicaps = $state<(Handicap | null)[][]>([]);
  /** Winner that counts for standings/pairing per board, server-computed (see
   *  `TournamentResponse.effective_winners`), indexed like `tournament.rounds`. */
  effectiveWinners = $state<(Winner | null)[][]>([]);
  /** What an undo would revert, or `null` when there is nothing to undo —
   *  which is also what disables the button. Server-side session state: the
   *  history lives there, and so does the decision about how to describe its
   *  top (see `crates/server/src/undo.rs`). */
  undoLabel = $state<UndoLabel | null>(null);
  /**
   * `false` when the server could not write the state it just answered with to
   * disk (see `TournamentResponse.persisted`). The edit *was* applied, and it
   * only lives in memory: shown as a standing banner, because every later edit
   * comes back `200` too and nothing else would ever say so.
   */
  persisted = $state(true);
  /** Why the server would refuse to start the next round / export the grid, or
   *  `null` if it would go ahead. Server-computed, so the buttons that read them
   *  cannot disagree with the guards that enforce them. */
  nextRoundBlocked = $state<BlockedReason | null>(null);
  gridExportBlocked = $state<BlockedReason | null>(null);

  /**
   * Whether the in-memory tournament has changes not yet written to a file —
   * purely client-side bookkeeping (the server has no notion of "saved to
   * disk"), and deliberately not part of the undo stack: undoing a change
   * doesn't retroactively make the tournament "saved" again. Set by every
   * {@link apply}; cleared only by {@link markSaved}.
   */
  hasUnsavedChanges = $state(false);

  /**
   * The version of the tournament state currently displayed. Used to reject a
   * background resync that resolved out of order (older than what we already
   * show), so it can't briefly revert a just-applied edit. Reset on a tournament
   * switch; a reconnect resync applies unconditionally (the server may have
   * restarted with a lower counter), see {@link refetch}.
   */
  #appliedVersion: number | null = null;

  /** Apply a tournament API response to local state. */
  apply(res: TournamentResponse) {
    this.#adopt(res);
    this.hasUnsavedChanges = true;
  }

  /** As {@link apply}, but for state we did not just edit — a load, a resync —
   *  which leaves the unsaved-changes flag alone. */
  #adopt(res: TournamentResponse) {
    this.tournament = res.tournament;
    this.standings = res.standings;
    this.teamStandings = res.team_standings ?? [];
    this.teamMatches = res.team_matches ?? [];
    this.cupPodium = res.cup_podium ?? null;
    this.cupBracket = res.cup_bracket ?? null;
    this.draftCupPlayers = res.draft_cup_players ?? [];
    this.draftLongPlayers = res.draft_long_players ?? [];
    this.suggestedHandicaps = res.suggested_handicaps ?? [];
    this.effectiveWinners = res.effective_winners ?? [];
    this.undoLabel = res.undo_label ?? null;
    this.persisted = res.persisted;
    this.nextRoundBlocked = res.next_round_blocked ?? null;
    this.gridExportBlocked = res.grid_export_blocked ?? null;
    this.#appliedVersion = res.version;
  }

  /**
   * The initial (or post-login) load: adopt whatever the server holds as state
   * that came from *there* rather than from an edit of ours — resuming it is
   * nothing to warn about losing. Returns false when the tournament no longer
   * exists (deleted from another client or tab).
   */
  async load(): Promise<boolean> {
    const res = await fetchTournament();
    if (!res) return false;
    this.#adopt(res);
    this.hasUnsavedChanges = false;
    return true;
  }

  /** Note that what is displayed is now on disk (a successful save), or came
   *  from there (a load, a restored backup). */
  markSaved() {
    this.hasUnsavedChanges = false;
  }

  /** Clear everything, so a moment of the previous tournament's state never
   *  shows under the newly selected one's name. */
  reset() {
    this.tournament = null;
    this.standings = [];
    this.teamStandings = [];
    this.teamMatches = [];
    this.cupPodium = null;
    this.cupBracket = null;
    this.draftCupPlayers = [];
    this.draftLongPlayers = [];
    this.suggestedHandicaps = [];
    this.effectiveWinners = [];
    this.undoLabel = null;
    this.persisted = true;
    this.nextRoundBlocked = null;
    this.gridExportBlocked = null;
    this.#appliedVersion = null;
    this.hasUnsavedChanges = false;
  }

  /**
   * Silently pull the authoritative state — after a live change pushed by
   * another referee, on reconnect, or after a rejected (conflicting) edit. Safe
   * because every edit already lives on the server; there is no unsaved local
   * state to lose.
   */
  async refetch(force = false) {
    try {
      const res = await fetchTournament();
      // Ignore a resync that resolved out of order — older than what we already
      // display — so a slow GET racing an in-flight mutation can't momentarily
      // revert the just-applied edit. A reconnect passes `force`, since a
      // restarted server may legitimately report a *lower* version.
      if (
        res &&
        (force || this.#appliedVersion === null || res.version >= this.#appliedVersion)
      ) {
        // A resync/live update isn't a local edit, so it must not flip the
        // "unsaved changes" flag.
        this.#adopt(res);
      }
    } catch {
      /* transient; the SSE stream will trigger another refetch on the next change */
    }
  }
}
