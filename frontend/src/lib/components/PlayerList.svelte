<script lang="ts">
  import { _ } from "svelte-i18n";
  import type { NewPlayer, Player, PlayerCategory } from "../types";
  import { formatGrade, gradeRank, parseGrade } from "../grade";
  import { printPage } from "../platform";

  interface Props {
    players: Player[];
    /** Show the cup-eligibility column (when the hybrid cup is enabled). */
    showEligible?: boolean;
    /** Referee-defined categories — one checkbox column each. */
    categories?: PlayerCategory[];
    /** Registration already finalized — the cup bracket is frozen, so bulk
     * eligibility edits (only meaningful pre-finalization) are hidden. */
    finalized?: boolean;
    /** Commit an in-place edit of a player's fields. */
    onEdit: (id: string, player: NewPlayer) => void;
    onRemove: (id: string) => void;
    /** Toggle a player's cup eligibility. */
    onToggleEligible?: (id: string, eligible: boolean) => void;
    /** Set cup eligibility for every player of the given nationality. */
    onSetEligibleByNationality?: (nationality: string, eligible: boolean) => void;
    /** Toggle a player's membership in a category. */
    onToggleCategory?: (id: string, categoryId: string, member: boolean) => void;
    /** Apply a manual point bonus (positive delta) or malus (negative) to a player. */
    onAddAdjustment?: (id: string, delta: number, reason: string) => void;
    /** Remove a previously applied point adjustment. */
    onRemoveAdjustment?: (id: string, adjustmentId: string) => void;
    busy?: boolean;
  }

  let {
    players,
    showEligible = false,
    categories = [],
    finalized = false,
    onEdit,
    onRemove,
    onToggleEligible,
    onSetEligibleByNationality,
    onToggleCategory,
    onAddAdjustment,
    onRemoveAdjustment,
    busy = false,
  }: Props = $props();

  const eligibleCount = $derived(players.filter((p) => p.eligible).length);

  const CUP_SIZES = [8, 16, 32, 64];

  // Rank of each eligible player among all eligible players, using the same
  // order the backend uses to seed the cup at finalization (rating
  // descending, unrated last, ties by registration order — see
  // `finalize_registration_with` in tournament.rs). This is independent of
  // whatever column the table is currently sorted by, so the cutoff badges
  // stay meaningful no matter how the referee has the table sorted.
  const eligibleRank = $derived.by(() => {
    const order = players
      .map((p, i) => ({ p, i }))
      .sort((a, b) => {
        const ra = a.p.rating;
        const rb = b.p.rating;
        if (ra != null && rb != null && ra !== rb) return rb - ra;
        if (ra != null && rb == null) return -1;
        if (ra == null && rb != null) return 1;
        return a.i - b.i;
      });
    const ranks = new Map<string, number>();
    let rank = 0;
    for (const { p } of order) {
      if (p.eligible) ranks.set(p.id, ++rank);
    }
    return ranks;
  });

  /** The cup size this player's eligibility rank marks the cutoff for, if any. */
  function cupCutoff(player: Player): number | null {
    const rank = eligibleRank.get(player.id);
    if (rank == null) return null;
    return CUP_SIZES.includes(rank) ? rank : null;
  }

  // Distinct nationalities present (non-empty), with a per-nationality count,
  // for the bulk "mark eligible" control. Sorted alphabetically.
  const nationalities = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const p of players) {
      const nat = p.nationality?.trim();
      if (nat) counts.set(nat, (counts.get(nat) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  });

  let bulkNationality = $state("");

  // Keep the selection valid as the roster changes (e.g. the last player of a
  // nationality is removed).
  $effect(() => {
    if (bulkNationality && !nationalities.some(([nat]) => nat === bulkNationality)) {
      bulkNationality = "";
    }
  });

  function applyBulkEligible(eligible: boolean) {
    if (!bulkNationality) return;
    onSetEligibleByNationality?.(bulkNationality, eligible);
  }

  // #, last name, first name, rating, grade, nat., club, [categories…], [cup], actions.
  const colCount = $derived(7 + categories.length + (showEligible ? 1 : 0) + 1);

  type Field = "last_name" | "first_name" | "rating" | "grade" | "nationality" | "club";

  // Sortable columns, including ones that aren't inline-editable ("#" and the
  // cup checkbox). This is display only, purely client-side — it never touches
  // the server, and edits still key off `player.id`.
  type SortKey = "tournament_id" | Field | "eligible";

  let sortKey = $state<SortKey>("rating");
  // -1 = descending, 1 = ascending. Rating-descending is the default, matching
  // the previous fixed order.
  let sortDir = $state<-1 | 1>(-1);

  function sortValue(p: Player, key: SortKey): string | number | null {
    switch (key) {
      case "tournament_id":
        return p.tournament_id ?? null;
      case "last_name":
        return p.last_name.toLowerCase();
      case "first_name":
        return p.first_name.toLowerCase() || null;
      case "rating":
        return p.rating ?? null;
      case "grade":
        return p.grade ? gradeRank(p.grade) : null;
      case "nationality":
        return p.nationality?.trim().toLowerCase() || null;
      case "club":
        return p.club?.trim().toLowerCase() || null;
      case "eligible":
        return p.eligible ? 1 : 0;
    }
  }

  // Click a header to sort by that column; click again to flip direction.
  // Numeric-ish columns default to descending (strongest/most-recent first),
  // text columns to ascending (A→Z).
  function toggleSort(key: SortKey) {
    if (sortKey === key) {
      sortDir = sortDir === 1 ? -1 : 1;
      return;
    }
    sortKey = key;
    sortDir = key === "rating" || key === "grade" || key === "tournament_id" || key === "eligible" ? -1 : 1;
  }

  // Missing values (no rating, no tournament number, blank nationality/club)
  // always sort to the bottom, regardless of direction — a null isn't "low"
  // or "high", it's just absent. `sort` is stable, so ties keep registration
  // order.
  const sorted = $derived(
    [...players].sort((a, b) => {
      const av = sortValue(a, sortKey);
      const bv = sortValue(b, sortKey);
      if (av == null && bv == null) return 0;
      if (av == null) return 1;
      if (bv == null) return -1;
      const cmp = typeof av === "string" && typeof bv === "string" ? av.localeCompare(bv) : av < bv ? -1 : av > bv ? 1 : 0;
      return sortDir === 1 ? cmp : -cmp;
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
      case "grade":
        return p.grade ? formatGrade(p.grade) : "";
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
      grade: p.grade,
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
      case "grade":
        edited.grade = parseGrade(value) ?? undefined;
        break;
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

{#snippet sortHeader(key: SortKey, label: string, numeric: boolean, title?: string)}
  <th class:num={numeric} {title}>
    <button type="button" class="sort-btn" onclick={() => toggleSort(key)}>
      {label}
      <span class="sort-arrow" class:active={sortKey === key}>
        {sortKey === key ? (sortDir === 1 ? "▲" : "▼") : "⇅"}
      </span>
    </button>
  </th>
{/snippet}

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
      title={$_("playerList.clickToEdit")}
      onclick={() => startEdit(player, field)}
    >
      {displayValue(player, field) || "—"}
    </button>
  {/if}
{/snippet}

<div class="player-toolbar print-hide">
  <button type="button" class="ghost" onclick={() => printPage()}>🖨 {$_("roundView.print")}</button>
</div>
{#if showEligible}
  <p class="elig-count">{$_("playerList.eligibleCount", { values: { count: eligibleCount } })}</p>
  {#if !finalized && nationalities.length > 0}
    <div class="elig-bulk print-hide">
      <span>{$_("playerList.eligibilityByNationality")}</span>
      <select bind:value={bulkNationality} disabled={busy}>
        <option value="">{$_("playerList.chooseNationality")}</option>
        {#each nationalities as [nat, count] (nat)}
          <option value={nat}>{nat} ({count})</option>
        {/each}
      </select>
      <button
        type="button"
        class="ghost small"
        disabled={busy || !bulkNationality}
        onclick={() => applyBulkEligible(true)}
      >
        {$_("playerList.markEligible")}
      </button>
      <button
        type="button"
        class="ghost small"
        disabled={busy || !bulkNationality}
        onclick={() => applyBulkEligible(false)}
      >
        {$_("playerList.clear")}
      </button>
    </div>
  {/if}
{/if}
{#if players.length === 0}
  <p class="empty">{$_("playerList.noPlayers")}</p>
{:else}
  <table>
    <thead>
      <tr>
        {@render sortHeader("tournament_id", "#", true, $_("playerList.tournamentNumberTitle"))}
        {@render sortHeader("last_name", $_("playerList.lastName"), false)}
        {@render sortHeader("first_name", $_("playerList.firstName"), false)}
        {@render sortHeader("rating", $_("playerList.rating"), true)}
        {@render sortHeader("grade", $_("playerList.grade"), false)}
        {@render sortHeader("nationality", $_("playerList.nationality"), false)}
        {@render sortHeader("club", $_("playerList.club"), false)}
        {#each categories as cat (cat.id)}
          <th class="category" title={cat.name}>{cat.name}</th>
        {/each}
        {#if showEligible}
          <th class="elig">
            <button type="button" class="sort-btn" title={$_("playerList.eligibleForCup")} onclick={() => toggleSort("eligible")}>
              {$_("playerList.cup")}
              <span class="sort-arrow" class:active={sortKey === "eligible"}>
                {sortKey === "eligible" ? (sortDir === 1 ? "▲" : "▼") : "⇅"}
              </span>
            </button>
          </th>
        {/if}
        <th class="print-hide" aria-label={$_("playerList.actions")}></th>
      </tr>
    </thead>
    <tbody>
      {#each sorted as player (player.id)}
        <tr>
          <td class="num">{player.tournament_id ?? "—"}</td>
          <td>{@render cell(player, "last_name", false)}</td>
          <td>{@render cell(player, "first_name", false)}</td>
          <td class="num">{@render cell(player, "rating", true)}</td>
          <td>{@render cell(player, "grade", false)}</td>
          <td>{@render cell(player, "nationality", false)}</td>
          <td>{@render cell(player, "club", false)}</td>
          {#each categories as cat (cat.id)}
            <td class="category">
              <input
                type="checkbox"
                checked={(player.categories ?? []).includes(cat.id)}
                disabled={busy}
                title={cat.name}
                onchange={(e) => onToggleCategory?.(player.id, cat.id, e.currentTarget.checked)}
              />
            </td>
          {/each}
          {#if showEligible}
            <td class="elig">
              {#if finalized}
                <span title={$_("playerList.eligibleForCupFull")} class="elig-frozen">
                  {player.eligible ? "✓" : ""}
                </span>
              {:else}
                <span class="elig-cell">
                  <input
                    type="checkbox"
                    checked={player.eligible ?? false}
                    disabled={busy}
                    title={$_("playerList.eligibleForCupFull")}
                    onchange={(e) => onToggleEligible?.(player.id, e.currentTarget.checked)}
                  />
                  {#if cupCutoff(player) !== null}
                    <span
                      class="cup-cutoff"
                      title={$_("playerList.cupCutoffTitle", { values: { size: cupCutoff(player) } })}
                    >
                      {cupCutoff(player)}
                    </span>
                  {/if}
                </span>
              {/if}
            </td>
          {/if}
          <td class="actions print-hide">
            {#if adjustmentTotal(player) !== 0}
              <span class="adj-badge" title={adjustmentTitle(player)}>
                {adjustmentTotal(player) > 0 ? "+" : ""}{adjustmentTotal(player)}
              </span>
            {/if}
            <button
              type="button"
              class="adjust"
              title={$_("playerList.manualAdjustmentTitle")}
              disabled={busy}
              onclick={() => toggleAdjustments(player.id)}
            >
              ±
            </button>
            <button
              type="button"
              class="remove"
              title={$_("playerList.removePlayer")}
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
                        title={$_("playerList.removeAdjustment")}
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
                  placeholder={$_("playerList.adjustmentPointsPlaceholder")}
                  value={adjustmentDelta}
                  oninput={(e) => (adjustmentDelta = e.currentTarget.value)}
                />
                <input
                  class="adj-reason"
                  type="text"
                  placeholder={$_("playerList.adjustmentReasonPlaceholder")}
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
                  {$_("playerList.add")}
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
    color: var(--text-secondary);
    font-size: 0.9rem;
  }
  .player-toolbar {
    display: flex;
    justify-content: flex-end;
    margin-bottom: 0.5rem;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.9rem;
  }
  th,
  td {
    padding: 0.25rem 0.4rem;
    border-bottom: 1px solid var(--border-divider);
    text-align: left;
  }
  th {
    color: var(--text-secondary);
    font-weight: 600;
    font-size: 0.8rem;
    padding: 0.4rem;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .sort-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0;
    border: none;
    background: transparent;
    color: inherit;
    font: inherit;
    font-weight: 600;
    font-size: 0.8rem;
    cursor: pointer;
  }
  th.num .sort-btn {
    flex-direction: row-reverse;
  }
  .sort-btn:hover {
    color: var(--text-strong);
  }
  .sort-arrow {
    font-size: 0.7rem;
    color: var(--text-muted);
  }
  .sort-arrow.active {
    color: var(--color-accent);
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
  .category {
    text-align: center;
    white-space: nowrap;
  }
  th.elig {
    width: auto;
    white-space: nowrap;
  }
  th.elig .sort-btn {
    justify-content: center;
  }
  .elig-frozen {
    color: var(--color-success);
    font-weight: 700;
  }
  .elig-cell {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
  }
  .cup-cutoff {
    display: inline-block;
    padding: 0 0.3rem;
    border-radius: 0.6rem;
    font-size: 0.7rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    background: var(--color-accent);
    color: var(--text-on-accent);
  }
  .elig-count {
    color: var(--text-secondary);
    font-size: 0.82rem;
    margin: 0 0 0.5rem;
  }
  .elig-bulk {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0 0 0.75rem;
    font-size: 0.85rem;
    color: var(--text-strong);
  }
  .elig-bulk select {
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.35rem;
    padding: 0.2rem 0.35rem;
    font: inherit;
  }
  .ghost {
    background: transparent;
    border: 1px solid var(--border-soft);
    color: inherit;
    border-radius: 0.35rem;
    padding: 0.2rem 0.55rem;
    cursor: pointer;
    font: inherit;
  }
  .ghost.small {
    font-size: 0.8rem;
  }
  .ghost:hover:not(:disabled) {
    background: var(--bg-hover);
  }
  .ghost:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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
    border-color: var(--border-soft);
    background: var(--bg-hover);
  }
  .num .cell-btn {
    text-align: right;
  }
  .cell-input {
    width: 100%;
    box-sizing: border-box;
    padding: 0.2rem 0.35rem;
    border: 1px solid var(--border-accent);
    border-radius: 0.35rem;
    background: var(--bg-inset);
    color: inherit;
    font: inherit;
  }
  .num .cell-input {
    text-align: right;
  }

  .remove {
    padding: 0.1rem 0.4rem;
    font-size: 0.8rem;
    color: var(--color-danger);
    border-color: transparent;
    background: transparent;
  }
  .remove:hover:not(:disabled) {
    border-color: var(--color-danger);
  }

  .adjust {
    padding: 0.1rem 0.4rem;
    font-size: 0.8rem;
    color: var(--text-secondary);
    border: 1px solid transparent;
    border-radius: 0.35rem;
    background: transparent;
  }
  .adjust:hover:not(:disabled) {
    border-color: var(--border-soft);
    background: var(--bg-hover);
  }
  .adj-badge {
    display: inline-block;
    margin-right: 0.25rem;
    padding: 0 0.3rem;
    border-radius: 0.6rem;
    font-size: 0.75rem;
    font-variant-numeric: tabular-nums;
    background: var(--bg-chip);
    color: var(--text-strong);
  }

  .adjustments-row td {
    background: var(--bg-inset);
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
    color: var(--color-success);
  }
  .adjustments-list .malus {
    color: var(--color-danger);
  }
  .adjustments-list .reason {
    color: var(--text-strong);
    flex: 1;
  }
  .remove-adj {
    padding: 0 0.3rem;
    font-size: 0.75rem;
    color: var(--color-danger);
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
    border: 1px solid var(--border-soft);
    border-radius: 0.35rem;
    background: var(--bg-inset);
    color: inherit;
    font: inherit;
  }
  .add-adj {
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border-soft);
    border-radius: 0.35rem;
    background: transparent;
    color: inherit;
    font: inherit;
  }
  .add-adj:hover:not(:disabled) {
    border-color: var(--border-accent);
  }

  @media print {
    .print-hide {
      display: none;
    }
  }
</style>
