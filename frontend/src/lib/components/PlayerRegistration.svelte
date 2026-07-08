<script lang="ts">
  import { _ } from "svelte-i18n";
  import type { NewPlayer, RatedPlayer } from "../types";
  import { formatGrade, parseGrade } from "../grade";

  interface Props {
    /** Register a new player. */
    onAdd: (player: NewPlayer) => void;
    /** FESA rating list for name/ELO autocomplete (empty if unavailable). */
    ratings?: RatedPlayer[];
    /** True while a registration request is in flight. */
    busy?: boolean;
  }

  let { onAdd, ratings = [], busy = false }: Props = $props();

  let lastNameInput: HTMLInputElement | undefined = $state();
  // Set on submit, while the add-player request is in flight (inputs disabled
  // meanwhile, which drops focus); refocus once it completes.
  let refocusPending = $state(false);
  $effect(() => {
    if (refocusPending && !busy) {
      refocusPending = false;
      lastNameInput?.focus();
    }
  });

  let lastName = $state("");
  let firstName = $state("");
  // Bound to a number input, so Svelte gives us a number (or null when empty).
  let rating = $state<number | null>(null);
  // Free-text grade entry, e.g. "3d" / "3 dan" / "5k" / "5 kyu" — parsed into a
  // Grade on submit; left as text so a referee can type it without a picker.
  let gradeText = $state("");
  let nationality = $state("");
  let club = $state("");
  // The FESA #games behind a picked suggestion, so we can tell the server the
  // rating is FESA-backed (and how reliable). Kept even if the referee edits the
  // rating by hand — the FESA list is often stale and referees routinely bump a
  // known player's rating, so it's still the same (established) player. Cleared
  // only when the last name is edited, since that may point at a different entry.
  let fesaGames = $state<number | null>(null);

  let showSuggestions = $state(false);
  let highlighted = $state(-1);

  /** Lowercase + strip diacritics so "thune" matches "Thuné". */
  function normalize(text: string): string {
    return text
      .normalize("NFD")
      .replace(/[̀-ͯ]/g, "")
      .toLowerCase()
      .trim();
  }

  // Suggestions are driven by what's typed in the last-name field.
  const suggestions = $derived.by<RatedPlayer[]>(() => {
    const query = normalize(lastName);
    if (query.length < 2 || ratings.length === 0) return [];
    const matches = ratings.filter((r) =>
      normalize(`${r.last_name} ${r.first_name}`).includes(query),
    );
    matches.sort((a, b) => {
      // Prefer last names that start with the query, then alphabetical.
      const aStarts = normalize(a.last_name).startsWith(query) ? 0 : 1;
      const bStarts = normalize(b.last_name).startsWith(query) ? 0 : 1;
      return aStarts - bStarts || a.last_name.localeCompare(b.last_name);
    });
    return matches.slice(0, 8);
  });

  function pick(r: RatedPlayer) {
    lastName = r.last_name;
    firstName = r.first_name;
    rating = r.rating;
    fesaGames = r.games; // rating is now FESA-backed
    gradeText = r.grade ? formatGrade(r.grade) : "";
    nationality = r.nationality;
    showSuggestions = false;
    highlighted = -1;
  }

  function onLastNameInput() {
    showSuggestions = true;
    highlighted = -1;
    fesaGames = null; // editing the name breaks the link to the picked entry
  }

  function onLastNameKeydown(event: KeyboardEvent) {
    if (!showSuggestions || suggestions.length === 0) return;
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        highlighted = (highlighted + 1) % suggestions.length;
        break;
      case "ArrowUp":
        event.preventDefault();
        highlighted = (highlighted - 1 + suggestions.length) % suggestions.length;
        break;
      case "Enter":
        if (highlighted >= 0) {
          event.preventDefault(); // select instead of submitting
          pick(suggestions[highlighted]);
        }
        break;
      case "Escape":
        showSuggestions = false;
        highlighted = -1;
        break;
    }
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    const ln = lastName.trim();
    if (!ln) return;

    const player: NewPlayer = { last_name: ln };
    const fn = firstName.trim();
    if (fn) player.first_name = fn;
    if (rating !== null && Number.isInteger(rating) && rating >= 0) {
      player.rating = rating;
      // Only forward the games count if the rating is still the FESA-picked one.
      if (fesaGames !== null) player.fesa_games = fesaGames;
    }
    const grade = parseGrade(gradeText);
    if (grade) player.grade = grade;
    const nat = nationality.trim();
    if (nat) player.nationality = nat;
    const cl = club.trim();
    if (cl) player.club = cl;

    onAdd(player);
    refocusPending = true;

    // Reset for the next entry; keep focus flowing for fast bulk registration.
    lastName = "";
    firstName = "";
    rating = null;
    fesaGames = null;
    gradeText = "";
    nationality = "";
    club = "";
    showSuggestions = false;
    highlighted = -1;
  }
</script>

<form class="registration" onsubmit={submit}>
  <div class="name-field">
    <input
      type="text"
      bind:this={lastNameInput}
      bind:value={lastName}
      oninput={onLastNameInput}
      onkeydown={onLastNameKeydown}
      onblur={() => (showSuggestions = false)}
      placeholder={$_("playerRegistration.lastName")}
      autocomplete="off"
      disabled={busy}
      aria-label={$_("playerRegistration.lastName")}
      role="combobox"
      aria-expanded={showSuggestions && suggestions.length > 0}
      aria-controls="fesa-suggestions"
      aria-activedescendant={highlighted >= 0 ? `fesa-opt-${highlighted}` : undefined}
    />
    {#if showSuggestions && suggestions.length > 0}
      <ul class="suggestions" id="fesa-suggestions" role="listbox">
        {#each suggestions as s, i (s.last_name + s.first_name + s.rating)}
          <!-- Keyboard selection is handled by the combobox input (arrow keys +
               Enter, tracked via aria-activedescendant); this handler is the
               mouse-only affordance. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <li
            id={`fesa-opt-${i}`}
            role="option"
            aria-selected={i === highlighted}
            class:highlighted={i === highlighted}
            onmousedown={(e) => e.preventDefault()}
            onclick={() => pick(s)}
          >
            <span class="s-name">{s.last_name} {s.first_name}</span>
            <span class="s-meta">{s.rating} · {s.nationality}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </div>

  <input
    type="text"
    bind:value={firstName}
    placeholder={$_("playerRegistration.firstName")}
    autocomplete="off"
    disabled={busy}
    aria-label={$_("playerRegistration.firstName")}
  />
  <input
    type="number"
    bind:value={rating}
    placeholder={$_("playerRegistration.ratingPlaceholder")}
    min="0"
    disabled={busy}
    aria-label={$_("playerRegistration.rating")}
    class="rating"
  />
  <input
    type="text"
    bind:value={gradeText}
    placeholder={$_("playerRegistration.gradePlaceholder")}
    autocomplete="off"
    disabled={busy}
    aria-label={$_("playerRegistration.grade")}
    class="grade"
  />
  <input
    type="text"
    bind:value={nationality}
    placeholder={$_("playerRegistration.natPlaceholder")}
    autocomplete="off"
    disabled={busy}
    aria-label={$_("playerRegistration.nationality")}
    class="nat"
    maxlength="3"
  />
  <input
    type="text"
    bind:value={club}
    placeholder={$_("playerRegistration.club")}
    autocomplete="off"
    disabled={busy}
    aria-label={$_("playerRegistration.club")}
    class="club"
  />
  <button type="submit" disabled={busy || lastName.trim() === ""}
    >{$_("playerRegistration.addPlayer")}</button
  >
</form>

<style>
  .registration {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
    align-items: flex-start;
  }
  input {
    padding: 0.45rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-inset);
    color: inherit;
    font: inherit;
  }
  .name-field {
    position: relative;
    flex: 1 1 12rem;
    min-width: 9rem;
  }
  .name-field input {
    width: 100%;
    box-sizing: border-box;
  }
  input[type="text"].club {
    flex: 1 1 8rem;
    min-width: 7rem;
  }
  input[type="text"]:not(.club):not(.nat):not(.grade) {
    flex: 1 1 8rem;
    min-width: 7rem;
  }
  .rating {
    width: 5.5rem;
    flex: none;
  }
  .grade {
    width: 4.5rem;
    flex: none;
  }
  .nat {
    width: 4rem;
    flex: none;
    text-transform: uppercase;
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
</style>
