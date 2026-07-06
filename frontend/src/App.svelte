<script lang="ts">
  import { onMount } from "svelte";
  import {
    addPlayer,
    ApiError,
    createTournament,
    editPlayer,
    fetchRatings,
    fetchTournament,
    refreshRatings,
    removePlayer,
    replaceTournament,
    setBoardWinner,
    startRound,
    undoTournament,
  } from "./lib/api";
  import type {
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

  // Keep the selected tab valid (e.g. after undo removes a round).
  $effect(() => {
    if (!tournament) return;
    const valid = new Set([
      "players",
      "results",
      ...tournament.rounds.map((r) => `round-${r.number}`),
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

  function handleStartRound() {
    run(async () => {
      apply(await startRound());
      // Jump to the round we just created.
      if (tournament) activeTab = `round-${tournament.rounds.length}`;
    });
  }

  function handleSetResult(roundNumber: number, boardIndex: number, clicked: Winner) {
    run(async () => {
      apply(await setBoardWinner(roundNumber, boardIndex, clicked));
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
            Round {round.number}
          </button>
        {/each}
        <button
          type="button"
          class="tab start-round"
          onclick={handleStartRound}
          disabled={busy || tournament.players.length < 2}
          title={tournament.players.length < 2
            ? "Need at least 2 players to start a round"
            : "Pair the next round"}
        >
          + Start round {tournament.rounds.length + 1}
        </button>
      </div>

      <div class="tab-content">
        {#if activeTab === "players"}
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
          <p class="muted placeholder">
            Standings will appear here once round results can be entered
            (coming soon).
          </p>
        {:else if activeRound}
          <RoundView
            round={activeRound}
            players={tournament.players}
            onClickWinner={(boardIndex, clicked) =>
              handleSetResult(activeRound.number, boardIndex, clicked)}
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
  .tab.start-round {
    margin-left: auto;
    color: #7aa2f7;
  }
  .tab.start-round:disabled {
    color: #6a6a72;
    cursor: not-allowed;
  }
  .placeholder {
    padding: 1.5rem 0;
    text-align: center;
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
