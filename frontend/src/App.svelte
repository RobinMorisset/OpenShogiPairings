<script lang="ts">
  import { onMount } from "svelte";
  import {
    addPlayer,
    addPointAdjustment,
    ApiError,
    cancelRound,
    completeRound,
    confirmRound,
    createTournament,
    editPlayer,
    fetchAmericanGrid,
    fetchRatings,
    fetchTournament,
    finalizeRegistration,
    prepareRound,
    refreshRatings,
    removePlayer,
    removePointAdjustment,
    replaceTournament,
    setBoardDrawn,
    setBoardHandicap,
    setBoardWinner,
    setPlayerEligible,
    undoTournament,
    updateDraft,
    updateSettings,
    type DraftUpdate,
  } from "./lib/api";
  import type {
    CupPodium,
    Handicap,
    NewPlayer,
    RatedPlayer,
    Standing,
    Tournament,
    TournamentResponse,
    TournamentSettings,
    Winner,
  } from "./lib/types";
  import {
    loadTournament,
    saveAmericanGrid,
    saveTournament,
  } from "./lib/tournamentFile";
  import ServerStatus from "./lib/components/ServerStatus.svelte";
  import CreateTournament from "./lib/components/CreateTournament.svelte";
  import PlayerRegistration from "./lib/components/PlayerRegistration.svelte";
  import PlayerList from "./lib/components/PlayerList.svelte";
  import RoundView from "./lib/components/RoundView.svelte";
  import RoundDraftView from "./lib/components/RoundDraftView.svelte";
  import ResultsView from "./lib/components/ResultsView.svelte";
  import TournamentSettingsView from "./lib/components/TournamentSettingsView.svelte";

  let tournament = $state<Tournament | null>(null);
  let standings = $state<Standing[]>([]);
  let cupPodium = $state<CupPodium | null>(null);
  let draftCupPlayers = $state<string[]>([]);
  let canUndo = $state(false);
  let initialLoad = $state<"loading" | "done">("loading");
  let creatingNew = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let ratings = $state<RatedPlayer[]>([]);

  /** Apply a tournament API response to local state. */
  function apply(res: TournamentResponse) {
    tournament = res.tournament;
    standings = res.standings;
    cupPodium = res.cup_podium ?? null;
    draftCupPlayers = res.draft_cup_players ?? [];
    canUndo = res.can_undo;
  }

  // Show the create form when there is no tournament, or when the user
  // explicitly asked to start a new one.
  let showCreate = $derived(tournament === null || creatingNew);

  // Which tab is open: "players", "results", or "round-{n}".
  let activeTab = $state("players");

  const activeRound = $derived(
    tournament?.rounds.find((r) => `round-${r.number}` === activeTab) ?? null,
  );

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
      ? "Finalize registration"
      : phase === "in_progress"
        ? `Complete round ${currentRound?.number}`
        : (tournament?.rounds.length ?? 0) > 0
          ? `Round ${tournament?.rounds.length} complete`
          : "Registration finalized",
  );
  const advanceEnabled = $derived(
    !busy &&
      ((phase === "registration" && enoughPlayers && cupReady) ||
        (phase === "in_progress" && currentRoundAllPlayed)),
  );
  const advanceTitle = $derived(
    phase === "registration" && !enoughPlayers
      ? "Register at least 2 players first"
      : phase === "registration" && !cupReady
        ? "Mark at least 8 players eligible for the cup, or turn the cup off in Settings"
        : phase === "in_progress" && !currentRoundAllPlayed
          ? "All games in the round must be played first"
          : "",
  );

  // "Start round" button.
  const nextRoundNumber = $derived((tournament?.rounds.length ?? 0) + 1);
  const startEnabled = $derived(!busy && phase === "ready" && enoughPlayers);
  const startTitle = $derived(
    phase !== "ready"
      ? "Finalize registration / complete the current round first"
      : !enoughPlayers
        ? "Need at least 2 players"
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
      ? "Complete a round first"
      : phase !== "ready"
        ? "Finish the current round before exporting"
        : "Download the American Grid (cross-table) for ELO",
  );

  // "Cancel last round" button: peels back one stage — discards the open draft,
  // or removes the most recent round — whenever there is anything to cancel.
  const canCancel = $derived(
    !busy && (!!tournament?.draft || (tournament?.rounds.length ?? 0) > 0),
  );
  const cancelTitle = $derived(
    tournament?.draft
      ? "Discard the round being prepared"
      : (tournament?.rounds.length ?? 0) > 0
        ? "Remove the last round and return to the previous state (undoable)"
        : "No round to cancel",
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

  onMount(async () => {
    // Load the FESA ratings in the background — autocomplete is a nice-to-have,
    // so a failure here must not block or error the rest of the app.
    fetchRatings()
      .then((r) => (ratings = r))
      .catch(() => {
        /* autocomplete simply unavailable */
      });

    try {
      const res = await fetchTournament();
      if (res) apply(res);
    } catch (err) {
      error = describe(err);
    } finally {
      initialLoad = "done";
    }
  });

  function describe(err: unknown): string {
    if (err instanceof ApiError && err.status === 0) {
      return "Cannot reach the server. Is it running (cargo run -p osp-server)?";
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
      error = describe(err);
    } finally {
      busy = false;
    }
  }

  function handleCreate(name: string) {
    run(async () => {
      apply(await createTournament(name));
      creatingNew = false;
    });
  }

  function handleLoad() {
    run(async () => {
      const loaded = await loadTournament();
      if (!loaded) return; // user cancelled the file dialog
      apply(await replaceTournament(loaded));
      creatingNew = false;
    });
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
      await saveTournament(current);
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
    <h1>OpenShogiPairings</h1>
    <p class="subtitle">Tournament management for shogi</p>
  </header>

  {#if error}
    <p class="error-banner" role="alert">{error}</p>
  {/if}

  {#if initialLoad === "loading"}
    <p class="muted">Loading…</p>
  {:else if showCreate}
    <CreateTournament
      onCreate={handleCreate}
      onLoad={handleLoad}
      onCancel={tournament ? () => (creatingNew = false) : undefined}
      {busy}
    />
  {:else if tournament}
    <section class="card">
      <div class="toolbar">
        <div class="title">
          <h2>{tournament.name}</h2>
        </div>
        <div class="toolbar-actions">
          <button
            type="button"
            class="ghost"
            onclick={handleUndo}
            disabled={busy || !canUndo}
            title="Undo the last change"
          >
            Undo
          </button>
          <button type="button" class="ghost" onclick={handleSave} disabled={busy}>
            Save
          </button>
          <button
            type="button"
            class="ghost"
            onclick={() => (creatingNew = true)}
            disabled={busy}
          >
            New…
          </button>
        </div>
      </div>

      <div class="tabs" role="tablist">
        <button
          type="button"
          class="tab"
          class:active={activeTab === "settings"}
          onclick={() => (activeTab = "settings")}
        >
          Settings
        </button>
        <button
          type="button"
          class="tab"
          class:active={activeTab === "players"}
          onclick={() => (activeTab = "players")}
        >
          Players ({tournament.players.length})
        </button>
        <button
          type="button"
          class="tab"
          class:active={activeTab === "results"}
          onclick={() => (activeTab = "results")}
        >
          Results
        </button>
        {#each tournament.rounds as round (round.number)}
          <button
            type="button"
            class="tab"
            class:active={activeTab === `round-${round.number}`}
            onclick={() => (activeTab = `round-${round.number}`)}
          >
            Round {round.number}{round.completed ? " ✓" : ""}
          </button>
        {/each}
        {#if tournament.draft}
          <button
            type="button"
            class="tab draft-tab"
            class:active={activeTab === "draft"}
            onclick={() => (activeTab = "draft")}
          >
            Round {tournament.draft.number} (draft)
          </button>
        {/if}
        <div class="round-controls">
          {#if phase === "registration" && cupEnabled}
            <label class="cup-size" title="Bracket size for the direct-elimination cup">
              Cup:
              {#if validCupSizes.length > 0}
                <select bind:value={cupSizeChoice} disabled={busy}>
                  {#each validCupSizes as s (s)}
                    <option value={s}>Top {s}</option>
                  {/each}
                </select>
              {:else}
                <span class="cup-warn">need ≥8 eligible</span>
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
            Prepare round {nextRoundNumber}
          </button>
          <button
            type="button"
            class="ctrl"
            onclick={handleExportGrid}
            disabled={!exportEnabled}
            title={exportTitle}
          >
            Export grid
          </button>
          <button
            type="button"
            class="ctrl danger"
            onclick={handleCancelRound}
            disabled={!canCancel}
            title={cancelTitle}
          >
            Cancel last round
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
                {ratings.length} players in the FESA rating list
              {:else}
                FESA rating list not loaded
              {/if}
            </span>
            <button
              type="button"
              class="ghost small"
              onclick={handleRefreshRatings}
              disabled={busy}
              title="Re-download the rating list from the FESA website"
            >
              Refresh FESA list
            </button>
          </div>
          <div class="players">
            <PlayerList
              players={tournament.players}
              showEligible={cupEnabled}
              onEdit={handleEditPlayer}
              onRemove={handleRemovePlayer}
              onToggleEligible={handleToggleEligible}
              onAddAdjustment={handleAddPointAdjustment}
              onRemoveAdjustment={handleRemovePointAdjustment}
              {busy}
            />
          </div>
        {:else if activeTab === "results"}
          <ResultsView {tournament} {standings} {cupPodium} />
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
            {standings}
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
    width: min(46rem, 92vw);
    margin: 0 auto;
    padding: 2rem 0 3rem;
  }
  header {
    text-align: center;
    margin-bottom: 1.5rem;
  }
  h1 {
    font-size: 1.8rem;
    margin: 0;
  }
  .subtitle {
    color: #9a9aa2;
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
  .toolbar-actions {
    display: flex;
    gap: 0.5rem;
  }

  .tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    border-bottom: 1px solid #34343b;
    margin-bottom: 1.25rem;
  }
  .tab {
    padding: 0.4rem 0.8rem;
    border: 1px solid transparent;
    border-bottom: none;
    border-radius: 0.4rem 0.4rem 0 0;
    background: transparent;
    color: #9a9aa2;
    font: inherit;
    cursor: pointer;
    margin-bottom: -1px;
  }
  .tab:hover:not(:disabled):not(.active) {
    color: #e6e6e6;
  }
  .tab.active {
    color: #e6e6e6;
    border-color: #34343b;
    background: #232329;
  }
  .tab.draft-tab {
    font-style: italic;
    color: #7aa2f7;
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
    color: #9a9aa2;
  }
  .cup-size select {
    background: #2d2d34;
    color: inherit;
    border: 1px solid #34343b;
    border-radius: 0.4rem;
    padding: 0.3rem 0.4rem;
    font: inherit;
  }
  .cup-warn {
    color: #d29922;
  }
  .ctrl {
    padding: 0.35rem 0.75rem;
    border: 1px solid #34343b;
    border-radius: 0.5rem;
    background: #2d2d34;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .ctrl:hover:not(:disabled) {
    border-color: #4a4a52;
  }
  .ctrl:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .ctrl.primary:not(:disabled) {
    border-color: #3b5bdb;
    background: #2b3a67;
    color: #cdd6f4;
  }
  .ctrl.danger:not(:disabled) {
    border-color: #6e2329;
    color: #ffb4ab;
  }
  .ctrl.danger:hover:not(:disabled) {
    border-color: #a13b3b;
    background: #3d1417;
  }
  .ratings-status {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    margin-top: 0.6rem;
    font-size: 0.8rem;
    color: #9a9aa2;
  }
  .ratings-status button.small {
    padding: 0.25rem 0.6rem;
    font-size: 0.78rem;
  }
  .players {
    margin-top: 1.25rem;
  }
  .error-banner {
    background: #3d1417;
    border: 1px solid #6e2329;
    color: #ffb4ab;
    padding: 0.6rem 0.9rem;
    border-radius: 0.5rem;
    font-size: 0.9rem;
    margin-bottom: 1rem;
  }
  .muted {
    color: #9a9aa2;
    text-align: center;
  }
  footer {
    margin-top: 2rem;
    display: flex;
    justify-content: center;
  }
</style>
