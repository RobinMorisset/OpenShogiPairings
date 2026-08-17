<script lang="ts">
  // A text field that only ever yields one of the options it was given.
  //
  // Every candidate already exists — a registered player, a team, a present
  // opponent — so text matching nothing can only be a typo, and there is
  // deliberately no commit path for it: no free-text value, no "create it"
  // fallback. Empty and focused it lists the pool, so it still answers "just
  // show me what's left" the way the <select> it replaces did, while a field
  // of a hundred players is now reachable by typing three letters.
  //
  // The caller owns the choice. `text` is what the field shows whenever it is
  // not being typed in, so a picker that keeps its choice passes the chosen
  // label back and one that clears after every pick passes "". Either way,
  // abandoning a half-typed query (blur, Escape) restores `text` rather than
  // leaving something on screen that was never committed.

  import { matchOptions, type PickerOption } from "../picker";

  interface Props {
    /** What can be chosen, in the order it should be offered. */
    options: PickerOption[];
    /** The committed choice's label, shown while the field is not being typed
     *  in. Empty for a picker that clears after each pick. */
    text?: string;
    placeholder: string;
    /** Defaults to the placeholder; state it when several pickers share one. */
    ariaLabel?: string;
    /** Unique on the page: the listbox and its options build their ids on it. */
    id: string;
    /** Shown under the field when the query matches nothing — the place to say
     *  where a missing candidate would come from. */
    noMatch: string;
    disabled?: boolean;
    /** The field's width, as CSS — the suggestion list matches it, so this is
     *  really "how wide are the labels". `100%` to fill the container. */
    width?: string;
    onPick: (option: PickerOption) => void;
  }

  let {
    options,
    text = "",
    placeholder,
    ariaLabel,
    id,
    noMatch,
    disabled = false,
    width = "14rem",
    onPick,
  }: Props = $props();

  let input: HTMLInputElement | undefined = $state();
  /** Being typed in: the field shows `query` and the list is open. */
  let editing = $state(false);
  let query = $state("");
  /** Which suggestion the arrow keys are on (-1 = none). */
  let highlighted = $state(-1);

  const suggestions = $derived(matchOptions(options, query));
  const open = $derived(editing && !disabled);

  /** Put the caret back here — the caller's affordance for filling one of
   *  these after another without reaching for the mouse. */
  export function focus() {
    input?.focus();
  }

  function start() {
    editing = true;
    query = "";
    highlighted = -1;
  }

  /** Stop editing, dropping the query: the field falls back to `text`. */
  function stop() {
    editing = false;
    query = "";
    highlighted = -1;
  }

  function choose(option: PickerOption) {
    stop();
    onPick(option);
  }

  function keydown(event: KeyboardEvent) {
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        if (suggestions.length > 0) highlighted = (highlighted + 1) % suggestions.length;
        break;
      case "ArrowUp":
        event.preventDefault();
        if (suggestions.length > 0) {
          highlighted = (highlighted - 1 + suggestions.length) % suggestions.length;
        }
        break;
      case "Enter": {
        event.preventDefault();
        // An arrowed-to suggestion wins; failing that, a single match is
        // unambiguous. Anything else — none, or several — has nothing to
        // commit, so Enter does nothing rather than guessing.
        const choice =
          highlighted >= 0
            ? suggestions[highlighted]
            : suggestions.length === 1
              ? suggestions[0]
              : null;
        if (choice) choose(choice);
        break;
      }
      case "Escape":
        event.preventDefault();
        stop();
        break;
    }
  }
</script>

<div class="picker" style:width>
  <input
    type="text"
    class="control-sm control-quiet"
    bind:this={input}
    {placeholder}
    {disabled}
    value={editing ? query : text}
    autocomplete="off"
    autocapitalize="off"
    autocorrect="off"
    spellcheck="false"
    aria-label={ariaLabel ?? placeholder}
    role="combobox"
    aria-expanded={open && suggestions.length > 0}
    aria-controls={`${id}-list`}
    aria-activedescendant={open && highlighted >= 0 ? `${id}-opt-${highlighted}` : undefined}
    onfocus={start}
    oninput={(e) => {
      editing = true;
      query = e.currentTarget.value;
      highlighted = -1;
    }}
    onkeydown={keydown}
    onblur={stop}
  />
  {#if open && suggestions.length > 0}
    <ul class="suggestions" id={`${id}-list`} role="listbox">
      {#each suggestions as s, i (s.key)}
        <!-- Keyboard selection runs through the input (arrow keys + Enter,
             tracked via aria-activedescendant); this handler is the mouse-only
             affordance. `mousedown` is cancelled so the click lands before the
             input's blur closes the list. -->
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <li
          id={`${id}-opt-${i}`}
          role="option"
          aria-selected={i === highlighted}
          class:highlighted={i === highlighted}
          onmousedown={(e) => e.preventDefault()}
          onclick={() => choose(s)}
        >
          <span class="s-name">{s.label}</span>
          {#if s.meta != null}
            <span class="s-meta" class:unofficial={s.metaUnofficial}>{s.meta}</span>
          {/if}
        </li>
      {/each}
    </ul>
  {:else if open && query.trim() !== ""}
    <p class="no-match">{noMatch}</p>
  {/if}
</div>

<style>
  /* The list is positioned against this box, and the field fills it — the
     `width` prop is the one place either dimension is set. */
  .picker {
    position: relative;
    max-width: 100%;
  }
  input {
    width: 100%;
  }
  .suggestions {
    position: absolute;
    z-index: 10;
    top: calc(100% + 2px);
    left: 0;
    right: 0;
    margin: 0;
    padding: 0.25rem;
    list-style: none;
    background: var(--bg-hover);
    border: 1px solid var(--border-soft);
    border-radius: 0.5rem;
    box-shadow: 0 8px 24px var(--shadow-dropdown);
    max-height: 16rem;
    overflow-y: auto;
  }
  .suggestions li {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 0.75rem;
    padding: 0.35rem 0.5rem;
    border-radius: 0.35rem;
    cursor: pointer;
  }
  .suggestions li.highlighted,
  .suggestions li:hover {
    background: var(--bg-hover-strong);
  }
  .s-name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .s-meta {
    color: var(--text-secondary);
    font-size: 0.8rem;
    font-variant-numeric: tabular-nums;
    flex: none;
  }
  /* A pairing ELO is not a real rating, and must never read as one. */
  .s-meta.unofficial {
    font-style: italic;
  }
  .no-match {
    margin: 0.25rem 0 0;
    color: var(--text-secondary);
    font-size: 0.85em;
  }
</style>
