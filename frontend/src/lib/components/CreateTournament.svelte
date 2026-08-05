<script lang="ts">
  import { _ } from "svelte-i18n";
  import { isTauri } from "../platform";

  interface Props {
    /** Create a new tournament with the given name and optional password. */
    onCreate: (name: string, password: string | undefined) => void;
    /**
     * Open a file picker and load the chosen tournament as a new entry, with
     * the same optional password the create form offers — the field below is
     * shared, since either way the referee is adding one tournament to this
     * server and may want it protected.
     */
    onLoad: (password: string | undefined) => void;
    /** True while a create/load request is in flight. */
    busy?: boolean;
  }

  let { onCreate, onLoad, busy = false }: Props = $props();

  let name = $state("");
  let password = $state("");

  /** The password to protect the new tournament with, or none if left blank. */
  function chosenPassword(): string | undefined {
    return password.trim() || undefined;
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    const trimmed = name.trim();
    if (trimmed) onCreate(trimmed, chosenPassword());
  }
</script>

<section class="create card">
  <h2>{$_("createTournament.title")}</h2>
  <form onsubmit={submit}>
    <label>
      {$_("createTournament.name")}
      <input
        type="text"
        bind:value={name}
        placeholder={$_("createTournament.namePlaceholder")}
        autocomplete="off"
        disabled={busy}
      />
    </label>
    {#if !isTauri()}
      <label>
        {$_("createTournament.password")}
        <input
          type="password"
          bind:value={password}
          placeholder={$_("createTournament.passwordPlaceholder")}
          autocomplete="new-password"
          disabled={busy}
        />
      </label>
    {/if}
    <div class="actions">
      <button type="submit" data-testid="create-tournament" disabled={busy || name.trim() === ""}>
        {$_("createTournament.create")}
      </button>
    </div>

    <!-- Inside the form, and below the password field, so that field plainly
         governs both ways of adding a tournament rather than reading as
         create-only. `type="button"` keeps it from submitting. -->
    <div class="or">{$_("createTournament.or")}</div>

    <button
      type="button"
      class="ghost"
      data-testid="load-tournament"
      disabled={busy}
      onclick={() => onLoad(chosenPassword())}
    >
      {$_("createTournament.loadFromFile")}
    </button>
  </form>
</section>

<style>
  .create {
    max-width: 26rem;
  }
  h2 {
    margin: 0 0 1rem;
    font-size: 1.15rem;
  }
  label {
    display: block;
    text-align: left;
    font-size: 0.85rem;
    color: var(--text-secondary);
    margin-top: 0.6rem;
  }
  label:first-child {
    margin-top: 0;
  }
  input[type="text"],
  input[type="password"] {
    display: block;
    width: 100%;
    box-sizing: border-box;
    margin-top: 0.3rem;
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
    margin-top: 0.9rem;
  }
  .or {
    margin: 1rem 0 0.75rem;
    color: var(--text-tertiary);
    font-size: 0.8rem;
  }
</style>
