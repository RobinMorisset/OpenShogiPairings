<script lang="ts">
  import type { NewPlayer, Player } from "../types";

  interface Props {
    players: Player[];
    /** Show the cup-eligibility column (when the hybrid cup is enabled). */
    showEligible?: boolean;
    /** Commit an in-place edit of a player's fields. */
    onEdit: (id: string, player: NewPlayer) => void;
    onRemove: (id: string) => void;
    /** Toggle a player's cup eligibility. */
    onToggleEligible?: (id: string, eligible: boolean) => void;
    /** Apply a manual point bonus (positive delta) or malus (negative) to a player. */
    onAddAdjustment?: (id: string, delta: number, reason: string) => void;
    /** Remove a previously applied point adjustment. */
    onRemoveAdjustment?: (id: string, adjustmentId: string) => void;
    busy?: boolean;
  }

  let {
    players,
    showEligible = false,
    onEdit,
    onRemove,
    onToggleEligible,
    onAddAdjustment,
    onRemoveAdjustment,
    busy = false,
  }: Props = $props();

  const eligibleCount = $derived(players.filter((p) => p.eligible).length);

  // #, last name, first name, rating, nat., club, [cup], actions.
  const colCount = $derived(6 + (showEligible ? 1 : 0) + 1);

  type Field = "last_name" | "first_name" | "rating" | "nationality" | "club";

  // Display order: highest ELO first, players without a rating at the bottom.
  // `sort` is stable, so equal ratings (and the no-rating group) keep their
  // registration order. This is display only — the underlying order is
  // untouched, and edits key off `player.id`.
  const sorted = $derived(
    [...players].sort((a, b) => {
      if (a.rating == null && b.rating == null) return 0;
      if (a.rating == null) return 1;
      if (b.rating == null) return -1;
      return b.rating - a.rating;
    }),
  );

  // Which cell (if any) is being edited, and its working text value.
  let editing = $state<{ id: string; field: Field } | null>(null);
  let draft = $state("");

  function displayValue(p: Player, field: Field): string {
    switch (field) {
      case "last_name":
        return p.last_name;
      case "first_name":
        return p.first_name;
      case "rating":
        return p.rating != null ? String(p.rating) : "";
      case "nationality":
        return p.nationality ?? "";
      case "club":
        return p.club ?? "";
    }
  }

  function startEdit(p: Player, field: Field) {
    if (busy) return;
    editing = { id: p.id, field };
    draft = displayValue(p, field);
  }

  function cancel() {
    editing = null;
  }

  function commit() {
    const active = editing;
    if (!active) return;
    editing = null;

    const p = players.find((x) => x.id === active.id);
    if (!p) return;

    const value = draft.trim();
    if (value === displayValue(p, active.field)) return; // unchanged
    if (active.field === "last_name" && value === "") return; // required field

    onEdit(active.id, buildEdit(p, active.field, value));
  }

  /** Full player payload with one field changed. */
  function buildEdit(p: Player, field: Field, value: string): NewPlayer {
    const edited: NewPlayer = {
      last_name: p.last_name,
      first_name: p.first_name,
      rating: p.rating,
      nationality: p.nationality,
      club: p.club,
    };
    switch (field) {
      case "last_name":
        edited.last_name = value;
        break;
      case "first_name":
        edited.first_name = value || undefined;
        break;
      case "nationality":
        edited.nationality = value || undefined;
        break;
      case "club":
        edited.club = value || undefined;
        break;
      case "rating": {
        const n = Number(value);
        edited.rating = value !== "" && Number.isInteger(n) && n >= 0 ? n : undefined;
        break;
      }
    }
    return edited;
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      commit();
    } else if (event.key === "Escape") {
      event.preventDefault();
      cancel();
    }
  }

  function autofocus(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  // Which player's adjustments panel (if any) is expanded, and the working
  // values for the "add adjustment" mini-form within it.
  let adjustingId = $state<string | null>(null);
  let adjustmentDelta = $state("");
  let adjustmentReason = $state("");

  function toggleAdjustments(id: string) {
    if (adjustingId === id) {
      adjustingId = null;
      return;
    }
    adjustingId = id;
    adjustmentDelta = "";
    adjustmentReason = "";
  }

  function adjustmentTotal(p: Player): number {
    return (p.adjustments ?? []).reduce((sum, a) => sum + a.delta, 0);
  }

  function adjustmentTitle(p: Player): string {
    return (p.adjustments ?? [])
      .map((a) => `${a.delta > 0 ? "+" : ""}${a.delta} — ${a.reason}`)
      .join("\n");
  }

  function submitAdjustment(id: string) {
    const delta = Number(adjustmentDelta);
    const reason = adjustmentReason.trim();
    if (!Number.isInteger(delta) || delta === 0 || reason === "") return;
    onAddAdjustment?.(id, delta, reason);
    adjustmentDelta = "";
    adjustmentReason = "";
  }
</script>

{#snippet cell(player: Player, field: Field, numeric: boolean)}
  {#if editing?.id === player.id && editing.field === field}
    <input
      class="cell-input"
      type={numeric ? "number" : "text"}
      min={numeric ? "0" : undefined}
      value={draft}
      oninput={(e) => (draft = e.currentTarget.value)}
      onkeydown={onKeydown}
      onblur={commit}
      use:autofocus
    />
  {:else}
    <button
      type="button"
      class="cell-btn"
      disabled={busy}
      title="Click to edit"
      onclick={() => startEdit(player, field)}
    >
      {displayValue(player, field) || "—"}
    </button>
  {/if}
{/snippet}

{#if showEligible}
  <p class="elig-count">{eligibleCount} eligible for the cup</p>
{/if}
{#if players.length === 0}
  <p class="empty">No players registered yet.</p>
{:else}
  <table>
    <thead>
      <tr>
        <th class="num">#</th>
        <th>Last name</th>
        <th>First name</th>
        <th class="num">Rating</th>
        <th>Nat.</th>
        <th>Club</th>
        {#if showEligible}<th class="elig" title="Eligible for the cup">Cup</th>{/if}
        <th aria-label="Actions"></th>
      </tr>
    </thead>
    <tbody>
      {#each sorted as player, i (player.id)}
        <tr>
          <td class="num">{i + 1}</td>
          <td>{@render cell(player, "last_name", false)}</td>
          <td>{@render cell(player, "first_name", false)}</td>
          <td class="num">{@render cell(player, "rating", true)}</td>
          <td>{@render cell(player, "nationality", false)}</td>
          <td>{@render cell(player, "club", false)}</td>
          {#if showEligible}
            <td class="elig">
              <input
                type="checkbox"
                checked={player.eligible ?? false}
                disabled={busy}
                title="Eligible for the direct-elimination cup"
                onchange={(e) => onToggleEligible?.(player.id, e.currentTarget.checked)}
              />
            </td>
          {/if}
          <td class="actions">
            {#if adjustmentTotal(player) !== 0}
              <span class="adj-badge" title={adjustmentTitle(player)}>
                {adjustmentTotal(player) > 0 ? "+" : ""}{adjustmentTotal(player)}
              </span>
            {/if}
            <button
              type="button"
              class="adjust"
              title="Manual point bonus/malus"
              disabled={busy}
              onclick={() => toggleAdjustments(player.id)}
            >
              ±
            </button>
            <button
              type="button"
              class="remove"
              title="Remove player"
              disabled={busy}
              onclick={() => onRemove(player.id)}
            >
              ✕
            </button>
          </td>
        </tr>
        {#if adjustingId === player.id}
          <tr class="adjustments-row">
            <td colspan={colCount}>
              {#if (player.adjustments ?? []).length > 0}
                <ul class="adjustments-list">
                  {#each player.adjustments ?? [] as adj (adj.id)}
                    <li>
                      <span class:bonus={adj.delta > 0} class:malus={adj.delta < 0}>
                        {adj.delta > 0 ? "+" : ""}{adj.delta}
                      </span>
                      <span class="reason">{adj.reason}</span>
                      <button
                        type="button"
                        class="remove-adj"
                        title="Remove this adjustment"
                        disabled={busy}
                        onclick={() => onRemoveAdjustment?.(player.id, adj.id)}
                      >
                        ✕
                      </button>
                    </li>
                  {/each}
                </ul>
              {/if}
              <div class="adjustment-form">
                <input
                  class="adj-delta"
                  type="number"
                  placeholder="±pts"
                  value={adjustmentDelta}
                  oninput={(e) => (adjustmentDelta = e.currentTarget.value)}
                />
                <input
                  class="adj-reason"
                  type="text"
                  placeholder="Reason"
                  value={adjustmentReason}
                  oninput={(e) => (adjustmentReason = e.currentTarget.value)}
                  onkeydown={(e) => e.key === "Enter" && submitAdjustment(player.id)}
                />
                <button
                  type="button"
                  class="add-adj"
                  disabled={busy}
                  onclick={() => submitAdjustment(player.id)}
                >
                  Add
                </button>
              </div>
            </td>
          </tr>
        {/if}
      {/each}
    </tbody>
  </table>
{/if}

<style>
  .empty {
    color: #9a9aa2;
    font-size: 0.9rem;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.9rem;
  }
  th,
  td {
    padding: 0.25rem 0.4rem;
    border-bottom: 1px solid #2b2b31;
    text-align: left;
  }
  th {
    color: #9a9aa2;
    font-weight: 600;
    font-size: 0.8rem;
    padding: 0.4rem;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .actions {
    text-align: right;
    width: 5rem;
    white-space: nowrap;
  }
  .elig {
    text-align: center;
    width: 2.5rem;
  }
  .elig-count {
    color: #9a9aa2;
    font-size: 0.82rem;
    margin: 0 0 0.5rem;
  }

  /* Editable cells: a plain-looking, full-width button that reveals an input. */
  .cell-btn {
    width: 100%;
    padding: 0.2rem 0.35rem;
    border: 1px solid transparent;
    border-radius: 0.35rem;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: inherit;
    cursor: text;
  }
  .cell-btn:hover:not(:disabled) {
    border-color: #3a3a42;
    background: #26262c;
  }
  .num .cell-btn {
    text-align: right;
  }
  .cell-input {
    width: 100%;
    box-sizing: border-box;
    padding: 0.2rem 0.35rem;
    border: 1px solid #4a4a8a;
    border-radius: 0.35rem;
    background: #1b1b1f;
    color: inherit;
    font: inherit;
  }
  .num .cell-input {
    text-align: right;
  }

  .remove {
    padding: 0.1rem 0.4rem;
    font-size: 0.8rem;
    color: #f85149;
    border-color: transparent;
    background: transparent;
  }
  .remove:hover:not(:disabled) {
    border-color: #f85149;
  }

  .adjust {
    padding: 0.1rem 0.4rem;
    font-size: 0.8rem;
    color: #9a9aa2;
    border: 1px solid transparent;
    border-radius: 0.35rem;
    background: transparent;
  }
  .adjust:hover:not(:disabled) {
    border-color: #3a3a42;
    background: #26262c;
  }
  .adj-badge {
    display: inline-block;
    margin-right: 0.25rem;
    padding: 0 0.3rem;
    border-radius: 0.6rem;
    font-size: 0.75rem;
    font-variant-numeric: tabular-nums;
    background: #2b2b31;
    color: #d0d0d6;
  }

  .adjustments-row td {
    background: #1b1b1f;
  }
  .adjustments-list {
    list-style: none;
    margin: 0 0 0.4rem;
    padding: 0;
    font-size: 0.85rem;
  }
  .adjustments-list li {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.1rem 0;
  }
  .adjustments-list .bonus {
    color: #3fb950;
  }
  .adjustments-list .malus {
    color: #f85149;
  }
  .adjustments-list .reason {
    color: #d0d0d6;
    flex: 1;
  }
  .remove-adj {
    padding: 0 0.3rem;
    font-size: 0.75rem;
    color: #f85149;
    border: none;
    background: transparent;
  }
  .adjustment-form {
    display: flex;
    gap: 0.4rem;
  }
  .adj-delta {
    width: 4.5rem;
  }
  .adj-reason {
    flex: 1;
  }
  .adj-delta,
  .adj-reason {
    padding: 0.2rem 0.35rem;
    border: 1px solid #3a3a42;
    border-radius: 0.35rem;
    background: #1b1b1f;
    color: inherit;
    font: inherit;
  }
  .add-adj {
    padding: 0.2rem 0.6rem;
    border: 1px solid #3a3a42;
    border-radius: 0.35rem;
    background: transparent;
    color: inherit;
    font: inherit;
  }
  .add-adj:hover:not(:disabled) {
    border-color: #4a4a8a;
  }
</style>
