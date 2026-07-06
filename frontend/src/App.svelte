<script lang="ts">
  import { onMount } from "svelte";
  import {
    addPlayer,
    ApiError,
    completeRound,
    confirmRound,
    createTournament,
    editPlayer,
    fetchRatings,
    fetchTournament,
    finalizeRegistration,
    prepareRound,
    refreshRatings,
    removePlayer,
    replaceTournament,
    setBoardDrawn,
    setBoardHandicap,
    setBoardWinner,
    undoTournament,
    updateDraft,
    updateSettings,
    type DraftUpdate,
  } from "./lib/api";
  import type {
    Handicap,
    NewPlayer,
    RatedPlayer,
    Tournament,
    TournamentResponse,
    Winner,
  } from "./lib/types";
  import { loadTournament, saveTournament } from "./lib/tournamentFile";
  import ServerStatus from "./lib/components/ServerStatus.svelte";
  import CreateTournament from "./lib/components/CreateTournament.svelte";
  import PlayerRegistration from "./lib/components/PlayerRegistration.svelte";
  import PlayerList from "./lib/components/PlayerList.svelte";
  import RoundView from "./lib/components/RoundView.svelte";
  import RoundDraftView from "./lib/components/RoundDraftView.svelte";
  import ResultsView from "./lib/components/ResultsView.svelte";
  import TournamentSettingsView from "./lib/components/TournamentSettingsView.svelte";

  let tournament = $state<Tournament | null>(null);
  let canUndo = $state(false);
  let initialLoad = $state<"loading" | "done">("loading");
  let creatingNew = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let ratings = $state<RatedPlayer[]>([]);

  /** Apply a tournament API response to local state. */
  function apply(res: TournamentResponse) {
    tournament = res.tournament;
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
      ((phase === "registration" && enoughPlayers) ||
        (phase === "in_progress" && currentRoundAllPlayed)),
  );
  const advanceTitle = $derived(
    phase === "registration" && !enoughPlayers
      ? "Register at least 2 players first"
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

  // Keep the selected tab valid (e.g. after undo removes a round).
  $effect(() => {
    if (!tournament) return;
    const valid = new Set([
      "settings",
      "players",
      "results",
      ...tournament.rounds.map((r) => `round-${r.number}`),
      ...(tournament.draft ? ["draft"] : []),
    ]);
    if (!valid.has(activeTab)) activeTab = "players";
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

  function handleUndo() {
    run(async () => {
      apply(await undoTournament());
    });
  }

  function handleAdvance() {
    if (phase === "registration") {
      run(async () => apply(await finalizeRegistration()));
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

  function handleUpdateSettings(macmahonThresholds: number[]) {
    run(async () => {
      apply(await updateSettings(macmahonThresholds));
    });
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
        </div>
      </div>

      <div class="tab-content">
        {#if activeTab === "settings"}
          <TournamentSettingsView
            settings={tournament.settings}
            finalized={tournament.registration_finalized}
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
              onEdit={handleEditPlayer}
              onRemove={handleRemovePlayer}
              {busy}
            />
          </div>
        {:else if activeTab === "results"}
          <ResultsView {tournament} />
        {:else if activeTab === "draft" && tournament.draft}
          <RoundDraftView
            draft={tournament.draft}
            players={tournament.players}
            onUpdate={handleUpdateDraft}
            onConfirm={handleConfirmRound}
            {busy}
          />
        {:else if activeRound}
          <RoundView
            round={activeRound}
            players={tournament.players}
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
