<script lang="ts">
  interface Props {
    /** Create a new tournament with the given name. */
    onCreate: (name: string) => void;
    /** Load a tournament from a chosen file. */
    onLoad: (file: File) => void;
    /** Cancel and return to the existing tournament (only when one exists). */
    onCancel?: () => void;
    /** True while a create/load request is in flight. */
    busy?: boolean;
  }

  let { onCreate, onLoad, onCancel, busy = false }: Props = $props();

  let name = $state("");
  let fileInput = $state<HTMLInputElement>();

  function submit(event: SubmitEvent) {
    event.preventDefault();
    const trimmed = name.trim();
    if (trimmed) onCreate(trimmed);
  }

  function onFileChosen(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    if (file) onLoad(file);
    input.value = ""; // allow re-selecting the same file later
  }
</script>

<section class="create card">
  <h2>New tournament</h2>
  <form onsubmit={submit}>
    <label>
      Name
      <input
        type="text"
        bind:value={name}
        placeholder="e.g. Paris Open 2026"
        autocomplete="off"
        disabled={busy}
      />
    </label>
    <div class="actions">
      <button type="submit" disabled={busy || name.trim() === ""}>
        Create
      </button>
      {#if onCancel}
        <button type="button" class="ghost" onclick={onCancel} disabled={busy}>
          Cancel
        </button>
      {/if}
    </div>
  </form>

  <div class="or">or</div>

  <button type="button" class="ghost" disabled={busy} onclick={() => fileInput?.click()}>
    Load from file…
  </button>
  <input
    bind:this={fileInput}
    type="file"
    accept=".json,application/json"
    class="hidden-file"
    onchange={onFileChosen}
  />
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
    color: #9a9aa2;
  }
  input[type="text"] {
    display: block;
    width: 100%;
    box-sizing: border-box;
    margin-top: 0.3rem;
    padding: 0.5rem 0.6rem;
    border: 1px solid #34343b;
    border-radius: 0.5rem;
    background: #1b1b1f;
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
    color: #6a6a72;
    font-size: 0.8rem;
  }
  .hidden-file {
    display: none;
  }
</style>
