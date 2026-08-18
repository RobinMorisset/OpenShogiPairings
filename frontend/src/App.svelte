<script lang="ts">
  import { _, locale } from "svelte-i18n";
  import { untrack } from "svelte";
  import {
    addPlayer,
    addPointAdjustment,
    ApiError,
    cancelRound,
    checkLicences,
    confirmRound,
    editPlayer,
    fetchAmericanGrid,
    fetchBackups,
    fetchCounterfactual,
    forcePairing,
    fetchRatings,
    importPlayersCsv,
    prepareRound,
    publicPageUrl,
    refreshRatings,
    removePlayer,
    removePointAdjustment,
    restoreBackup,
    setBoardDrawn,
    setBoardHandicap,
    setBoardLong,
    setBoardNoShow,
    setBoardWinner,
    setPlayerEligible,
    setPlayersEligible,
    setPlayerCategory,
    setSitoutValue,
    setTeamSitoutValue,
    undoTournament,
    updateDraft,
    updateSettings,
    subscribeToChanges,
    type DraftUpdate,
  } from "./lib/api";
  import { describeApiError, describeCoded } from "./lib/errorCodes";
  import { isDecided } from "./lib/noShow";
  import { pairingRating } from "./lib/teams";
  import type {
    BackupList,
    BlockedReason,
    Handicap,
    LicenceCheck,
    NewPlayer,
    Forfeit,
    RatedPlayer,
    SitoutValue,
    TournamentResponse,
    TournamentSettings,
    Winner,
  } from "./lib/types";
  import { saveAmericanGrid, saveTournament } from "./lib/tournamentFile";
  import { pickCsvFile } from "./lib/csvImport";
  import { handicapChoice } from "./lib/handicap";
  import { buildSheetPlayers, macMahonRowShown } from "./lib/resultSheets";
  import { Publication, PrintJobs } from "./lib/publication.svelte";
  import { createTeamActions } from "./lib/teamActions";
  import { TournamentStore } from "./lib/tournamentStore.svelte";
  import ContentCard from "./lib/components/ContentCard.svelte";
  import PageShell from "./lib/components/PageShell.svelte";
  import TabStrip, { type Tab } from "./lib/components/TabStrip.svelte";
  import QrCode from "./lib/components/QrCode.svelte";
  import ServerStatus from "./lib/components/ServerStatus.svelte";
  import Login from "./lib/components/Login.svelte";
  import TournamentPicker from "./lib/components/TournamentPicker.svelte";
  import { authRequired, currentTournamentId, initialTab } from "./lib/session";
  import PlayerRegistration from "./lib/components/PlayerRegistration.svelte";
  import PlayerList from "./lib/components/PlayerList.svelte";
  import LicenceCheckPanel from "./lib/components/LicenceCheckPanel.svelte";
  import TeamsPanel from "./lib/components/TeamsPanel.svelte";
  import RoundView from "./lib/components/RoundView.svelte";
  import RoundDraftView from "./lib/components/RoundDraftView.svelte";
  import ResultsView from "./lib/components/ResultsView.svelte";
  import ResultSheets from "./lib/components/ResultSheets.svelte";
  import CupBracket from "./lib/components/CupBracket.svelte";
  import TournamentSettingsView from "./lib/components/TournamentSettingsView.svelte";
  import LocaleSwitcher from "./lib/components/LocaleSwitcher.svelte";
  import ThemeSwitcher from "./lib/components/ThemeSwitcher.svelte";
  import ConnectionStatus from "./lib/components/ConnectionStatus.svelte";

  // The open tournament and the two ways it gets here (an edit's response, a
  // resync of somebody else's) live in `tournamentStore.svelte.ts`. The aliases
  // below are read-only views of it, so the rest of this component — and all of
  // its markup — still names the state directly, while every write goes through
  // one of the store's methods.
  const store = new TournamentStore();
  const tournament = $derived(store.tournament);
  const standings = $derived(store.standings);
  const teamStandings = $derived(store.teamStandings);
  const teamMatches = $derived(store.teamMatches);
  const cupPodium = $derived(store.cupPodium);
  const cupBracket = $derived(store.cupBracket);
  const draftCupPlayers = $derived(store.draftCupPlayers);
  const draftLongPlayers = $derived(store.draftLongPlayers);
  const suggestedHandicaps = $derived(store.suggestedHandicaps);
  const effectiveWinners = $derived(store.effectiveWinners);
  const canUndo = $derived(store.canUndo);
  const hasUnsavedChanges = $derived(store.hasUnsavedChanges);
  const persisted = $derived(store.persisted);
  const apply = (res: TournamentResponse) => store.apply(res);
  const refetch = (force = false) => store.refetch(force);

  let initialLoad = $state<"loading" | "done">("loading");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let ratings = $state<RatedPlayer[]>([]);

  /** Warn before an action that discards the current tournament, if it has
   * unsaved changes. Returns whether the caller should proceed. */
  function confirmDiscard(): boolean {
    if (!hasUnsavedChanges || !tournament) return true;
    return window.confirm(
      $_("app.confirmDiscard", { values: { name: tournament.name } }),
    );
  }

  // Which tab is open: "players", "results", or "round-{n}".
  let activeTab = $state("players");

  /**
   * Whether the open tab's content is a table that will not wrap — so the card
   * should grow to stay behind it instead of letting it overflow the rounded
   * panel (`ContentCard`'s `wide`).
   *
   * Deliberately a list rather than "always on": the card is sized with
   * `max-content`, which asks the content how wide it would like to be with
   * nothing wrapped. For a table that is its natural width; for the prose on
   * the Settings tab it would be the whole paragraph on one line.
   */
  const tabHasWideTable = $derived(
    activeTab === "results" || activeTab === "players" || activeTab.startsWith("round-"),
  );

  const activeRound = $derived(
    tournament?.rounds.find((r) => `round-${r.number}` === activeTab) ?? null,
  );

  // The Results (standings) tab only appears once a round has been played:
  // before that everyone just sits at their MacMahon start, so there is nothing
  // to rank (and the server returns no standings until registration is
  // finalized anyway).
  const showResults = $derived((tournament?.rounds.length ?? 0) > 0);

  // The Cup tab appears once the bracket is frozen — i.e. registration has been
  // finalized with a cup enabled, at which point `tournament.cup` is set and the
  // seed order can't change. Sits next to Standings.
  const showCup = $derived(tournament?.cup != null);

  // Team mode: teams are the unit of pairing, so registration grows a Teams tab
  // (next to Players, where the rosters are built) and the rest of the app
  // groups by match. Declared here because the tab list below reads it.
  const teamMode = $derived(tournament?.settings.teams != null);

  // The tab strip: what it shows, in the order it shows it. A list rather than
  // markup because two other things read it — the arrow-key shortcuts below,
  // and `TabStrip`, which the reader page and the static export render from
  // their own lists so that a tab cannot look like one thing here and another
  // there.
  const tabs = $derived<Tab[]>(
    tournament
      ? [
          { id: "settings", label: $_("app.tabSettings"), testid: "tab-settings" },
          {
            id: "players",
            label: $_("app.tabPlayers", { values: { count: tournament.players.length } }),
            testid: "tab-players",
          },
          ...(teamMode
            ? [
                {
                  id: "teams",
                  label: $_("app.tabTeams", { values: { count: (tournament.teams ?? []).length } }),
                  testid: "tab-teams",
                },
              ]
            : []),
          ...(showResults
            ? [{ id: "results", label: $_("app.tabResults"), testid: "tab-results" }]
            : []),
          ...(showCup ? [{ id: "cup", label: $_("app.tabCup"), testid: "tab-cup" }] : []),
          ...tournament.rounds.map((r) => ({
            id: `round-${r.number}`,
            label: r.completed
              ? $_("app.tabRoundCompleted", { values: { number: r.number } })
              : $_("app.tabRound", { values: { number: r.number } }),
            testid: `tab-round-${r.number}`,
          })),
          ...(tournament.draft
            ? [
                {
                  id: "draft",
                  label: $_("app.tabRoundDraft", { values: { number: tournament.draft.number } }),
                  testid: "tab-draft",
                  // The round being prepared is not one of the settled ones.
                  accent: true,
                },
              ]
            : []),
        ]
      : [],
  );

  /** The tabs the arrow keys step through — the strip's own order, by
   *  construction rather than by a second list kept in sync with it. */
  const tabOrder = $derived(tabs.map((t) => t.id));

  // The suggested-handicap slice for the active round, matched by position
  // (rounds are numbered sequentially without gaps).
  const activeRoundSuggested = $derived.by(() => {
    if (!tournament || !activeRound) return [];
    const idx = tournament.rounds.findIndex((r) => r.number === activeRound.number);
    return idx >= 0 ? (suggestedHandicaps[idx] ?? []) : [];
  });

  // The team matches of the round on screen, the grouping its board table
  // follows — the server's own, so the round view and the standings cannot show
  // two different matches or two different scores.
  const activeRoundMatches = $derived.by(() => {
    if (!tournament || !activeRound) return [];
    const idx = tournament.rounds.findIndex((r) => r.number === activeRound.number);
    return idx >= 0 ? (teamMatches[idx] ?? []) : [];
  });

  // Why the engine chose the active round's pairings. Frozen onto the round when
  // it was confirmed, so it travels with the tournament and needs no round-trip
  // of its own — and stays the ledger the round was really paired from, whatever
  // has been edited since (see the appendix of docs/archive/public-access.md).
  //
  // What *can* go stale is the data it cites: the round itself says whether its
  // explanation still matches the present, and one that no longer does is shown
  // with a warning rather than silently. The server decides it (an edit clears
  // the flag on the rounds it can have disturbed), so this only reads it.
  const explanationStale = $derived(!!activeRound && !activeRound.pairing_explanation_valid);

  // Tournament phase, derived from the finalize flag, the draft, and the last
  // round's state.
  type Phase = "registration" | "ready" | "draft" | "in_progress";
  const currentRound = $derived(tournament?.rounds.at(-1) ?? null);
  const phase = $derived<Phase>(
    !tournament || !tournament.registration_finalized
      ? "registration"
      : tournament.draft
        ? "draft"
        : !currentRound || currentRound.completed
          ? "ready"
          : "in_progress",
  );
  // Why the server would refuse to start the next round / export the grid, or
  // `null` if it would go ahead. Computed by the same code that enforces it
  // (`Tournament::next_round_blocker`, `grid_export_blocker`), so the button and
  // the refusal cannot disagree — which is exactly what went wrong when these
  // rules were mirrored in TypeScript instead.
  const nextRoundBlocked = $derived(store.nextRoundBlocked);
  const gridExportBlocked = $derived(store.gridExportBlocked);
  /** The translated sentence for a blocked reason, falling back to its code. */
  function blockedText(reason: BlockedReason | null): string {
    if (!reason) return "";
    return describeCoded(reason.code, reason.values, $_) ?? reason.code;
  }

  const enoughPlayers = $derived((tournament?.players.length ?? 0) >= 2);

  // The active round can be re-paired (via "force this pairing") only if it is
  // the current, in-progress round with nothing decided yet. `isDecided` covers
  // no-shows as well as results, matching the server's guard — a forfeited board
  // has no `result`, so checking that alone would leave the button enabled on a
  // request `force_pairing` rejects.
  const canForceActiveRound = $derived(
    !!activeRound &&
      !!currentRound &&
      activeRound.number === currentRound.number &&
      !activeRound.completed &&
      !activeRound.boards.some(isDecided),
  );

  // Hybrid cup: which bracket sizes are choosable at finalization depends on how
  // many players the referee has marked eligible — and, under the qualifier
  // format, a bracket of `size` takes half as many players again (mirrors
  // `cup_field_size` in the backend).
  const cupEnabled = $derived(tournament?.settings.cup_enabled ?? false);
  const teamSize = $derived(tournament?.settings.teams?.size ?? 1);
  // MacMahon starting points in use — the one configuration where an unrated
  // team member needs a referee-assigned pairing ELO.
  const macmahonInUse = $derived.by(() => {
    const pairing = tournament?.settings.pairing;
    return pairing?.kind === "swiss" && (pairing.macmahon.thresholds ?? []).length > 0;
  });
  const cupFormat = $derived(tournament?.settings.cup_format ?? "direct");
  const eligibleCount = $derived(
    tournament?.players.filter((p) => p.eligible).length ?? 0,
  );
  const cupFieldSize = $derived((size: number) =>
    cupFormat === "qualifier" ? size + size / 2 : size,
  );
  const validCupSizes = $derived(
    [8, 16, 32, 64].filter((s) => cupFieldSize(s) <= eligibleCount),
  );
  let cupSizeChoice = $state<number | null>(null);
  // Default the size to the largest that fits, and keep it valid as eligibility
  // changes.
  $effect(() => {
    if (!cupEnabled) return;
    if (cupSizeChoice == null || !validCupSizes.includes(cupSizeChoice)) {
      cupSizeChoice = validCupSizes.at(-1) ?? null;
    }
  });
  const cupReady = $derived(!cupEnabled || cupSizeChoice != null);

  // "Start round" button. In the "registration" phase it also finalizes
  // registration (a single undo step); from round 2 on, registration is already
  // finalized and it just prepares the next round.
  const nextRoundNumber = $derived((tournament?.rounds.length ?? 0) + 1);
  // Preparing round 1 of a team tournament also finalizes the rosters, so the
  // button carries `finalize_teams`'s guards — in the same order, and phrased as
  // the error it would otherwise have raised. Without this it looked live and
  // the server refused. Empty once the rosters are ready (and from round 2 on,
  // where they are frozen and were validated here).
  const teamsNotReady = $derived.by(() => {
    if (!teamMode || phase !== "registration") return "";
    const teams = tournament?.teams ?? [];
    const players = tournament?.players ?? [];
    if (teams.length < 2) {
      return $_("serverError.notEnoughTeams", { values: { have: teams.length } });
    }
    const assigned = new Set(teams.flatMap((t) => t.members));
    const without = players.filter((p) => !assigned.has(p.id)).length;
    if (without > 0) {
      return $_("serverError.playersWithoutTeam", { values: { count: without } });
    }
    const short = teams.find((t) => t.members.length !== teamSize);
    if (short) {
      return $_("serverError.incompleteTeam", {
        values: { name: short.name, have: short.members.length, need: teamSize },
      });
    }
    if (macmahonInUse) {
      const missing = players.filter((p) => pairingRating(p) == null).length;
      if (missing > 0) {
        return $_("serverError.membersWithoutPairingRating", { values: { count: missing } });
      }
    }
    return "";
  });
  const startEnabled = $derived(
    !busy &&
      enoughPlayers &&
      teamsNotReady === "" &&
      // In registration the button finalizes *then* prepares, so the server's
      // "registration not finalized" is expected rather than blocking; from
      // `ready` on, its verdict is the whole answer.
      ((phase === "registration" && cupReady) ||
        (phase === "ready" && nextRoundBlocked === null)),
  );
  const startTitle = $derived(
    phase === "draft" || phase === "in_progress"
      ? $_("app.startTitleNotReady")
      : !enoughPlayers
        ? $_("app.startTitleNeedPlayers")
        : teamsNotReady !== ""
          ? teamsNotReady
          : nextRoundBlocked !== null && phase !== "registration"
            ? blockedText(nextRoundBlocked)
            : phase === "registration" && !cupReady
              ? $_("app.advanceTitleNeedCup", { values: { needed: cupFieldSize(8) } })
              : "",
  );

  // "Export grid" button: available only in the "ready" phase (no draft, no
  // round in progress) once at least one round has been completed.
  const completedRoundCount = $derived(
    tournament?.rounds.filter((r) => r.completed).length ?? 0,
  );
  const exportEnabled = $derived(
    !busy &&
      phase === "ready" &&
      completedRoundCount > 0 &&
      gridExportBlocked === null,
  );
  const exportTitle = $derived(
    completedRoundCount === 0
      ? $_("app.exportTitleNoRounds")
      : phase !== "ready"
        ? $_("app.exportTitleNotReady")
        : gridExportBlocked !== null
          ? blockedText(gridExportBlocked)
          : $_("app.exportTitleReady"),
  );

  // "Cancel last round" button: peels back one stage — discards the open draft,
  // or removes the most recent round — whenever there is anything to cancel.
  const canCancel = $derived(
    !busy && (!!tournament?.draft || (tournament?.rounds.length ?? 0) > 0),
  );
  const cancelTitle = $derived(
    tournament?.draft
      ? $_("app.cancelTitleDraft")
      : (tournament?.rounds.length ?? 0) > 0
        ? $_("app.cancelTitleRound")
        : $_("app.cancelTitleNothing"),
  );

  // Keep the selected tab valid (e.g. after undo or cancel-round removes a
  // round). Falls back to the last remaining round rather than the players
  // list, since that's still the state the referee cares about; only drops
  // to "players" when no round is left at all.
  $effect(() => {
    if (!tournament) return;
    const valid = new Set([
      "settings",
      "players",
      ...(teamMode ? ["teams"] : []),
      ...(showResults ? ["results"] : []),
      ...(showCup ? ["cup"] : []),
      ...tournament.rounds.map((r) => `round-${r.number}`),
      ...(tournament.draft ? ["draft"] : []),
    ]);
    if (!valid.has(activeTab)) {
      const lastRound = tournament.rounds.at(-1);
      activeTab = lastRound ? `round-${lastRound.number}` : "players";
    }
  });

  // Initial (and post-login) load of ratings + the open tournament. Re-run
  // after a successful sign-in, since the first attempt would have 401'd.
  async function loadInitial() {
    initialLoad = "loading";

    // Load the FESA ratings in the background — autocomplete is a nice-to-have,
    // so a failure here must not block or error the rest of the app. (Also
    // silently covers the admin password being required and not yet entered.)
    fetchRatings()
      .then((r) => (ratings = r))
      .catch(() => {
        /* autocomplete simply unavailable */
      });

    try {
      if (await store.load()) {
        // Pull the backups listing now rather than when the panel is first
        // opened: its directory is what the Backups button's tooltip names, and
        // a tooltip that only becomes true after you've clicked the button is
        // no use. Best-effort, like the ratings above — a failure here just
        // leaves the tooltip without its path until the panel is opened.
        fetchBackups()
          .then((listing) => (backupListing = listing))
          .catch(() => {
            /* the panel fetches it again when opened */
          });
        // Likewise for the publication state — see `Publication.load`.
        publication.load().catch(() => {
          /* the panel fetches it again when opened */
        });
      } else {
        // The tournament no longer exists (e.g. deleted from another
        // client/tab) — back to the picker.
        currentTournamentId.set(null);
      }
    } catch (err) {
      // A 401 just means "log in first" — the login overlay handles it, so
      // don't also surface it as an error banner.
      if (!(err instanceof ApiError && err.status === 401)) {
        error = describe(err);
      }
    } finally {
      initialLoad = "done";
    }
  }

  // (Re)load whenever the open tournament changes — including the very first
  // selection — and keep the live-sync subscription scoped to it.
  $effect(() => {
    if ($currentTournamentId === null) return;
    // Reset the view before loading the newly selected tournament, so a
    // moment of stale UI from the previous one never shows.
    store.reset();
    // Each tournament has its own backups directory, so the previous one's
    // listing (and the path in the tooltip) must not carry over.
    backupListing = null;
    publication.reset();
    error = null;
    // Names skipped by an import into the *previous* tournament say nothing
    // about this one.
    csvSkipped = [];
    // Read (and clear) the requested tab *untracked*: this effect must depend
    // only on `currentTournamentId`. Subscribing to `initialTab` here would let
    // the `set(null)` below re-trigger the effect and immediately reset the tab
    // back to "players", defeating the whole point.
    activeTab = untrack(() => $initialTab) ?? "players";
    initialTab.set(null);

    void loadInitial();
    // Live sync: refetch on another referee's change and on every (re)connect.
    // A reconnect forces the resync (the server may have restarted with a lower
    // version counter); a live "changed" event stays order-guarded in refetch.
    const unsubscribe = subscribeToChanges((reason) =>
      void refetch(reason === "reconnect"),
    );
    // Also resync when the tab regains focus/visibility, in case the SSE stream
    // was throttled or dropped while the tab was backgrounded.
    const onFocus = () => void refetch();
    const onVisible = () => {
      if (!document.hidden) void refetch();
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      unsubscribe();
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisible);
    };
  });

  function describe(err: unknown): string {
    return describeApiError(err, $_);
  }

  /** Run an async action with shared busy/error handling. */
  async function run(action: () => Promise<void>) {
    busy = true;
    error = null;
    // The CSV import's "already registered, not added again" notice describes
    // one action, and stops being true the moment another one lands: undo the
    // import and it is reporting players who are no longer registered at all.
    // So it is cleared here with the error, for the same reason and at the same
    // moment — anything the referee does next replaces it. The import puts its
    // own list back at the end of its action, after this has run.
    //   Only user-initiated actions reach this. The live-sync subscription, the
    //   focus handler and the visibility handler all call `refetch` directly, so
    //   another referee's edit arriving over SSE does not blank a notice the
    //   referee here is still reading.
    csvSkipped = [];
    try {
      await action();
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) {
        // Another referee changed the tournament first, so this edit was
        // rejected. Reload the authoritative state and say the action didn't take.
        error = $_("app.conflictReloaded");
        await refetch();
      } else {
        error = describe(err);
        // A 500 is the server telling us it broke *somewhere* in handling this
        // request — which may be after the edit itself had already been applied
        // (a panic while building the response is exactly that case). So the
        // view on screen can already be behind, and its version certainly is.
        // Reload rather than leave the referee reading a state that has moved,
        // and stale enough that their next edit would come back as somebody
        // else's conflict.
        if (err instanceof ApiError && err.status === 500) await refetch();
      }
    } finally {
      busy = false;
    }
  }

  function handleAddPlayer(player: NewPlayer) {
    run(async () => {
      apply(await addPlayer(player));
    });
  }

  // Players the last import left out because they were already registered (see
  // `handleImportCsv`). Shown until the next import, so the referee can compare
  // the list against their file.
  let csvSkipped = $state<string[]>([]);

  /**
   * Import players from a CSV file. The file's text is sent to the server, which
   * parses it (column detection, FESA enrichment) and registers the roster as a
   * single undo-able step. A malformed file surfaces as an error banner.
   *
   * Loading the same file twice registers nobody twice: the server skips the
   * players already registered and names them back, which we show — a skipped
   * row is a deliberate choice, not something to leave the referee to notice.
   */
  function handleImportCsv() {
    run(async () => {
      const text = await pickCsvFile();
      if (text === null) return; // cancelled
      const imported = await importPlayersCsv(text);
      apply(imported);
      csvSkipped = imported.skipped_duplicates;
    });
  }

  // The licence check (see `LicenceCheckPanel.svelte`): a toggled panel that
  // reports which registered players of one nationality are absent from a
  // federation's list of paid-up licences. Purely a question about the roster —
  // it registers nothing and edits nothing, so the answer is kept here rather
  // than folded into the tournament state.
  let showLicenceCheck = $state(false);
  let licenceCheck = $state<{ nationality: string; check: LicenceCheck } | null>(null);

  /**
   * Check `nationality` against a licence list the referee picks. The file's text
   * goes to the server, which parses it with the same rules as a CSV roster
   * import; a malformed file surfaces as an error banner and no answer at all,
   * so an unusable list can never read as "everybody has paid".
   */
  function handleCheckLicences(nationality: string) {
    licenceCheck = null;
    run(async () => {
      const text = await pickCsvFile();
      if (text === null) return; // cancelled
      licenceCheck = { nationality, check: await checkLicences(nationality, text) };
    });
  }

  function handleEditPlayer(id: string, player: NewPlayer) {
    run(async () => {
      apply(await editPlayer(id, player));
    });
  }

  function handleRemovePlayer(id: string) {
    run(async () => {
      apply(await removePlayer(id));
    });
  }

  // The team-roster actions, as one group (see `teamActions.ts`).
  const teams = createTeamActions({ run, apply });

  function handleToggleEligible(id: string, eligible: boolean) {
    run(async () => {
      apply(await setPlayerEligible(id, eligible));
    });
  }

  // Bulk cup-eligibility toggle: every player of the given nationality, in one
  // request. It used to be a loop over the single-player endpoint, which could
  // fail on the seventh of twelve and leave the roster in a state nobody asked
  // for — half eligible, half not, and no version at which the instruction had
  // been carried out. The server applies the whole list inside one mutation, so
  // it either all lands or none of it does, and one undo puts it back.
  function handleSetEligibleByNationality(nationality: string, eligible: boolean) {
    run(async () => {
      const ids = (tournament?.players ?? [])
        .filter((p) => (p.nationality ?? "") === nationality)
        .map((p) => p.id);
      if (ids.length === 0) return;
      apply(await setPlayersEligible(ids, eligible));
    });
  }

  function handleToggleCategory(id: string, categoryId: string, member: boolean) {
    run(async () => {
      apply(await setPlayerCategory(id, categoryId, member));
    });
  }

  function handleAddPointAdjustment(id: string, delta: number, reason: string) {
    run(async () => {
      apply(await addPointAdjustment(id, delta, reason));
    });
  }

  function handleRemovePointAdjustment(id: string, adjustmentId: string) {
    run(async () => {
      apply(await removePointAdjustment(id, adjustmentId));
    });
  }

  function handleUndo() {
    run(async () => {
      apply(await undoTournament());
    });
  }

  // Keyboard shortcuts. Skipped while typing in a field so we don't clobber the
  // browser's native caret movement (arrows) or per-field undo (Ctrl/Cmd+Z).
  function handleKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement | null;
    const typing =
      !!target &&
      (target.isContentEditable ||
        target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.tagName === "SELECT");

    // Ctrl/Cmd+Z → undo the last tournament change.
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && e.key.toLowerCase() === "z") {
      if (typing) return;
      e.preventDefault();
      if (canUndo && !busy) handleUndo();
      return;
    }

    // Left/Right arrows → step between the visible tabs.
    if (!typing && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
      const i = tabOrder.indexOf(activeTab);
      if (i === -1) return;
      const next = e.key === "ArrowLeft" ? i - 1 : i + 1;
      if (next < 0 || next >= tabOrder.length) return;
      e.preventDefault();
      activeTab = tabOrder[next];
    }
  }

  function handlePrepareRound() {
    // Starting round 1 also finalizes registration (folded into one undo step
    // server-side); pass the chosen cup size along so that finalize can seed the
    // bracket. From round 2 on this is ignored (already finalized).
    const size =
      phase === "registration" && cupEnabled ? (cupSizeChoice ?? undefined) : undefined;
    run(async () => {
      apply(await prepareRound(size));
      activeTab = "draft";
    });
  }

  function handleUpdateDraft(update: DraftUpdate) {
    run(async () => apply(await updateDraft(update)));
  }

  function handleConfirmRound() {
    run(async () => {
      apply(await confirmRound());
      // Jump to the round we just created.
      if (tournament) activeTab = `round-${tournament.rounds.length}`;
    });
  }

  function handleSetResult(roundNumber: number, boardIndex: number, clicked: Winner) {
    run(async () => {
      apply(await setBoardWinner(roundNumber, boardIndex, clicked));
    });
  }

  function handleSetDrawn(roundNumber: number, boardIndex: number, drawn: boolean) {
    run(async () => {
      apply(await setBoardDrawn(roundNumber, boardIndex, drawn));
    });
  }

  function handleSetHandicap(
    roundNumber: number,
    boardIndex: number,
    handicap: Handicap | null,
  ) {
    run(async () => {
      apply(await setBoardHandicap(roundNumber, boardIndex, handicap));
    });
  }

  function handleSetSitoutValue(roundNumber: number, player: number, value: SitoutValue) {
    run(async () => {
      apply(await setSitoutValue(roundNumber, player, value));
    });
  }

  function handleSetTeamSitoutValue(roundNumber: number, teamId: string, value: SitoutValue) {
    run(async () => {
      apply(await setTeamSitoutValue(roundNumber, teamId, value));
    });
  }

  function handleSetNoShow(roundNumber: number, boardIndex: number, absent: Forfeit | null) {
    run(async () => {
      apply(await setBoardNoShow(roundNumber, boardIndex, absent));
    });
  }

  function handleSetLong(roundNumber: number, boardIndex: number, long: boolean) {
    run(async () => {
      apply(await setBoardLong(roundNumber, boardIndex, long));
    });
  }

  function handleSave() {
    if (!tournament) return;
    const current = tournament;
    run(async () => {
      const saved = await saveTournament(current);
      if (saved) store.markSaved();
    });
  }

  // Automatic server-side backups, taken at round state-machine transitions —
  // shown in a small toggled panel. `null` until the listing has been fetched
  // (see `loadInitial`), which is distinct from a listing whose `directory` is
  // null: that one means the server is keeping no backups at all.
  let showBackups = $state(false);
  let backupListing = $state<BackupList | null>(null);
  const backups = $derived(backupListing?.backups ?? []);

  // The button's tooltip names the directory the backups actually land in.
  // Where they are kept is the first thing a referee needs when the answer to
  // "can I get yesterday's state back?" is "yes, they're on disk" — and the
  // one thing the panel of timestamps can't tell them.
  const backupsTitle = $derived.by(() => {
    const base = $_("app.backupsTitle");
    if (backupListing === null) return base; // not fetched yet
    return backupListing.directory
      ? `${base}\n${$_("app.backupsDirectory", { values: { path: backupListing.directory } })}`
      : `${base}\n${$_("app.backupsNowhere")}`;
  });

  function handleToggleBackups() {
    showBackups = !showBackups;
    if (!showBackups) return;
    run(async () => {
      backupListing = await fetchBackups();
    });
  }

  function handleRestoreBackup(id: string) {
    if (!confirmDiscard()) return;
    run(async () => {
      apply(await restoreBackup(id));
      store.markSaved(); // matches what the server had at that point
      showBackups = false;
    });
  }

  // The public read-only page and the two printed pages (see
  // `publication.svelte.ts`).
  const publication = new Publication({
    run,
    t: (key) => $_(key),
    confirm: (message) => window.confirm(message),
  });
  const prints = new PrintJobs(run);

  /**
   * Whether the standings table may ask for landscape paper (it owns the rule —
   * see `landscapePaper` in ResultsView, which the reader page shares).
   *
   * The QR sheet and the result sheets are their own documents, printed from
   * whichever tab happens to be open, and both are portrait. An unnamed `@page`
   * is document-wide, so it has to stand down for them — and it gets the chance
   * to, because `PrintJobs` sets these flags and then `await tick()` before
   * calling `window.print()`, which flushes the effect that removes it.
   */
  const landscapePaper = $derived(!prints.qr && prints.sheets === null);

  /**
   * The print job in progress, as a class on the page shell.
   *
   * Both printed documents are states of the *whole page* — each hides almost
   * everything and rearranges what is left — so the flag has to sit on the
   * element every print rule below hangs off, which is the shell's. The two
   * never overlap: each is set, printed and cleared inside one `run`.
   */
  const printJob = $derived(prints.qr ? "printing-qr" : prints.sheets ? "printing-sheets" : "");

  const publicUrl = $derived(
    publication.state?.key && $currentTournamentId
      ? publicPageUrl($currentTournamentId, publication.state.key)
      : null,
  );

  const sheetMacMahonRow = $derived(!!tournament && macMahonRowShown(tournament.settings));

  function handlePrintSheets(rounds: number, blanks: number) {
    prints.printSheets(() => {
      const t = tournament!;
      return {
        players: buildSheetPlayers(t.players, t.settings, standings),
        rounds,
        blanks,
      };
    });
  }

  function handleRefreshRatings() {
    run(async () => {
      ratings = await refreshRatings();
    });
  }

  function handleUpdateSettings(settings: TournamentSettings) {
    run(async () => {
      apply(await updateSettings(settings));
    });
  }

  function handleExportGrid() {
    if (!tournament) return;
    const name = tournament.name;
    run(async () => {
      const grid = await fetchAmericanGrid();
      await saveAmericanGrid(name, grid);
    });
  }

  function handleCancelRound() {
    run(async () => apply(await cancelRound()));
    // If the cancelled round's tab was active, the tab-validity effect resets it.
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<PageShell title="OpenShogiPairings" modifiers={printJob}>
  {#snippet controls()}
    {#if $currentTournamentId !== null}
      <ConnectionStatus />
    {/if}
    <ThemeSwitcher />
    <LocaleSwitcher />
  {/snippet}

  {#if error && !$authRequired}
    <p class="error-banner" role="alert">{error}</p>
  {/if}

  <!-- The server applied the edit but could not write it to disk. It answered
       200, and so will every edit after it, so this banner is the only thing
       that will ever say the tournament is one restart away from being lost. -->
  {#if !persisted && !$authRequired}
    <p class="error-banner" role="alert">{$_("app.notPersisted")}</p>
  {/if}

  {#if $currentTournamentId === null}
    <TournamentPicker />
  {:else if $authRequired}
    <Login onSuccess={loadInitial} />
  {:else if initialLoad === "loading"}
    <p class="muted">{$_("app.loading")}</p>
  {:else if tournament}
    <ContentCard wide={tabHasWideTable}>
      <div class="toolbar fit-width">
        <div class="title">
          <h2>{tournament.name}</h2>
          {#if hasUnsavedChanges}
            <span class="unsaved-dot" title={$_("app.unsavedChanges")}>●</span>
          {/if}
        </div>
        <div class="toolbar-actions">
          <button
            type="button"
            class="ghost control-lg"
            data-testid="undo"
            onclick={handleUndo}
            disabled={busy || !canUndo}
            title={$_("app.undoTitle")}
          >
            {$_("app.undo")}
          </button>
          <button
            type="button"
            class="ghost control-lg"
            data-testid="save"
            onclick={handleSave}
            disabled={busy}
          >
            {$_("app.save")}
          </button>
          <button
            type="button"
            class="ghost control-lg"
            class:active={showBackups}
            data-testid="toggle-backups"
            onclick={handleToggleBackups}
            disabled={busy}
            title={backupsTitle}
          >
            {$_("app.backups")}
          </button>
          <button
            type="button"
            class="ghost control-lg"
            class:active={publication.open}
            data-testid="toggle-publication"
            onclick={() => publication.toggle()}
            disabled={busy}
            title={$_("app.publicPageTitle")}
          >
            {$_("app.publicPage")}
          </button>
          <button
            type="button"
            class="ghost control-lg"
            data-testid="switch-tournament"
            onclick={() => currentTournamentId.set(null)}
            disabled={busy}
            title={$_("app.switchTournamentTitle")}
          >
            {$_("app.switchTournament")}
          </button>
        </div>
      </div>

      {#if publication.open}
        <div class="publication-panel fit-width">
          {#if !publication.canPublish}
            <!-- The desktop app: no live link is possible from a server on a
                 random loopback port, so say so once rather than leave the
                 referee looking for a button that cannot exist here. -->
            <p class="small">{$_("app.publicPageDesktop")}</p>
          {:else if publication.state === null}
            <p class="small">{$_("app.loading")}</p>
          {:else if publication.state.published && publicUrl}
            <p class="small">{$_("app.publicPageLive")}</p>
            <!-- Only shown when printing the code for the wall: on screen the
                 tournament's name is right above, in the toolbar. -->
            <h3 class="qr-print-title">{tournament.name}</h3>
            <div class="public-share">
              <div class="qr-holder">
                <QrCode
                  text={publicUrl}
                  label={$_("app.publicQrLabel", { values: { name: tournament.name } })}
                />
              </div>
              <p class="public-url" data-testid="public-url">{publicUrl}</p>
            </div>
            <div class="publication-actions">
              <button
                type="button"
                class="ghost control-xs"
                data-testid="copy-public-link"
                onclick={() => publication.copyLink(publicUrl)}
                disabled={busy}
              >
                {publication.copiedLink ? $_("app.publicLinkCopied") : $_("app.copyPublicLink")}
              </button>
              <button
                type="button"
                class="ghost control-xs"
                data-testid="print-public-qr"
                onclick={() => prints.printQr()}
                disabled={busy}
                title={$_("app.printPublicQrTitle")}
              >
                {$_("app.printPublicQr")}
              </button>
              <button
                type="button"
                class="ghost control-xs"
                data-testid="rotate-public-key"
                onclick={() => publication.setPublished(true)}
                disabled={busy}
                title={$_("app.rotatePublicKeyTitle")}
              >
                {$_("app.rotatePublicKey")}
              </button>
              <button
                type="button"
                class="ghost control-xs danger"
                data-testid="unpublish"
                onclick={() => publication.setPublished(false)}
                disabled={busy}
              >
                {$_("app.unpublish")}
              </button>
            </div>
          {:else}
            <p class="small">{$_("app.publicPageOff")}</p>
            <div class="publication-actions">
              <button
                type="button"
                class="ghost control-xs"
                data-testid="publish"
                onclick={() => publication.setPublished(true)}
                disabled={busy}
              >
                {$_("app.publish")}
              </button>
            </div>
          {/if}

          <!-- The other transport (docs/archive/public-access.md phase 2): a file, not
               a link. Offered whatever the deployment, and not gated on the
               publication flag above — that flag governs *this server's* reader
               endpoint, while saving the file is itself the act of publishing.
               Tying them together would make the desktop referee mint a
               capability key pointing at a loopback port to get a file. -->
          <div class="publication-export">
            <p class="small">{$_("app.publicExportHint")}</p>
            <div class="publication-actions">
              <button
                type="button"
                class="ghost control-xs"
                data-testid="export-public-page"
                onclick={() => publication.exportPages()}
                disabled={busy}
                title={$_("app.publicExportTitle")}
              >
                {publication.exportedPages > 0
                  ? $_("app.publicExportDone", { values: { count: publication.exportedPages } })
                  : $_("app.publicExport")}
              </button>
            </div>
          </div>

          <!-- The adjustment reasons are referee-to-referee prose today, and
               this panel is where they stop being that — by either transport.
               Said here rather than next to the field, so it is read at the
               moment it starts to matter. -->
          <p class="small warn">{$_("app.publicPageReasonsWarning")}</p>
        </div>
      {/if}

      {#if showBackups}
        <div class="backups-panel fit-width print-hide">
          <!-- The path, in the panel as well as the button's tooltip: with an
               empty list it is the only thing that answers "where would they
               be?", and it is selectable here, so it can be pasted into a file
               manager rather than retyped from a tooltip. -->
          {#if backupListing !== null}
            <p class="small backups-dir">
              {#if backupListing.directory}
                {$_("app.backupsDirectory", { values: { path: backupListing.directory } })}
              {:else}
                {$_("app.backupsNowhere")}
              {/if}
            </p>
          {/if}
          {#if backups.length === 0}
            <p class="small">
              {$_("app.noBackupsYet")}
            </p>
          {:else}
            <ul class="backups-list">
              {#each backups as b (b.id)}
                <li>
                  <span class="backup-time"
                    >{new Date(b.taken_at * 1000).toLocaleString($locale ?? undefined)}</span
                  >
                  <span class="backup-label">{b.label}</span>
                  <button
                    type="button"
                    class="ghost control-xs"
                    disabled={busy}
                    onclick={() => handleRestoreBackup(b.id)}
                  >
                    {$_("app.restore")}
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {/if}

      <TabStrip {tabs} active={activeTab} onSelect={(id) => (activeTab = id)}>
        {#snippet trailing()}
          <div class="round-controls">
            {#if phase === "registration" && cupEnabled}
              <label class="cup-size" title={$_("app.cupTitle")}>
                {$_("app.cupLabel")}
                {#if validCupSizes.length > 0}
                  <select class="control-lg" bind:value={cupSizeChoice} disabled={busy}>
                    {#each validCupSizes as s (s)}
                      <option value={s}>
                        {$_(
                          cupFormat === "qualifier"
                            ? "app.cupSizeOptionQualifier"
                            : "app.cupSizeOption",
                          { values: { size: s, field: cupFieldSize(s) } },
                        )}
                      </option>
                    {/each}
                  </select>
                {:else}
                  <span class="cup-warn">
                    {$_("app.cupNeedMoreEligible", { values: { needed: cupFieldSize(8) } })}
                  </span>
                {/if}
              </label>
            {/if}
            <button
              type="button"
              class="ctrl control-lg primary"
              data-testid="prepare-round"
              onclick={handlePrepareRound}
              disabled={!startEnabled}
              title={startTitle}
            >
              {$_("app.prepareRound", { values: { number: nextRoundNumber } })}
            </button>
            <button
              type="button"
              class="ctrl control-lg"
              data-testid="export-grid"
              onclick={handleExportGrid}
              disabled={!exportEnabled}
              title={exportTitle}
            >
              {$_("app.exportGrid")}
            </button>
            <button
              type="button"
              class="ctrl control-lg danger"
              data-testid="cancel-round"
              onclick={handleCancelRound}
              disabled={!canCancel}
              title={cancelTitle}
            >
              {$_("app.cancelLastRound")}
            </button>
          </div>
        {/snippet}
      </TabStrip>

      <div class="tab-content">
        {#if activeTab === "settings"}
          <TournamentSettingsView
            settings={tournament.settings}
            tournamentName={tournament.name}
            finalized={tournament.registration_finalized}
            players={tournament.players}
            onUpdate={handleUpdateSettings}
            {busy}
          />
        {:else if activeTab === "players"}
          <div class="print-hide">
            <PlayerRegistration onAdd={handleAddPlayer} {ratings} {busy} />
          </div>
          <div class="ratings-status print-hide">
            <!-- Left: the FESA list and the button that refreshes it — a state
                 and the action on that state. Right: the two roster-file
                 buttons, which have nothing to do with either. -->
            <div class="ratings-group">
              <span>
                {#if ratings.length > 0}
                  {$_("app.ratingsLoaded", { values: { count: ratings.length } })}
                {:else}
                  {$_("app.ratingsNotLoaded")}
                {/if}
              </span>
              <button
                type="button"
                class="ghost control-xs"
                onclick={handleRefreshRatings}
                disabled={busy}
                title={$_("app.refreshRatingsTitle")}
              >
                {$_("app.refreshRatings")}
              </button>
            </div>
            <div class="ratings-group">
              <button
                type="button"
                class="ghost control-xs"
                class:active={showLicenceCheck}
                data-testid="check-licences"
                onclick={() => (showLicenceCheck = !showLicenceCheck)}
                title={$_("playerRegistration.checkLicencesTitle")}
              >
                {$_("playerRegistration.checkLicences")}
              </button>
              <button
                type="button"
                class="ghost control-xs"
                onclick={handleImportCsv}
                disabled={busy}
                title={$_("playerRegistration.importCsvTitle")}
              >
                {$_("playerRegistration.importCsv")}
              </button>
            </div>
          </div>
          {#if csvSkipped.length > 0}
            <p class="csv-skipped print-hide" role="status" data-testid="csv-skipped">
              {$_("playerRegistration.csvSkippedDuplicates", {
                values: { count: csvSkipped.length, names: csvSkipped.join(", ") },
              })}
            </p>
          {/if}
          {#if showLicenceCheck}
            <div class="licence-check print-hide">
              <LicenceCheckPanel
                players={tournament.players}
                result={licenceCheck}
                onCheck={handleCheckLicences}
                {busy}
              />
            </div>
          {/if}
          <div class="players">
            <PlayerList
              players={tournament.players}
              showEligible={cupEnabled}
              {cupFormat}
              categories={tournament.settings.categories ?? []}
              finalized={tournament.registration_finalized}
              onEdit={handleEditPlayer}
              onRemove={teamMode && tournament.registration_finalized
                ? undefined
                : handleRemovePlayer}
              onToggleEligible={handleToggleEligible}
              onSetEligibleByNationality={handleSetEligibleByNationality}
              onToggleCategory={handleToggleCategory}
              onAddAdjustment={teamMode ? undefined : handleAddPointAdjustment}
              onRemoveAdjustment={teamMode ? undefined : handleRemovePointAdjustment}
              published={publication.state?.published ?? false}
              {busy}
            />
          </div>
        {:else if activeTab === "teams"}
          <TeamsPanel
            teams={tournament.teams ?? []}
            players={tournament.players}
            size={teamSize}
            finalized={tournament.registration_finalized}
            {macmahonInUse}
            onAdd={teams.add}
            onRename={teams.rename}
            onRemove={teams.remove}
            onAddMember={teams.addMember}
            onRemoveMember={teams.removeMember}
            onSetBoardOrder={teams.setBoardOrder}
            onSortByRating={teams.sortByRating}
            onSetPairingRating={teams.setPairingRating}
            onAddAdjustment={teams.addAdjustment}
            onRemoveAdjustment={teams.removeAdjustment}
            {busy}
          />
        {:else if activeTab === "results"}
          <ResultsView
            {tournament}
            {standings}
            teamStandings={teamMode ? teamStandings : []}
            {cupPodium}
            {effectiveWinners}
            {teamMatches}
            categories={tournament.settings.categories ?? []}
            {landscapePaper}
            onSetSitoutValue={handleSetSitoutValue}
            onSetTeamSitoutValue={handleSetTeamSitoutValue}
          />
        {:else if activeTab === "cup" && tournament.cup && cupBracket}
          <CupBracket bracket={cupBracket} cup={tournament.cup} players={tournament.players} />
        {:else if activeTab === "draft" && tournament.draft}
          <RoundDraftView
            draft={tournament.draft}
            players={tournament.players}
            cupPlayers={draftCupPlayers}
            longGamePlayers={draftLongPlayers}
            teams={teamMode ? (tournament.teams ?? []) : []}
            onUpdate={handleUpdateDraft}
            onConfirm={handleConfirmRound}
            onPrintSheets={handlePrintSheets}
            {sheetMacMahonRow}
            {busy}
          />
        {:else if activeRound}
          <RoundView
            round={activeRound}
            players={tournament.players}
            handicapPolicy={handicapChoice(tournament.settings.handicap_policy)}
            suggestedHandicaps={activeRoundSuggested}
            explanation={activeRound.explanation}
            {explanationStale}
            onProbe={(a, b, mode) => fetchCounterfactual(activeRound.number, a, b, mode)}
            canForce={canForceActiveRound}
            onForcePairing={(a, b) => run(async () => apply(await forcePairing(a, b)))}
            onClickWinner={(boardIndex, clicked) =>
              handleSetResult(activeRound.number, boardIndex, clicked)}
            onToggleDrawn={(boardIndex, drawn) =>
              handleSetDrawn(activeRound.number, boardIndex, drawn)}
            onSetNoShow={(boardIndex, absent) =>
              handleSetNoShow(activeRound.number, boardIndex, absent)}
            onSetHandicap={(boardIndex, handicap) =>
              handleSetHandicap(activeRound.number, boardIndex, handicap)}
            longEnabled={tournament.settings.long_boards_enabled}
            cup={tournament.cup}
            isCurrentRound={!!currentRound && activeRound.number === currentRound.number}
            onSetLong={(boardIndex, long) =>
              handleSetLong(activeRound.number, boardIndex, long)}
            teams={teamMode ? (tournament.teams ?? []) : []}
            matches={activeRoundMatches}
            onPrintSheets={handlePrintSheets}
            {sheetMacMahonRow}
            {busy}
          />
        {/if}
      </div>
    </ContentCard>
  {/if}

  {#if prints.sheets && tournament}
    <!-- A direct child of the shell, so the print stylesheet below can hide its
         siblings and leave the slips alone whatever tab is open. -->
    <ResultSheets
      tournamentName={tournament.name}
      players={prints.sheets.players}
      rounds={prints.sheets.rounds}
      blanks={prints.sheets.blanks}
      macMahonRow={sheetMacMahonRow}
    />
  {/if}

  {#snippet footer()}
    <ServerStatus />
  {/snippet}
</PageShell>

<style>
  .toolbar {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    margin-bottom: 1rem;
  }
  .title {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
  }
  .title h2 {
    margin: 0;
    font-size: 1.25rem;
  }
  .unsaved-dot {
    color: var(--color-warning);
    font-size: 0.6rem;
  }
  .toolbar-actions {
    display: flex;
    gap: 0.5rem;
  }
  /* An accent border on the ghost button's transparent background — the same
     "this panel is open" mark as the ratings and result-sheets buttons.
     `--text-on-accent` is for text *on* an accent fill; over a transparent
     button it is white on white in the light theme. */
  .toolbar-actions .ghost.active {
    border-color: var(--border-accent);
  }

  .backups-panel {
    margin-bottom: 1.25rem;
    padding: 0.6rem 0.9rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-inset);
  }
  .backups-panel .small {
    font-size: 0.8rem;
    color: var(--text-secondary);
    margin: 0;
  }
  /* The directory line: a path, so monospaced and selectable, and allowed to
     wrap — an absolute path is easily wider than the panel. */
  .backups-dir {
    font-family: var(--font-mono, ui-monospace, monospace);
    overflow-wrap: anywhere;
    user-select: text;
    margin-bottom: 0.4rem;
  }
  .backups-list {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 0.85rem;
  }
  .backups-list li {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.3rem 0;
    border-bottom: 1px solid var(--border-divider);
  }
  .backups-list li:last-child {
    border-bottom: none;
  }
  .backup-time {
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .backup-label {
    flex: 1;
  }

  .publication-panel {
    margin-bottom: 1.25rem;
    padding: 0.6rem 0.9rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-inset);
  }
  .publication-panel .small {
    font-size: 0.8rem;
    color: var(--text-secondary);
    margin: 0 0 0.4rem;
  }
  .publication-panel .small.warn {
    color: var(--color-warning);
    margin: 0.5rem 0 0;
  }
  .public-share {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 0.6rem;
  }
  .qr-holder {
    flex: none;
    width: 10rem;
  }
  /* The link itself: selectable and monospaced, because it gets pasted into a
     message or a printed sheet rather than read. The QR beside it is what
     anyone in the room actually uses. */
  .public-url {
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 0.8rem;
    overflow-wrap: anywhere;
    user-select: text;
    margin: 0;
  }
  /* Screen shows the tournament name in the toolbar; the printed sheet leaves
     the toolbar behind, so it carries its own heading. */
  .qr-print-title {
    display: none;
  }
  /* The second transport, ruled off from the live link above it: they solve
     the same problem by different means, and only one of them exists on the
     desktop app. */
  .publication-export {
    margin-top: 0.8rem;
    padding-top: 0.6rem;
    border-top: 1px solid var(--border-divider);
  }
  .publication-actions {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .publication-actions button.danger {
    color: var(--text-on-danger);
  }

  /* On the strip's line, at its right end. The strip itself — and how a tab
     looks — is `TabStrip`'s. */
  .round-controls {
    margin-left: auto;
    display: flex;
    gap: 0.5rem;
    align-items: center;
    padding-bottom: 0.3rem;
  }
  .cup-size {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.85rem;
    color: var(--text-secondary);
  }
  /* Raised like the lifecycle buttons it sits with, not sunken like a field. */
  .cup-size select {
    background: var(--bg-raised);
  }
  .cup-warn {
    color: var(--color-warning);
  }
  .ctrl:hover:not(:disabled) {
    border-color: var(--border-hover);
  }
  .ctrl:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .ctrl.primary:not(:disabled) {
    border-color: var(--border-accent-strong);
    background: var(--bg-accent);
    color: var(--text-on-accent);
  }
  .ctrl.danger:not(:disabled) {
    border-color: var(--border-danger);
    color: var(--text-on-danger);
  }
  .ctrl.danger:hover:not(:disabled) {
    border-color: var(--border-danger-strong);
    background: var(--bg-danger);
  }
  .ratings-status {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    margin-top: 0.6rem;
    font-size: 0.8rem;
    color: var(--text-secondary);
  }
  .ratings-group {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }
  .ratings-status .ghost.active {
    border-color: var(--border-accent);
  }
  .licence-check {
    margin-top: 0.6rem;
  }
  /* The import did what was asked — it just left some rows out — so this reads
     as a notice about the file, not as an error. */
  .csv-skipped {
    margin: 0.6rem 0 0;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border-warning);
    border-radius: 4px;
    background: var(--bg-warning);
    color: var(--color-warning-strong);
    font-size: 0.85rem;
    /* This lives inside `.tab-content` — the one child the `width: 0` rule above
       exempts — so on the Players tab, where the card is sized with
       `max-content`, its contribution to that measurement is the whole sentence
       on one line. It names every skipped player, so a fifteen-name import
       measured 2409px and dragged the card, and the page behind it, out to
       2489px against a 1265px window. A `max-width` clamps the intrinsic
       contribution and not just the used width, so the sentence wraps and stops
       setting the width of anything. 70ch is the measure the same kind of prose
       already uses in RoundView (`.report-stale` and its neighbours), for the
       same reason given there: a line the width of a standings table is not a
       line anybody reads. */
    max-width: 70ch;
  }
  .players {
    margin-top: 1.25rem;
  }
  /* The box is app.css's. This one addition is not: preserve newlines so a
     multi-line error (e.g. a list of bad rows) renders as separate lines
     instead of running together. */
  .error-banner {
    white-space: pre-line;
  }
  .muted {
    text-align: center;
  }

  /* The header, the footer and the column's own width are `PageShell`'s, and so
     is their print reset; the card's is `ContentCard`'s, the tab strip's is
     `TabStrip`'s (which takes the round controls inside it with it), and
     `.fit-width`'s and `.print-hide`'s are app.css's. What is left here is this
     app's own furniture.

     The `.app` in the selectors below is the shell's element rather than this
     component's, so it takes a `:global()` to name — Svelte scopes a selector
     to the markup it is written in, and would otherwise quietly match nothing.
     The trailing halves stay scoped, so these still only reach this app's
     markup. */
  @media print {
    .toolbar {
      display: none;
    }

    /* The publication panel is chrome for the referee at the screen, with one
       exception: the QR code for the wall is printed out of it, so that job —
       and only that job — keeps it. (`.print-hide` would be simpler and is what
       the backups panel next to it uses, but it hides for every print job with
       no way back.) */
    :global(.app:not(.printing-qr)) .publication-panel {
      display: none;
    }

    /* Printing the QR code for the wall is a different document from printing
       the pairings: one sheet, the tournament's name, the code as large as the
       page allows, and the link underneath for anyone who would rather type it.
       Everything else — including the panel's own explanation and buttons — is
       for the referee at the screen, not for the wall. */
    :global(.app.printing-qr) .tab-content,
    :global(.app.printing-qr) .publication-panel .small,
    :global(.app.printing-qr) .publication-actions {
      display: none;
    }
    :global(.app.printing-qr) .publication-panel {
      border: none;
      background: transparent;
      padding: 0;
      text-align: center;
    }
    :global(.app.printing-qr) .qr-print-title {
      display: block;
      font-size: 1.6rem;
      margin: 0 0 1rem;
    }
    :global(.app.printing-qr) .public-share {
      display: block;
    }
    :global(.app.printing-qr) .qr-holder {
      width: min(15cm, 80vw);
      margin: 0 auto 0.8rem;
    }
    :global(.app.printing-qr) .public-url {
      font-size: 0.9rem;
    }

    /* Printing the result sheets is a document of its own too: pages of slips to
       cut apart, and nothing of the screen around them. The slips sit beside the
       card rather than inside it, so hiding the card — rather than naming the
       parts of whichever tab happens to be open — is what leaves them alone.
       (The shell's other children are already gone: it hides the header and the
       footer for every print job, app.css hides the error banner, and its column
       is already back to the page width.) */
    :global(.app.printing-sheets > .card) {
      display: none;
    }
  }
</style>
