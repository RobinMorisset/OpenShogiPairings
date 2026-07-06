<script lang="ts">
  import { untrack } from "svelte";
  import type { TournamentSettings } from "../types";

  interface Props {
    settings: TournamentSettings;
    /** Registration already finalized — edits here get a warning. */
    finalized: boolean;
    onUpdate: (settings: TournamentSettings) => void;
    busy?: boolean;
  }

  let { settings, finalized, onUpdate, busy = false }: Props = $props();

  // Keep only positive integers, then sort ascending — the server's canonical
  // order, used to compare our local state against what it stored.
  function cleanSorted(arr: number[]): number[] {
    return arr
      .filter((v) => Number.isFinite(v) && v >= 1)
      .map((v) => Math.round(v))
      .sort((a, b) => a - b);
  }

  function eq(a: number[], b: number[]): boolean {
    return a.length === b.length && a.every((v, i) => v === b[i]);
  }

  // Local editable rows, kept in *entry* order (not sorted) so the row a referee
  // is editing never jumps or shows a stale value. The inputs bind to these.
  let thresholds = $state<number[]>([]);
  let removals = $state<number[]>([]);

  // Adopt the persisted settings only on a genuine external change — a load, an
  // undo, or the server de-duplicating/capping our input. When our own edit merely
  // round-trips (the server just sorts it), our sorted local state already matches
  // what it stored, so we keep the entry order rather than reshuffling under the
  // cursor. `untrack` makes this fire on `settings` changes only, not our writes.
  $effect(() => {
    const serverThresholds = settings.macmahon_thresholds;
    const serverRemovals = settings.macmahon_removals;
    untrack(() => {
      if (
        !eq(cleanSorted(thresholds), serverThresholds) ||
        !eq(cleanSorted(removals), serverRemovals)
      ) {
        thresholds = [...serverThresholds];
        removals = [...serverRemovals];
      }
    });
  });

  function persist() {
    // Send the current rows; the server sorts, de-duplicates and caps the
    // removals to the threshold count.
    onUpdate({
      macmahon_thresholds: thresholds
        .filter((v) => Number.isFinite(v) && v >= 1)
        .map((v) => Math.round(v)),
      macmahon_removals: removals
        .filter((v) => Number.isFinite(v) && v >= 1)
        .map((v) => Math.round(v)),
    });
  }

  function addThreshold() {
    thresholds.push(thresholds.length ? Math.max(...thresholds) + 100 : 1500);
    persist();
  }

  function removeThreshold(i: number) {
    thresholds.splice(i, 1);
    persist();
  }

  function editThreshold(i: number, raw: string) {
    thresholds[i] = Number(raw);
    persist();
  }

  function addRemoval() {
    // Default to the round of the last removal (so repeated clicks stack drops on
    // the same round), or round 1 for the first one.
    removals.push(removals.length ? Math.max(...removals) : 1);
    persist();
  }

  function removeRemoval(i: number) {
    removals.splice(i, 1);
    persist();
  }

  function editRemoval(i: number, raw: string) {
    removals[i] = Number(raw);
    persist();
  }

  // Normalized (sorted, de-duplicated, capped) view of the local rows — drives the
  // previews so they always reflect what the server will store, updated live on
  // each edit rather than lagging a round-trip.
  const normThresholds = $derived.by(() => {
    const sorted = cleanSorted(thresholds);
    return sorted.filter((v, i) => i === 0 || v !== sorted[i - 1]);
  });
  const normRemovals = $derived(cleanSorted(removals).slice(0, normThresholds.length));

  // Can't schedule more removals than there are (distinct) thresholds to drop.
  const canAddRemoval = $derived(removals.length < normThresholds.length);

  // Preview of the resulting bands, e.g. "below 1200 → 0".
  const bands = $derived.by(() => {
    const t = normThresholds;
    const rows: { label: string; points: number }[] = [];
    if (t.length === 0) return rows;
    rows.push({ label: `below ${t[0]}`, points: 0 });
    for (let i = 0; i < t.length; i++) {
      const hi = t[i + 1];
      rows.push({
        label: hi != null ? `${t[i]}–${hi - 1}` : `${t[i]} and above`,
        points: i + 1,
      });
    }
    return rows;
  });

  // Preview of how the starting-point spread shrinks over the rounds, driven by
  // the normalized removal schedule.
  const schedule = $derived.by(() => {
    const total = normThresholds.length;
    const rem = normRemovals;
    if (total === 0 || rem.length === 0) return [];
    const counts = new Map<number, number>();
    for (const r of rem) counts.set(r, (counts.get(r) ?? 0) + 1);
    const rounds = [...counts.keys()].sort((a, b) => a - b);
    const rows: { label: string; max: number }[] = [];
    let active = total;
    let start = 1;
    for (const r of rounds) {
      rows.push({ label: start === r ? `round ${r}` : `rounds ${start}–${r}`, max: active });
      active = Math.max(0, active - (counts.get(r) ?? 0));
      start = r + 1;
    }
    rows.push({ label: `round ${start}+`, max: active });
    return rows;
  });
</script>

<div class="settings">
  {#if finalized}
    <p class="hint warning">
      ⚠ Registration is finalized — changing the MacMahon groups will change
      everyone's points and future pairings.
    </p>
  {/if}

  <h3>MacMahon groups</h3>
  <p class="desc">
    Each player starts with one point per ELO threshold their rating reaches. For
    example, with thresholds 1200 and 1700 a player rated below 1200 starts at 0,
    from 1200 to 1699 at 1, and 1700 or above at 2. Unrated players start at 0.
    Leave the list empty to disable MacMahon.
  </p>

  <div class="thresholds">
    {#each thresholds as v, i (i)}
      <div class="threshold-row">
        <input
          type="number"
          min="1"
          step="1"
          class="threshold"
          value={v}
          disabled={busy}
          onchange={(e) => editThreshold(i, e.currentTarget.value)}
        />
        <button
          type="button"
          class="remove"
          disabled={busy}
          title="Remove this threshold"
          onclick={() => removeThreshold(i)}>✕</button
        >
      </div>
    {/each}
    {#if thresholds.length === 0}
      <p class="muted">No thresholds — every player starts at 0 MacMahon points.</p>
    {/if}
    <button
      type="button"
      class="ghost small"
      disabled={busy}
      onclick={addThreshold}>Add threshold</button
    >
  </div>

  {#if bands.length > 0}
    <div class="preview">
      <h4>Starting points</h4>
      <ul>
        {#each bands as b (b.points)}
          <li><span class="band">{b.label}</span> → <strong>{b.points}</strong></li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if thresholds.length > 0}
    <div class="section">
      <h3>Degressive MacMahon</h3>
      <p class="desc">
        Also called <em>accelerated Swiss</em>: drop the lowest MacMahon group at
        the end of a round, so the starting-point head start fades as the field
        converges. Schedule one entry per group to drop; two entries on the same
        round drop two groups at once. A drop scheduled after round N takes effect
        from round N+1.
      </p>

      <div class="thresholds">
        {#each removals as r, i (i)}
          <div class="threshold-row">
            <span class="prefix">Drop one group after round</span>
            <input
              type="number"
              min="1"
              step="1"
              class="threshold narrow"
              value={r}
              disabled={busy}
              onchange={(e) => editRemoval(i, e.currentTarget.value)}
            />
            <button
              type="button"
              class="remove"
              disabled={busy}
              title="Remove this drop"
              onclick={() => removeRemoval(i)}>✕</button
            >
          </div>
        {/each}
        {#if removals.length === 0}
          <p class="muted">No drops — MacMahon groups stay fixed all tournament.</p>
        {/if}
        <button
          type="button"
          class="ghost small"
          disabled={busy || !canAddRemoval}
          title={canAddRemoval ? "" : "Can't drop more groups than there are thresholds"}
          onclick={addRemoval}>Add drop</button
        >
      </div>

      {#if schedule.length > 0}
        <div class="preview">
          <h4>Spread over rounds</h4>
          <ul>
            {#each schedule as s (s.label)}
              <li>
                <span class="band">{s.label}</span> → up to
                <strong>{s.max}</strong>
                starting {s.max === 1 ? "point" : "points"}
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .settings {
    max-width: 32rem;
  }
  h3 {
    margin: 0.4rem 0 0.3rem;
  }
  .section {
    margin-top: 1.75rem;
    border-top: 1px solid #2a2a30;
    padding-top: 1rem;
  }
  .desc {
    color: #9a9aa2;
    font-size: 0.85rem;
    margin: 0 0 1rem;
    line-height: 1.4;
  }
  .thresholds {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    align-items: flex-start;
  }
  .threshold-row {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .prefix {
    color: #c9c9d0;
    font-size: 0.9rem;
  }
  .threshold {
    width: 6rem;
    background: #1c1c22;
    color: inherit;
    border: 1px solid #3a3a42;
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
  }
  .threshold.narrow {
    width: 4rem;
  }
  .remove {
    background: transparent;
    border: 1px solid transparent;
    color: #9a9aa2;
    cursor: pointer;
    border-radius: 0.4rem;
    padding: 0.1rem 0.4rem;
  }
  .remove:hover:not(:disabled) {
    color: #f85149;
    border-color: #3a3a42;
  }
  .ghost {
    background: transparent;
    border: 1px solid #3a3a42;
    color: inherit;
    border-radius: 0.4rem;
    padding: 0.3rem 0.6rem;
    cursor: pointer;
    font: inherit;
  }
  .ghost.small {
    font-size: 0.85rem;
  }
  .ghost:hover:not(:disabled) {
    background: #26262c;
  }
  .ghost:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .preview {
    margin-top: 1.25rem;
  }
  .preview h4 {
    margin: 0 0 0.4rem;
    color: #9a9aa2;
    font-size: 0.8rem;
    font-weight: 600;
  }
  .preview ul {
    list-style: none;
    padding: 0;
    margin: 0;
    font-size: 0.9rem;
  }
  .preview li {
    padding: 0.15rem 0;
  }
  .band {
    color: #c9c9d0;
    font-variant-numeric: tabular-nums;
  }
  .muted {
    color: #9a9aa2;
    font-size: 0.9rem;
  }
  .hint.warning {
    color: #d29922;
    font-size: 0.85rem;
    margin: 0 0 1rem;
  }
</style>
