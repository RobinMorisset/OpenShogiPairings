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
    undoTournament,
  } from "./lib/api";
  import type {
    NewPlayer,
    RatedPlayer,
    Tournament,
    TournamentResponse,
  } from "./lib/types";
  import { loadTournament, saveTournament } from "./lib/tournamentFile";
  import ServerStatus from "./lib/components/ServerStatus.svelte";
  import CreateTournament from "./lib/components/CreateTournament.svelte";
  import PlayerRegistration from "./lib/components/PlayerRegistration.svelte";
  import PlayerList from "./lib/components/PlayerList.svelte";

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
          <span class="count"
            >{tournament.players.length} player{tournament.players.length === 1
              ? ""
              : "s"}</span
          >
        </div>
        <div class="toolbar-actions">
          <button
            type="button"
            class="ghost"
            onclick={handleUndo}
            disabled={busy || !canUndo}
            title="Undo the last player change"
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
  .count {
    color: #9a9aa2;
    font-size: 0.85rem;
  }
  .toolbar-actions {
    display: flex;
    gap: 0.5rem;
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
