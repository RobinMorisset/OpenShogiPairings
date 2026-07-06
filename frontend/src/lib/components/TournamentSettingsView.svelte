<script lang="ts">
  import type { TournamentSettings } from "../types";

  interface Props {
    settings: TournamentSettings;
    /** Registration already finalized — edits here get a warning. */
    finalized: boolean;
    onUpdate: (settings: TournamentSettings) => void;
    busy?: boolean;
  }

  let { settings, finalized, onUpdate, busy = false }: Props = $props();

  // Local editable copies, re-synced whenever the persisted settings change (e.g.
  // after the server sorts/de-dups the thresholds or caps the removals, or on undo).
  let thresholds = $state<number[]>([]);
  let removals = $state<number[]>([]);
  $effect(() => {
    thresholds = [...settings.macmahon_thresholds];
    removals = [...settings.macmahon_removals];
  });

  function persist(nextThresholds: number[], nextRemovals: number[]) {
    // Keep positive integers only; the server sorts, de-duplicates and caps the
    // removals to the threshold count.
    const cleanThresholds = nextThresholds
      .filter((v) => Number.isFinite(v) && v > 0)
      .map((v) => Math.round(v));
    const cleanRemovals = nextRemovals
      .filter((v) => Number.isFinite(v) && v >= 1)
      .map((v) => Math.round(v));
    onUpdate({
      macmahon_thresholds: cleanThresholds,
      macmahon_removals: cleanRemovals,
    });
  }

  function addThreshold() {
    const next = thresholds.length ? thresholds[thresholds.length - 1] + 100 : 1500;
    persist([...thresholds, next], removals);
  }

  function removeThreshold(i: number) {
    persist(
      thresholds.filter((_, j) => j !== i),
      removals,
    );
  }

  function editThreshold(i: number, raw: string) {
    const next = [...thresholds];
    next[i] = Number(raw);
    persist(next, removals);
  }

  function addRemoval() {
    // Default to the round of the last removal (so repeated clicks stack drops on
    // the same round), or round 1 for the first one.
    const next = removals.length ? Math.max(...removals) : 1;
    persist(thresholds, [...removals, next]);
  }

  function removeRemoval(i: number) {
    persist(
      thresholds,
      removals.filter((_, j) => j !== i),
    );
  }

  function editRemoval(i: number, raw: string) {
    const next = [...removals];
    next[i] = Number(raw);
    persist(thresholds, next);
  }

  // Can't schedule more removals than there are thresholds to drop.
  const canAddRemoval = $derived(removals.length < thresholds.length);

  // Preview of the resulting bands, e.g. "below 1200 → 0".
  const bands = $derived.by(() => {
    const t = settings.macmahon_thresholds;
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
  // the persisted (normalized) removal schedule.
  const schedule = $derived.by(() => {
    const total = settings.macmahon_thresholds.length;
    const rem = settings.macmahon_removals;
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
