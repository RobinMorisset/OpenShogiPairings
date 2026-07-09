<script lang="ts">
  import { _ } from "svelte-i18n";
  import {
    addPlayer,
    addPointAdjustment,
    ApiError,
    cancelRound,
    completeRound,
    confirmRound,
    editPlayer,
    fetchAmericanGrid,
    fetchBackups,
    fetchCounterfactual,
    forcePairing,
    fetchRatings,
    fetchRoundExplanation,
    fetchTournament,
    finalizeRegistration,
    prepareRound,
    refreshRatings,
    removePlayer,
    removePointAdjustment,
    restoreBackup,
    setBoardDrawn,
    setBoardHandicap,
    setBoardWinner,
    setPlayerEligible,
    undoTournament,
    updateDraft,
    updateSettings,
    subscribeToChanges,
    type DraftUpdate,
  } from "./lib/api";
  import type {
    BackupInfo,
    CupPodium,
    Handicap,
    NewPlayer,
    RatedPlayer,
    RoundExplanation,
    Standing,
    Tournament,
    TournamentResponse,
    TournamentSettings,
    Winner,
  } from "./lib/types";
  import { saveAmericanGrid, saveTournament } from "./lib/tournamentFile";
  import ServerStatus from "./lib/components/ServerStatus.svelte";
  import Login from "./lib/components/Login.svelte";
  import TournamentPicker from "./lib/components/TournamentPicker.svelte";
  import { authRequired, currentTournamentId } from "./lib/session";
  import PlayerRegistration from "./lib/components/PlayerRegistration.svelte";
  import PlayerList from "./lib/components/PlayerList.svelte";
  import RoundView from "./lib/components/RoundView.svelte";
  import RoundDraftView from "./lib/components/RoundDraftView.svelte";
  import ResultsView from "./lib/components/ResultsView.svelte";
  import TournamentSettingsView from "./lib/components/TournamentSettingsView.svelte";
  import LocaleSwitcher from "./lib/components/LocaleSwitcher.svelte";
  import ThemeSwitcher from "./lib/components/ThemeSwitcher.svelte";
  import ConnectionStatus from "./lib/components/ConnectionStatus.svelte";

  let tournament = $state<Tournament | null>(null);
  let standings = $state<Standing[]>([]);
  let cupPodium = $state<CupPodium | null>(null);
  let draftCupPlayers = $state<string[]>([]);
  let suggestedHandicaps = $state<(Handicap | null)[][]>([]);
  /** Winner that counts for standings/pairing per board, server-computed (see
   *  `TournamentResponse.effective_winners`), indexed like `tournament.rounds`. */
  let effectiveWinners = $state<(Winner | null)[][]>([]);
  let canUndo = $state(false);
  let initialLoad = $state<"loading" | "done">("loading");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let ratings = $state<RatedPlayer[]>([]);

  // Whether the in-memory tournament has changes not yet written to a file —
  // purely client-side bookkeeping (the server has no notion of "saved to
  // disk"), and deliberately not part of the undo stack: undoing a change
  // doesn't retroactively make the tournament "saved" again. Reset to false
  // only by a fresh create/load or a successful save; every other mutation
  // (including undo) sets it, via `apply`.
  let hasUnsavedChanges = $state(false);

  /** Apply a tournament API response to local state. */
  function apply(res: TournamentResponse) {
    tournament = res.tournament;
    standings = res.standings;
    cupPodium = res.cup_podium ?? null;
    draftCupPlayers = res.draft_cup_players ?? [];
    suggestedHandicaps = res.suggested_handicaps ?? [];
    effectiveWinners = res.effective_winners ?? [];
    canUndo = res.can_undo;
    hasUnsavedChanges = true;
  }

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

  const activeRound = $derived(
    tournament?.rounds.find((r) => `round-${r.number}` === activeTab) ?? null,
  );

  // The suggested-handicap slice for the active round, matched by position
  // (rounds are numbered sequentially without gaps).
  const activeRoundSuggested = $derived.by(() => {
    if (!tournament || !activeRound) return [];
    const idx = tournament.rounds.findIndex((r) => r.number === activeRound.number);
    return idx >= 0 ? (suggestedHandicaps[idx] ?? []) : [];
  });

  // Why the engine chose the active round's pairings — fetched lazily whenever a
  // round tab is open (and refetched when the tournament changes, since editing
  // an earlier result can shift a later round's ledger).
  let roundExplanation = $state<RoundExplanation | null>(null);
  $effect(() => {
    const round = activeRound;
    void tournament; // re-run on any tournament update
    if (!round) {
      roundExplanation = null;
      return;
    }
    let cancelled = false;
    roundExplanation = null;
    fetchRoundExplanation(round.number)
      .then((ex) => {
        if (!cancelled) roundExplanation = ex;
      })
      .catch(() => {
        if (!cancelled) roundExplanation = null;
      });
    return () => {
      cancelled = true;
    };
  });

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
  const enoughPlayers = $derived((tournament?.players.length ?? 0) >= 2);
  const currentRoundAllPlayed = $derived(
    currentRound ? currentRound.boards.every((b) => b.result != null) : false,
  );

  // The active round can be re-paired (via "force this pairing") only if it is
  // the current, in-progress round with no results recorded yet.
  const canForceActiveRound = $derived(
    !!activeRound &&
      !!currentRound &&
      activeRound.number === currentRound.number &&
      !activeRound.completed &&
      activeRound.boards.every((b) => b.result == null),
  );

  // Hybrid cup: which bracket sizes are choosable at finalization depends on how
  // many players the referee has marked eligible.
  const cupEnabled = $derived(tournament?.settings.cup_enabled ?? false);
  const eligibleCount = $derived(
    tournament?.players.filter((p) => p.eligible).length ?? 0,
  );
  const validCupSizes = $derived([8, 16, 32, 64].filter((s) => s <= eligibleCount));
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

  // "Advance" button (finalize registration / complete current round).
  const advanceLabel = $derived(
    phase === "registration"
      ? $_("app.advanceRegistration")
      : phase === "in_progress"
        ? $_("app.advanceInProgress", { values: { number: currentRound?.number } })
        : (tournament?.rounds.length ?? 0) > 0
          ? $_("app.advanceRoundComplete", { values: { number: tournament?.rounds.length } })
          : $_("app.advanceRegistrationFinalized"),
  );
  const advanceEnabled = $derived(
    !busy &&
      ((phase === "registration" && enoughPlayers && cupReady) ||
        (phase === "in_progress" && currentRoundAllPlayed)),
  );
  const advanceTitle = $derived(
    phase === "registration" && !enoughPlayers
      ? $_("app.advanceTitleNeedPlayers")
      : phase === "registration" && !cupReady
        ? $_("app.advanceTitleNeedCup")
        : phase === "in_progress" && !currentRoundAllPlayed
          ? $_("app.advanceTitleNeedResults")
          : "",
  );

  // "Start round" button.
  const nextRoundNumber = $derived((tournament?.rounds.length ?? 0) + 1);
  const startEnabled = $derived(!busy && phase === "ready" && enoughPlayers);
  const startTitle = $derived(
    phase !== "ready"
      ? $_("app.startTitleNotReady")
      : !enoughPlayers
        ? $_("app.startTitleNeedPlayers")
        : "",
  );

  // "Export grid" button: available only in the "ready" phase (no draft, no
  // round in progress) once at least one round has been completed.
  const completedRoundCount = $derived(
    tournament?.rounds.filter((r) => r.completed).length ?? 0,
  );
  const exportEnabled = $derived(
    !busy && phase === "ready" && completedRoundCount > 0,
  );
  const exportTitle = $derived(
    completedRoundCount === 0
      ? $_("app.exportTitleNoRounds")
      : phase !== "ready"
        ? $_("app.exportTitleNotReady")
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
      "results",
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
      const res = await fetchTournament();
      if (res) {
        apply(res);
        // Resuming whatever the server already had isn't a new edit made in
        // this session — nothing to warn about losing yet.
        hasUnsavedChanges = false;
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

  // Silently pull the authoritative state — after a live change pushed by
  // another referee, on reconnect, or after a rejected (conflicting) edit. Safe
  // because every edit already lives on the server; there is no unsaved local
  // state to lose.
  async function refetch() {
    try {
      const res = await fetchTournament();
      if (res) {
        // A resync/live update isn't a local edit, so don't let it flip the
        // "unsaved changes" flag — apply() sets that for genuine local edits.
        const wasUnsaved = hasUnsavedChanges;
        apply(res);
        hasUnsavedChanges = wasUnsaved;
      }
    } catch {
      /* transient; the SSE stream will trigger another refetch on the next change */
    }
  }

  // (Re)load whenever the open tournament changes — including the very first
  // selection — and keep the live-sync subscription scoped to it.
  $effect(() => {
    if ($currentTournamentId === null) return;
    // Reset the view before loading the newly selected tournament, so a
    // moment of stale UI from the previous one never shows.
    tournament = null;
    standings = [];
    cupPodium = null;
    draftCupPlayers = [];
    suggestedHandicaps = [];
    effectiveWinners = [];
    canUndo = false;
    hasUnsavedChanges = false;
    error = null;
    activeTab = "players";

    void loadInitial();
    // Live sync: refetch on another referee's change and on every (re)connect.
    const unsubscribe = subscribeToChanges(refetch);
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
    if (err instanceof ApiError && err.status === 0) {
      return $_("app.cannotReachServer");
    }
    return err instanceof Error ? err.message : String(err);
  }

  /** Run an async action with shared busy/error handling. */
  async function run(action: () => Promise<void>) {
    busy = true;
    error = null;
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

  function handleToggleEligible(id: string, eligible: boolean) {
    run(async () => {
      apply(await setPlayerEligible(id, eligible));
    });
  }

  // Bulk cup-eligibility toggle: every player of the given nationality, one
  // request at a time so each response's tournament state is applied before
  // the next request goes out.
  function handleSetEligibleByNationality(nationality: string, eligible: boolean) {
    run(async () => {
      const ids = (tournament?.players ?? [])
        .filter((p) => (p.nationality ?? "") === nationality)
        .map((p) => p.id);
      for (const id of ids) {
        apply(await setPlayerEligible(id, eligible));
      }
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

  function handleAdvance() {
    if (phase === "registration") {
      const size = cupEnabled ? (cupSizeChoice ?? undefined) : undefined;
      run(async () => apply(await finalizeRegistration(size)));
    } else if (phase === "in_progress") {
      run(async () => apply(await completeRound()));
    }
  }

  function handlePrepareRound() {
    run(async () => {
      apply(await prepareRound());
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

  function handleSave() {
    if (!tournament) return;
    const current = tournament;
    run(async () => {
      const saved = await saveTournament(current);
      if (saved) hasUnsavedChanges = false;
    });
  }

  // Automatic server-side backups, taken at round state-machine transitions —
  // shown in a small toggled panel, fetched lazily when opened.
  let showBackups = $state(false);
  let backups = $state<BackupInfo[]>([]);

  function handleToggleBackups() {
    showBackups = !showBackups;
    if (!showBackups) return;
    run(async () => {
      backups = await fetchBackups();
    });
  }

  function handleRestoreBackup(id: string) {
    if (!confirmDiscard()) return;
    run(async () => {
      apply(await restoreBackup(id));
      hasUnsavedChanges = false; // matches what the server had at that point
      showBackups = false;
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

<div class="app">
  <header>
    <div class="header-top">
      <div class="header-titles">
        <h1>OpenShogiPairings</h1>
        <p class="subtitle">{$_("app.subtitle")}</p>
      </div>
      <div class="header-controls">
        {#if $currentTournamentId !== null}
          <ConnectionStatus />
        {/if}
        <ThemeSwitcher />
        <LocaleSwitcher />
      </div>
    </div>
  </header>

  {#if error && !$authRequired}
    <p class="error-banner" role="alert">{error}</p>
  {/if}

  {#if $currentTournamentId === null}
    <TournamentPicker />
  {:else if $authRequired}
    <Login onSuccess={loadInitial} />
  {:else if initialLoad === "loading"}
    <p class="muted">{$_("app.loading")}</p>
  {:else if tournament}
    <section class="card">
      <div class="toolbar">
        <div class="title">
          <h2>{tournament.name}</h2>
          {#if hasUnsavedChanges}
            <span class="unsaved-dot" title={$_("app.unsavedChanges")}>●</span>
          {/if}
        </div>
        <div class="toolbar-actions">
          <button
            type="button"
            class="ghost"
            onclick={handleUndo}
            disabled={busy || !canUndo}
            title={$_("app.undoTitle")}
          >
            {$_("app.undo")}
          </button>
          <button type="button" class="ghost" onclick={handleSave} disabled={busy}>
            {$_("app.save")}
          </button>
          <button
            type="button"
            class="ghost"
            class:active={showBackups}
            onclick={handleToggleBackups}
            disabled={busy}
            title={$_("app.backupsTitle")}
          >
            {$_("app.backups")}
          </button>
          <button
            type="button"
            class="ghost"
            onclick={() => currentTournamentId.set(null)}
            disabled={busy}
            title={$_("app.switchTournamentTitle")}
          >
            {$_("app.switchTournament")}
          </button>
        </div>
      </div>

      {#if showBackups}
        <div class="backups-panel">
          {#if backups.length === 0}
            <p class="small">
              {$_("app.noBackupsYet")}
            </p>
          {:else}
            <ul class="backups-list">
              {#each backups as b (b.id)}
                <li>
                  <span class="backup-time">{new Date(b.taken_at * 1000).toLocaleString()}</span>
                  <span class="backup-label">{b.label}</span>
                  <button
                    type="button"
                    class="ghost small"
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

      <div class="tabs" role="tablist">
        <button
          type="button"
          class="tab"
          class:active={activeTab === "settings"}
          onclick={() => (activeTab = "settings")}
        >
          {$_("app.tabSettings")}
        </button>
        <button
          type="button"
          class="tab"
          class:active={activeTab === "players"}
          onclick={() => (activeTab = "players")}
        >
          {$_("app.tabPlayers", { values: { count: tournament.players.length } })}
        </button>
        <button
          type="button"
          class="tab"
          class:active={activeTab === "results"}
          onclick={() => (activeTab = "results")}
        >
          {$_("app.tabResults")}
        </button>
        {#each tournament.rounds as round (round.number)}
          <button
            type="button"
            class="tab"
            class:active={activeTab === `round-${round.number}`}
            onclick={() => (activeTab = `round-${round.number}`)}
          >
            {round.completed
              ? $_("app.tabRoundCompleted", { values: { number: round.number } })
              : $_("app.tabRound", { values: { number: round.number } })}
          </button>
        {/each}
        {#if tournament.draft}
          <button
            type="button"
            class="tab draft-tab"
            class:active={activeTab === "draft"}
            onclick={() => (activeTab = "draft")}
          >
            {$_("app.tabRoundDraft", { values: { number: tournament.draft.number } })}
          </button>
        {/if}
        <div class="round-controls">
          {#if phase === "registration" && cupEnabled}
            <label class="cup-size" title={$_("app.cupTitle")}>
              {$_("app.cupLabel")}
              {#if validCupSizes.length > 0}
                <select bind:value={cupSizeChoice} disabled={busy}>
                  {#each validCupSizes as s (s)}
                    <option value={s}>{$_("app.cupSizeOption", { values: { size: s } })}</option>
                  {/each}
                </select>
              {:else}
                <span class="cup-warn">{$_("app.cupNeedMoreEligible")}</span>
              {/if}
            </label>
          {/if}
          <button
            type="button"
            class="ctrl"
            onclick={handleAdvance}
            disabled={!advanceEnabled}
            title={advanceTitle}
          >
            {advanceLabel}
          </button>
          <button
            type="button"
            class="ctrl primary"
            onclick={handlePrepareRound}
            disabled={!startEnabled}
            title={startTitle}
          >
            {$_("app.prepareRound", { values: { number: nextRoundNumber } })}
          </button>
          <button
            type="button"
            class="ctrl"
            onclick={handleExportGrid}
            disabled={!exportEnabled}
            title={exportTitle}
          >
            {$_("app.exportGrid")}
          </button>
          <button
            type="button"
            class="ctrl danger"
            onclick={handleCancelRound}
            disabled={!canCancel}
            title={cancelTitle}
          >
            {$_("app.cancelLastRound")}
          </button>
        </div>
      </div>

      <div class="tab-content">
        {#if activeTab === "settings"}
          <TournamentSettingsView
            settings={tournament.settings}
            finalized={tournament.registration_finalized}
            players={tournament.players}
            onUpdate={handleUpdateSettings}
            {busy}
          />
        {:else if activeTab === "players"}
          <PlayerRegistration onAdd={handleAddPlayer} {ratings} {busy} />
          <div class="ratings-status">
            <span>
              {#if ratings.length > 0}
                {$_("app.ratingsLoaded", { values: { count: ratings.length } })}
              {:else}
                {$_("app.ratingsNotLoaded")}
              {/if}
            </span>
            <button
              type="button"
              class="ghost small"
              onclick={handleRefreshRatings}
              disabled={busy}
              title={$_("app.refreshRatingsTitle")}
            >
              {$_("app.refreshRatings")}
            </button>
          </div>
          <div class="players">
            <PlayerList
              players={tournament.players}
              showEligible={cupEnabled}
              finalized={tournament.registration_finalized}
              onEdit={handleEditPlayer}
              onRemove={handleRemovePlayer}
              onToggleEligible={handleToggleEligible}
              onSetEligibleByNationality={handleSetEligibleByNationality}
              onAddAdjustment={handleAddPointAdjustment}
              onRemoveAdjustment={handleRemovePointAdjustment}
              {busy}
            />
          </div>
        {:else if activeTab === "results"}
          <ResultsView {tournament} {standings} {cupPodium} {effectiveWinners} />
        {:else if activeTab === "draft" && tournament.draft}
          <RoundDraftView
            draft={tournament.draft}
            players={tournament.players}
            cupPlayers={draftCupPlayers}
            onUpdate={handleUpdateDraft}
            onConfirm={handleConfirmRound}
            {busy}
          />
        {:else if activeRound}
          <RoundView
            round={activeRound}
            players={tournament.players}
            handicapPolicy={tournament.settings.handicap_policy}
            suggestedHandicaps={activeRoundSuggested}
            explanation={roundExplanation}
            onProbe={(a, b, mode) => fetchCounterfactual(activeRound.number, a, b, mode)}
            canForce={canForceActiveRound}
            onForcePairing={(a, b) => run(async () => apply(await forcePairing(a, b)))}
            onClickWinner={(boardIndex, clicked) =>
              handleSetResult(activeRound.number, boardIndex, clicked)}
            onToggleDrawn={(boardIndex, drawn) =>
              handleSetDrawn(activeRound.number, boardIndex, drawn)}
            onSetHandicap={(boardIndex, handicap) =>
              handleSetHandicap(activeRound.number, boardIndex, handicap)}
            {busy}
          />
        {/if}
      </div>
    </section>
  {/if}

  <footer>
    <ServerStatus />
  </footer>
</div>

<style>
  .app {
    width: min(90rem, 95vw);
    margin: 0 auto;
    padding: 2rem 0 3rem;
  }
  header {
    margin-bottom: 1.5rem;
  }
  .header-top {
    display: flex;
    justify-content: center;
    align-items: flex-start;
    position: relative;
  }
  .header-titles {
    text-align: center;
  }
  .header-controls {
    position: absolute;
    right: 0;
    top: 0.2rem;
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  h1 {
    font-size: 1.8rem;
    margin: 0;
  }
  .subtitle {
    color: var(--text-secondary);
    margin: 0.25rem 0 0;
  }
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
  .toolbar-actions .ghost.active {
    border-color: var(--border-accent);
    color: var(--text-on-accent);
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
  .backups-list button.small {
    padding: 0.2rem 0.6rem;
    font-size: 0.78rem;
  }

  .tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    border-bottom: 1px solid var(--border);
    margin-bottom: 1.25rem;
  }
  .tab {
    padding: 0.4rem 0.8rem;
    border: 1px solid transparent;
    border-bottom: none;
    border-radius: 0.4rem 0.4rem 0 0;
    background: transparent;
    color: var(--text-secondary);
    font: inherit;
    cursor: pointer;
    margin-bottom: -1px;
  }
  .tab:hover:not(:disabled):not(.active) {
    color: var(--text);
  }
  .tab.active {
    color: var(--text);
    border-color: var(--border);
    background: var(--bg-surface);
  }
  .tab.draft-tab {
    font-style: italic;
    color: var(--color-accent);
  }
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
  .cup-size select {
    background: var(--bg-raised);
    color: inherit;
    border: 1px solid var(--border);
    border-radius: 0.4rem;
    padding: 0.3rem 0.4rem;
    font: inherit;
  }
  .cup-warn {
    color: var(--color-warning);
  }
  .ctrl {
    padding: 0.35rem 0.75rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-raised);
    color: inherit;
    font: inherit;
    cursor: pointer;
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
  .ratings-status button.small {
    padding: 0.25rem 0.6rem;
    font-size: 0.78rem;
  }
  .players {
    margin-top: 1.25rem;
  }
  .error-banner {
    background: var(--bg-danger);
    border: 1px solid var(--border-danger);
    color: var(--text-on-danger);
    padding: 0.6rem 0.9rem;
    border-radius: 0.5rem;
    font-size: 0.9rem;
    margin-bottom: 1rem;
  }
  .muted {
    color: var(--text-secondary);
    text-align: center;
  }
  footer {
    margin-top: 2rem;
    display: flex;
    justify-content: center;
  }

  @media print {
    header,
    .toolbar,
    .tabs,
    .round-controls,
    footer {
      display: none;
    }
    .card {
      border: none;
      background: transparent;
      padding: 0;
    }
  }
</style>
