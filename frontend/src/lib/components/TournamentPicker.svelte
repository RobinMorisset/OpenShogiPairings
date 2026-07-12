<script lang="ts">
  import { _ } from "svelte-i18n";
  import {
    ApiError,
    createTournamentEntry,
    deleteTournamentEntry,
    listTournaments,
    loginAdmin,
    replaceTournament,
  } from "../api";
  import { currentTournamentId, initialTab } from "../session";
  import { loadTournament } from "../tournamentFile";
  import CreateTournament from "./CreateTournament.svelte";
  import type { TournamentSummary } from "../types";

  let tournaments = $state<TournamentSummary[]>([]);
  let loading = $state(true);
  let busy = $state(false);
  let error = $state<string | null>(null);

  // Set only when creating requires the admin password we don't have yet —
  // stashes the pending create so the retry after logging in can reuse it.
  let pendingCreate = $state<{ name: string; password?: string } | null>(null);
  let adminPassword = $state("");

  async function refresh() {
    loading = true;
    try {
      tournaments = await listTournaments();
      error = null;
    } catch (err) {
      error = describe(err);
    } finally {
      loading = false;
    }
  }

  function describe(err: unknown): string {
    if (err instanceof ApiError && err.status === 0) {
      return $_("app.cannotReachServer");
    }
    return err instanceof Error ? err.message : String(err);
  }

  function select(id: string) {
    currentTournamentId.set(id);
  }

  async function handleCreate(name: string, password: string | undefined) {
    busy = true;
    error = null;
    try {
      const { id } = await createTournamentEntry(name, password);
      pendingCreate = null;
      initialTab.set("settings");
      select(id);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        // The server requires the admin password to create tournaments —
        // stash this attempt and ask for it inline.
        pendingCreate = { name, password };
      } else {
        error = describe(err);
      }
    } finally {
      busy = false;
    }
  }

  async function submitAdminPassword(event: SubmitEvent) {
    event.preventDefault();
    if (!pendingCreate || adminPassword.length === 0) return;
    busy = true;
    error = null;
    try {
      await loginAdmin(adminPassword);
      adminPassword = "";
      const { name, password } = pendingCreate;
      await handleCreate(name, password);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        error = $_("login.wrongPassword");
      } else {
        error = describe(err);
      }
    } finally {
      busy = false;
    }
  }

  async function handleLoad() {
    const loaded = await loadTournament();
    if (!loaded) return; // user cancelled the file dialog
    busy = true;
    error = null;
    try {
      const { id } = await createTournamentEntry(loaded.name);
      select(id);
      // `select` synchronously updates api.ts's notion of the open tournament,
      // so this can run right after: it seeds the new (blank) entry with the
      // file's actual contents.
      await replaceTournament(loaded);
    } catch (err) {
      error = describe(err);
    } finally {
      busy = false;
    }
  }

  async function handleDelete(t: TournamentSummary) {
    if (!window.confirm($_("picker.confirmDelete", { values: { name: t.name } }))) return;
    busy = true;
    error = null;
    try {
      await deleteTournamentEntry(t.id);
      await refresh();
    } catch (err) {
      error = describe(err);
    } finally {
      busy = false;
    }
  }

  void refresh();
</script>

<section class="picker">
  <section class="card list-card">
    <h2>{$_("picker.title")}</h2>
    {#if loading}
      <p class="muted">{$_("app.loading")}</p>
    {:else if tournaments.length === 0}
      <p class="muted">{$_("picker.empty")}</p>
    {:else}
      <ul class="tournaments">
        {#each tournaments as t (t.id)}
          <li>
            <button type="button" class="ghost pick" onclick={() => select(t.id)} disabled={busy}>
              {#if t.has_password}
                <span class="lock" title={$_("picker.passwordProtected")}>🔒</span>
              {/if}
              {t.name}
            </button>
            <button
              type="button"
              class="ghost small danger"
              onclick={() => handleDelete(t)}
              disabled={busy}
              title={$_("picker.delete")}
            >
              {$_("picker.delete")}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  {#if pendingCreate}
    <section class="card admin-card">
      <h2>{$_("picker.adminPasswordTitle")}</h2>
      <p class="muted">{$_("picker.adminPasswordPrompt")}</p>
      <form onsubmit={submitAdminPassword}>
        <input
          type="password"
          bind:value={adminPassword}
          placeholder={$_("login.passwordPlaceholder")}
          autocomplete="current-password"
          disabled={busy}
        />
        <div class="actions">
          <button type="submit" disabled={busy || adminPassword.length === 0}>
            {$_("login.submit")}
          </button>
          <button type="button" class="ghost" onclick={() => (pendingCreate = null)} disabled={busy}>
            {$_("createTournament.cancel")}
          </button>
        </div>
      </form>
    </section>
  {:else}
    <CreateTournament onCreate={handleCreate} onLoad={handleLoad} {busy} />
  {/if}

  {#if error}
    <p class="error-banner" role="alert">{error}</p>
  {/if}
</section>

<style>
  .picker {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
    align-items: center;
  }
  .list-card,
  .admin-card {
    width: 100%;
    max-width: 26rem;
  }
  h2 {
    margin: 0 0 1rem;
    font-size: 1.15rem;
  }
  .muted {
    color: var(--text-secondary);
    font-size: 0.9rem;
  }
  .tournaments {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .tournaments li {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    border-bottom: 1px solid var(--border-divider);
  }
  .tournaments li:last-child {
    border-bottom: none;
  }
  .pick {
    flex: 1;
    text-align: left;
    padding: 0.6rem 0.4rem;
    border: none;
    border-radius: 0.4rem;
  }
  .lock {
    margin-right: 0.4rem;
  }
  .small {
    padding: 0.25rem 0.6rem;
    font-size: 0.78rem;
  }
  .danger {
    color: var(--text-on-danger);
  }
  form {
    display: flex;
    gap: 0.5rem;
  }
  input[type="password"] {
    flex: 1;
    box-sizing: border-box;
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-inset);
    color: inherit;
    font: inherit;
  }
  .actions {
    display: flex;
    gap: 0.5rem;
  }
  .error-banner {
    width: 100%;
    max-width: 26rem;
    box-sizing: border-box;
    background: var(--bg-danger);
    border: 1px solid var(--border-danger);
    color: var(--text-on-danger);
    padding: 0.6rem 0.9rem;
    border-radius: 0.5rem;
    font-size: 0.9rem;
  }
</style>
