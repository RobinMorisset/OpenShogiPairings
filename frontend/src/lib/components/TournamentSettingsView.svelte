<script lang="ts">
  import type { TournamentSettings } from "../types";

  interface Props {
    settings: TournamentSettings;
    /** Registration already finalized — edits here get a warning. */
    finalized: boolean;
    onUpdate: (thresholds: number[]) => void;
    busy?: boolean;
  }

  let { settings, finalized, onUpdate, busy = false }: Props = $props();

  // Local editable copy, re-synced whenever the persisted settings change (e.g.
  // after the server sorts/de-dups the list, or on undo).
  let values = $state<number[]>([]);
  $effect(() => {
    values = [...settings.macmahon_thresholds];
  });

  function persist(next: number[]) {
    // Keep positive integers only; the server sorts and de-duplicates.
    const clean = next
      .filter((v) => Number.isFinite(v) && v > 0)
      .map((v) => Math.round(v));
    onUpdate(clean);
  }

  function addThreshold() {
    const next = values.length ? values[values.length - 1] + 100 : 1500;
    persist([...values, next]);
  }

  function removeThreshold(i: number) {
    persist(values.filter((_, j) => j !== i));
  }

  function editThreshold(i: number, raw: string) {
    const next = [...values];
    next[i] = Number(raw);
    persist(next);
  }

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
    {#each values as v, i (i)}
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
    {#if values.length === 0}
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
</div>

<style>
  .settings {
    max-width: 32rem;
  }
  h3 {
    margin: 0.4rem 0 0.3rem;
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
  .threshold {
    width: 6rem;
    background: #1c1c22;
    color: inherit;
    border: 1px solid #3a3a42;
    border-radius: 0.4rem;
    padding: 0.3rem 0.45rem;
    font: inherit;
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
