<script lang="ts">
  import { _ } from "svelte-i18n";
  import {
    ApiError,
    createTournamentEntry,
    deleteTournamentEntry,
    importTournament,
    listTournaments,
    loginAdmin,
  } from "../api";
  import { describeApiError } from "../errorCodes";
  import { currentTournamentId, getToken, initialTab } from "../session";
  import { loadTournament } from "../tournamentFile";
  import CreateTournament from "./CreateTournament.svelte";
  import type { Tournament, TournamentSummary } from "../types";

  let tournaments = $state<TournamentSummary[]>([]);
  let loading = $state(true);
  let busy = $state(false);
  let error = $state<string | null>(null);

  // Set only when an action requires the admin password we don't have yet —
  // stashes the pending create/load so the retry after logging in can reuse it.
  // At most one is ever set at a time.
  let pendingCreate = $state<{ name: string; password?: string } | null>(null);
  let pendingLoad = $state<{ tournament: Tournament; password?: string } | null>(null);
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
    return describeApiError(err, $_);
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
    if ((!pendingCreate && !pendingLoad) || adminPassword.length === 0) return;
    busy = true;
    error = null;
    try {
      await loginAdmin(adminPassword);
      adminPassword = "";
      // Retry whichever action was waiting on the admin password.
      if (pendingCreate) {
        const { name, password } = pendingCreate;
        await handleCreate(name, password);
      } else if (pendingLoad) {
        const { tournament, password } = pendingLoad;
        await applyLoaded(tournament, password);
      }
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

  async function handleLoad(password: string | undefined) {
    error = null;
    let loaded: Tournament | null;
    try {
      loaded = await loadTournament();
    } catch (err) {
      // Not a file we can read as a tournament at all. This must be caught: the
      // button discards the promise, so the rejection would otherwise go
      // nowhere and the referee would see the dialog close and nothing happen.
      error = describe(err);
      return;
    }
    if (!loaded) return; // user cancelled the file dialog
    await applyLoaded(loaded, password);
  }

  // Hand the file to the server, which validates it and registers it in one
  // step, then switch into what it created. Split out of `handleLoad` so the
  // retry after an admin login can reuse it.
  //
  // Nothing exists until the import succeeds — that atomicity is the server's,
  // not ours: a file this build can't read used to be rejected only after an
  // empty tournament had already been created and selected.
  async function applyLoaded(loaded: Tournament, password: string | undefined) {
    busy = true;
    error = null;
    try {
      const { id } = await importTournament(loaded, password);
      pendingLoad = null;
      select(id);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        // The server requires the admin password to create tournaments —
        // stash this file and ask for it inline, exactly as handleCreate does.
        pendingLoad = { tournament: loaded, password };
      } else {
        error = describe(err);
      }
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
            <button
              type="button"
              class="ghost pick"
              data-testid="select-tournament"
              onclick={() => select(t.id)}
              disabled={busy}
            >
              {#if t.has_password}
                {#if getToken(t.id)}
                  <span class="lock" title={$_("picker.passwordUnlocked")}>🔓</span>
                {:else}
                  <span class="lock" title={$_("picker.passwordProtected")}>🔒</span>
                {/if}
              {/if}
              {t.name}
            </button>
            <button
              type="button"
              class="ghost small danger"
              data-testid="delete-tournament"
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

  {#if pendingCreate || pendingLoad}
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
          <button
            type="submit"
            data-testid="admin-password-submit"
            disabled={busy || adminPassword.length === 0}
          >
            {$_("login.submit")}
          </button>
          <button
            type="button"
            class="ghost"
            data-testid="admin-password-cancel"
            onclick={() => {
              pendingCreate = null;
              pendingLoad = null;
            }}
            disabled={busy}
          >
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
