<script lang="ts">
  import type { NewPlayer } from "../types";

  interface Props {
    /** Register a new player. */
    onAdd: (player: NewPlayer) => void;
    /** True while a registration request is in flight. */
    busy?: boolean;
  }

  let { onAdd, busy = false }: Props = $props();

  let name = $state("");
  // Bound to a number input, so Svelte gives us a number (or null when empty).
  let rating = $state<number | null>(null);
  let club = $state("");

  function submit(event: SubmitEvent) {
    event.preventDefault();
    const trimmedName = name.trim();
    if (!trimmedName) return;

    const player: NewPlayer = { name: trimmedName };

    if (rating !== null && Number.isInteger(rating) && rating >= 0) {
      player.rating = rating;
    }

    const trimmedClub = club.trim();
    if (trimmedClub !== "") player.club = trimmedClub;

    onAdd(player);

    // Reset for the next entry; keep focus flowing for fast bulk registration.
    name = "";
    rating = null;
    club = "";
  }
</script>

<form class="registration" onsubmit={submit}>
  <input
    type="text"
    bind:value={name}
    placeholder="Player name"
    autocomplete="off"
    disabled={busy}
    aria-label="Player name"
  />
  <input
    type="number"
    bind:value={rating}
    placeholder="Rating"
    min="0"
    disabled={busy}
    aria-label="Rating (optional)"
    class="rating"
  />
  <input
    type="text"
    bind:value={club}
    placeholder="Club (optional)"
    autocomplete="off"
    disabled={busy}
    aria-label="Club (optional)"
  />
  <button type="submit" disabled={busy || name.trim() === ""}>Add player</button>
</form>

<style>
  .registration {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
    align-items: center;
  }
  input {
    padding: 0.45rem 0.6rem;
    border: 1px solid #34343b;
    border-radius: 0.5rem;
    background: #1b1b1f;
    color: inherit;
    font: inherit;
  }
  input[type="text"] {
    flex: 1 1 10rem;
    min-width: 8rem;
  }
  .rating {
    width: 6rem;
    flex: none;
  }
</style>
