<script lang="ts">
  import type { DraftUpdate } from "../api";
  import type { Player, RoundDraft } from "../types";

  interface Props {
    draft: RoundDraft;
    players: Player[];
    /** Push the edited draft to the server. */
    onUpdate: (update: DraftUpdate) => void;
    /** Confirm the draft: pair remaining players and start the round. */
    onConfirm: () => void;
    busy?: boolean;
  }

  let { draft, players, onUpdate, onConfirm, busy = false }: Props = $props();

  const byId = $derived(new Map(players.map((p) => [p.id, p])));
  const absentSet = $derived(new Set(draft.absent));
  const forcedIds = $derived(
    new Set([
      ...draft.forced_boards.flatMap((b) => [b.player1, b.player2]),
      ...(draft.forced_bye ? [draft.forced_bye] : []),
    ]),
  );

  function byNumber(a: Player, b: Player) {
    return (a.tournament_id ?? Infinity) - (b.tournament_id ?? Infinity);
  }
  const allSorted = $derived([...players].sort(byNumber));
  const present = $derived(allSorted.filter((p) => !absentSet.has(p.id)));
  // Present players not already fixed into a forced pairing / bye.
  const forceable = $derived(present.filter((p) => !forcedIds.has(p.id)));

  function label(id: string): string {
    const p = byId.get(id);
    if (!p) return "(unknown)";
    const num = p.tournament_id != null ? `${p.tournament_id}. ` : "";
    return `${num}${p.last_name} ${p.first_name}`.trim();
  }

  /** The current draft as an update payload. */
  function base(): DraftUpdate {
    return {
      absent: [...draft.absent],
      forced_boards: draft.forced_boards.map((b) => ({
        player1: b.player1,
        player2: b.player2,
      })),
      forced_bye: draft.forced_bye ?? null,
    };
  }

  function toggleAbsent(id: string) {
    const willBeAbsent = !absentSet.has(id);
    const update = base();
    update.absent = willBeAbsent
      ? [...update.absent, id]
      : update.absent.filter((x) => x !== id);
    if (willBeAbsent) {
      // An absent player can't be in a forced pairing or the forced bye.
      update.forced_boards = update.forced_boards.filter(
        (b) => b.player1 !== id && b.player2 !== id,
      );
      if (update.forced_bye === id) update.forced_bye = null;
    }
    onUpdate(update);
  }

  // Transient selection for the "add forced pairing" controls.
  let pairA = $state("");
  let pairB = $state("");

  function addForcedPair() {
    if (!pairA || !pairB || pairA === pairB) return;
    const update = base();
    update.forced_boards = [...update.forced_boards, { player1: pairA, player2: pairB }];
    pairA = "";
    pairB = "";
    onUpdate(update);
  }

  function removeForcedPair(index: number) {
    const update = base();
    update.forced_boards = update.forced_boards.filter((_, i) => i !== index);
    onUpdate(update);
  }

  function setForcedBye(id: string) {
    const update = base();
    update.forced_bye = id || null;
    onUpdate(update);
  }

  // Client-side validation (the server validates authoritatively too).
  const problem = $derived.by<string | null>(() => {
    if (present.length < 2) return "Need at least 2 present players.";
    if (draft.forced_bye) {
      if (present.length % 2 === 0)
        return "A forced bye needs an odd number of present players.";
      if ((present.length - 2 * draft.forced_boards.length - 1) % 2 !== 0)
        return "The forced pairings and bye leave a player unpairable.";
    }
    return null;
  });
</script>

<div class="draft">
  <p class="summary">
    Preparing <strong>round {draft.number}</strong> — {present.length} present,
    {draft.absent.length} absent. Players you don't pair by hand are matched
    automatically.
  </p>

  <section>
    <h3>Absent this round</h3>
    <p class="muted small">Defaults to those absent last round.</p>
    <div class="players-grid">
      {#each allSorted as p (p.id)}
        <label class="chk">
          <input
            type="checkbox"
            checked={absentSet.has(p.id)}
            disabled={busy}
            onchange={() => toggleAbsent(p.id)}
          />
          {label(p.id)}
        </label>
      {/each}
    </div>
  </section>

  <section>
    <h3>Forced pairings</h3>
    {#if draft.forced_boards.length > 0}
      <ul class="forced-list">
        {#each draft.forced_boards as board, i (i)}
          <li>
            <span>{label(board.player1)} — {label(board.player2)}</span>
            <button
              type="button"
              class="remove"
              disabled={busy}
              onclick={() => removeForcedPair(i)}
              title="Remove this forced pairing">✕</button
            >
          </li>
        {/each}
      </ul>
    {/if}
    <div class="add-pair">
      <select bind:value={pairA} disabled={busy}>
        <option value="">Player…</option>
        {#each forceable as p (p.id)}
          <option value={p.id}>{label(p.id)}</option>
        {/each}
      </select>
      <span class="vs">vs</span>
      <select bind:value={pairB} disabled={busy}>
        <option value="">Player…</option>
        {#each forceable as p (p.id)}
          {#if p.id !== pairA}
            <option value={p.id}>{label(p.id)}</option>
          {/if}
        {/each}
      </select>
      <button
        type="button"
        class="ghost"
        disabled={busy || !pairA || !pairB || pairA === pairB}
        onclick={addForcedPair}>Force pairing</button
      >
    </div>
  </section>

  <section>
    <h3>Forced bye</h3>
    <p class="muted small">Only applies with an odd number of present players.</p>
    <select
      value={draft.forced_bye ?? ""}
      disabled={busy}
      onchange={(e) => setForcedBye(e.currentTarget.value)}
    >
      <option value="">Automatic (lowest of remaining)</option>
      {#each present as p (p.id)}
        {#if !forcedIds.has(p.id) || draft.forced_bye === p.id}
          <option value={p.id}>{label(p.id)}</option>
        {/if}
      {/each}
    </select>
  </section>

  <div class="confirm-row">
    {#if problem}
      <span class="problem">{problem}</span>
    {/if}
    <button
      type="button"
      class="ghost primary"
      disabled={busy || problem !== null}
      onclick={onConfirm}
    >
      Start round {draft.number}
    </button>
  </div>
  <p class="muted small">Use <strong>Undo</strong> to discard this draft.</p>
</div>

<style>
  .draft {
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
  }
  .summary {
    margin: 0;
    color: #c9c9d1;
  }
  section {
    border: 1px solid #2b2b31;
    border-radius: 0.6rem;
    padding: 0.75rem 1rem;
  }
  h3 {
    margin: 0 0 0.4rem;
    font-size: 0.95rem;
  }
  .muted {
    color: #9a9aa2;
  }
  .small {
    font-size: 0.8rem;
  }
  .players-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(12rem, 1fr));
    gap: 0.25rem 1rem;
    margin-top: 0.5rem;
  }
  .chk {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.9rem;
  }
  .forced-list {
    list-style: none;
    margin: 0 0 0.6rem;
    padding: 0;
    font-size: 0.9rem;
  }
  .forced-list li {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.2rem 0;
  }
  .add-pair {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  select {
    padding: 0.35rem 0.5rem;
    border: 1px solid #34343b;
    border-radius: 0.5rem;
    background: #1b1b1f;
    color: inherit;
    font: inherit;
  }
  .vs {
    color: #9a9aa2;
    font-size: 0.85rem;
  }
  .remove {
    padding: 0.1rem 0.4rem;
    color: #f85149;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 0.35rem;
    cursor: pointer;
  }
  .remove:hover:not(:disabled) {
    border-color: #f85149;
  }
  .confirm-row {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 1rem;
  }
  .problem {
    color: #d29922;
    font-size: 0.85rem;
  }
  .primary:not(:disabled) {
    border-color: #3b5bdb;
    background: #2b3a67;
    color: #cdd6f4;
  }
</style>
