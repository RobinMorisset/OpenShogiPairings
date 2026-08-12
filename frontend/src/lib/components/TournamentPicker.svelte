<script lang="ts">
  import { _, locale } from "svelte-i18n";
  import {
    ApiError,
    createTournamentEntry,
    deleteTournamentEntry,
    fetchDeletedTournaments,
    importTournament,
    listTournaments,
    loginAdmin,
    restoreDeletedTournament,
  } from "../api";
  import { describeApiError } from "../errorCodes";
  import { currentTournamentId, getToken, initialTab } from "../session";
  import { loadTournament } from "../tournamentFile";
  import CreateTournament from "./CreateTournament.svelte";
  import type { DeletedTournament, Tournament, TournamentSummary } from "../types";

  let tournaments = $state<TournamentSummary[]>([]);
  let loading = $state(true);
  let busy = $state(false);
  let error = $state<string | null>(null);

  // The bin: tournaments deleted recently enough that their backups are still
  // on the server. Empty (so the section is absent) when nothing is in there —
  // and on a restricted listing, where we have no admin token to fetch it with.
  let deleted = $state<DeletedTournament[]>([]);

  // The entry whose restore needs more from the referee — its password, or
  // which backup to go back to — expanded inline under it. `null` the rest of
  // the time: a restore with nothing to ask about just happens.
  let restoring = $state<DeletedTournament | null>(null);
  let restoreBackupId = $state("");
  let restorePassword = $state("");

  // True when the server answered with only the *published* tournaments,
  // because it has an admin password and we didn't present one. Surfaced rather
  // than swallowed: a referee looking at a list that is quietly missing their
  // tournament would conclude it had been deleted.
  let restricted = $state(false);

  // Set only when an action requires the admin password we don't have yet —
  // stashes the pending create/load so the retry after logging in can reuse it.
  // At most one is ever set at a time; `"list"` is the case with nothing to
  // retry afterwards but the listing itself.
  let pendingCreate = $state<{ name: string; password?: string } | null>(null);
  let pendingLoad = $state<{ tournament: Tournament; password?: string } | null>(null);
  let pendingList = $state(false);
  let adminPassword = $state("");

  async function refresh() {
    loading = true;
    try {
      const listing = await listTournaments();
      tournaments = listing.tournaments;
      restricted = listing.restricted;
      // Only when the listing wasn't restricted: that is exactly the case where
      // this server has no admin password, or we hold a valid admin token — the
      // two in which the bin is ours to read. Asking anyway would 401 and, worse,
      // clear the very token we don't have.
      deleted = restricted ? [] : await fetchDeletedTournaments();
      error = null;
    } catch (err) {
      error = describe(err);
    } finally {
      loading = false;
    }
  }

  /** A timestamp in Unix seconds, in the reader's own locale. */
  function when(seconds: number): string {
    return new Date(seconds * 1000).toLocaleString($locale ?? undefined);
  }

  // Restoring puts the tournament back as it was, backups and all, so there is
  // nothing to confirm. Ask only when there is something to ask: the password it
  // had, or which of several backups to come back at.
  function startRestore(d: DeletedTournament) {
    if (!d.has_password && d.backups.length <= 1) {
      void doRestore(d);
      return;
    }
    restoring = d;
    restoreBackupId = d.backups[0]?.id ?? "";
    restorePassword = "";
  }

  async function doRestore(d: DeletedTournament, options: { backupId?: string; password?: string } = {}) {
    busy = true;
    error = null;
    try {
      const { id } = await restoreDeletedTournament(d.id, options);
      restoring = null;
      restorePassword = "";
      select(id);
    } catch (err) {
      // 403 is this tournament's own password being wrong — the server keeps it
      // distinct from a 401 precisely so we ask again here instead of dropping
      // the admin session.
      error =
        err instanceof ApiError && err.status === 403 ? $_("login.wrongPassword") : describe(err);
    } finally {
      busy = false;
    }
  }

  function submitRestore(event: SubmitEvent) {
    event.preventDefault();
    if (!restoring) return;
    void doRestore(restoring, {
      backupId: restoreBackupId || undefined,
      password: restorePassword || undefined,
    });
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
    if ((!pendingCreate && !pendingLoad && !pendingList) || adminPassword.length === 0) return;
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
      } else {
        pendingList = false;
        await refresh();
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
  <!-- Above the cards, not under them: at the bottom of a page with a
       tournament list and a create form this sat below the fold, so the action
       that failed looked like it had simply done nothing. -->
  {#if error}
    <p class="error-banner" role="alert">{error}</p>
  {/if}

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
    {#if restricted && !loading}
      <p class="muted restricted">
        {$_("picker.restrictedToPublished")}
        <button
          type="button"
          class="ghost small"
          data-testid="unlock-full-list"
          onclick={() => (pendingList = true)}
          disabled={busy}
        >
          {$_("picker.showAll")}
        </button>
      </p>
    {/if}
  </section>

  {#if deleted.length > 0}
    <section class="card deleted-card">
      <h2>{$_("picker.deletedTitle")}</h2>
      <p class="muted">{$_("picker.deletedHint")}</p>
      <ul class="tournaments">
        {#each deleted as d (d.id)}
          <li>
            <span class="deleted-name">
              {#if d.has_password}
                <span class="lock" title={$_("picker.passwordProtected")}>🔒</span>
              {/if}
              {d.name}
            </span>
            <span class="muted deleted-when">
              {$_("picker.deletedWhen", { values: { when: when(d.deleted_at) } })}
            </span>
            <button
              type="button"
              class="ghost small"
              data-testid="restore-tournament"
              onclick={() => startRestore(d)}
              disabled={busy}
            >
              {$_("picker.restore")}
            </button>
          </li>
          {#if restoring?.id === d.id}
            <li class="restore-row">
              <form onsubmit={submitRestore}>
                {#if d.backups.length > 1}
                  <label class="muted">
                    {$_("picker.restoreFrom")}
                    <select bind:value={restoreBackupId} data-testid="restore-backup" disabled={busy}>
                      {#each d.backups as b (b.id)}
                        <option value={b.id}>{when(b.taken_at)} — {b.label}</option>
                      {/each}
                    </select>
                  </label>
                {/if}
                {#if d.has_password}
                  <p class="muted">
                    {$_("picker.restorePasswordPrompt", { values: { name: d.name } })}
                  </p>
                  <input
                    type="password"
                    bind:value={restorePassword}
                    placeholder={$_("login.passwordPlaceholder")}
                    autocomplete="current-password"
                    data-testid="restore-password"
                    disabled={busy}
                  />
                {/if}
                <div class="actions">
                  <button
                    type="submit"
                    data-testid="restore-submit"
                    disabled={busy || (d.has_password && restorePassword.length === 0)}
                  >
                    {$_("picker.restore")}
                  </button>
                  <button
                    type="button"
                    class="ghost"
                    data-testid="restore-cancel"
                    onclick={() => (restoring = null)}
                    disabled={busy}
                  >
                    {$_("createTournament.cancel")}
                  </button>
                </div>
              </form>
            </li>
          {/if}
        {/each}
      </ul>
    </section>
  {/if}

  {#if pendingCreate || pendingLoad || pendingList}
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
              pendingList = false;
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
</section>

<style>
  .picker {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
    align-items: center;
  }
  .list-card,
  .deleted-card,
  .admin-card {
    width: 100%;
    max-width: 26rem;
  }
  .deleted-card .muted {
    margin: 0 0 0.75rem;
  }
  .deleted-name {
    flex: 1;
    padding: 0.6rem 0.4rem;
  }
  .deleted-when {
    font-size: 0.78rem;
  }
  .restore-row {
    display: block;
    padding: 0 0.4rem 0.6rem;
  }
  .restore-row form {
    flex-direction: column;
    align-items: stretch;
  }
  .restore-row p {
    margin: 0;
  }
  .restore-row label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8rem;
  }
  .restore-row select {
    flex: 1;
    min-width: 0;
    padding: 0.35rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: 0.4rem;
    background: var(--bg-inset);
    color: inherit;
    font: inherit;
    font-size: 0.8rem;
  }
  h2 {
    margin: 0 0 1rem;
    font-size: 1.15rem;
  }
  .muted {
    color: var(--text-secondary);
    font-size: 0.9rem;
  }
  .restricted {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin: 0.75rem 0 0;
    font-size: 0.8rem;
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
