<!-- The ranking order: which tie-breaks apply, and in what order. -->
<script lang="ts">
  import { _ } from "svelte-i18n";
  import { TIEBREAKS } from "../../types";
  import type { Tiebreak } from "../../types";
  import { tiebreakLabel, tiebreakTitle } from "../../tiebreaks";

  interface Props {
    tiebreaks: Tiebreak[];
    /** Whether the estimated-ELO criterion ranks anything (see the parent). */
    estEloRanks: boolean;
    /** Board wins only means anything when a match and its boards differ. */
    teamMode: boolean;
    busy: boolean;
    persist: () => void;
  }

  let { tiebreaks = $bindable(), estEloRanks, teamMode, busy, persist }: Props = $props();

  // Metrics not yet in the ranking order — the choices for the "add" dropdown.
  // Two are conditional. Estimated ELO is only meaningful as a ranking criterion
  // when the estimate is maintained *and* applies to rated players (otherwise it
  // just sits at the registration rating). Board wins only means anything when a
  // match and its boards are different things, which is team mode; for an
  // individual it is their own wins, already carried by Points.
  const availableTiebreaks = $derived(
    TIEBREAKS.filter(
      (t) =>
        !tiebreaks.includes(t.code) &&
        (estEloRanks || t.code !== "est_elo") &&
        (teamMode || t.code !== "board_wins"),
    ),
  );
  const labelOf = (code: Tiebreak) => tiebreakLabel(code, $_);
  const titleOf = (code: Tiebreak) => tiebreakTitle(code, $_);

  function addTiebreak(code: Tiebreak) {
    if (!code || tiebreaks.includes(code)) return;
    tiebreaks.push(code);
    persist();
  }

  function removeTiebreak(i: number) {
    tiebreaks.splice(i, 1);
    persist();
  }

  function moveTiebreak(i: number, delta: number) {
    const j = i + delta;
    if (j < 0 || j >= tiebreaks.length) return;
    [tiebreaks[i], tiebreaks[j]] = [tiebreaks[j], tiebreaks[i]];
    persist();
  }

  // Drag-and-drop reordering — an alternative to the ▲▼ buttons above, not a
  // replacement (buttons stay for keyboard/accessibility).
  let dragIndex = $state<number | null>(null);
  let dragOverIndex = $state<number | null>(null);

  function reorderTiebreak(from: number, to: number) {
    if (from === to) return;
    const [item] = tiebreaks.splice(from, 1);
    tiebreaks.splice(to, 0, item);
    persist();
  }
</script>

<section class="section">
  <h3>{$_("settings.rankingTitle")}</h3>
  <p class="desc">
    {$_("settings.rankingDesc")}
  </p>

  <div class="thresholds">
    {#each tiebreaks as code, i (code)}
      <div
        class="threshold-row"
        class:dragging={dragIndex === i}
        class:drag-over={dragOverIndex === i && dragIndex !== null && dragIndex !== i}
        role="listitem"
        draggable={!busy}
        ondragstart={(e) => {
          dragIndex = i;
          e.dataTransfer?.setData("text/plain", String(i));
          if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
        }}
        ondragover={(e) => {
          e.preventDefault();
          dragOverIndex = i;
        }}
        ondragleave={() => {
          if (dragOverIndex === i) dragOverIndex = null;
        }}
        ondrop={(e) => {
          e.preventDefault();
          if (dragIndex !== null) reorderTiebreak(dragIndex, i);
          dragIndex = null;
          dragOverIndex = null;
        }}
        ondragend={() => {
          dragIndex = null;
          dragOverIndex = null;
        }}
      >
        <span class="drag-handle" title={$_("settings.dragToReorder")} aria-hidden="true"
          >⠿</span
        >
        <span class="tb-rank">{i + 1}.</span>
        <span class="tb-label" title={titleOf(code)}>{labelOf(code)}</span>
        <button
          type="button"
          class="remove"
          disabled={busy || i === 0}
          title={$_("settings.moveUp")}
          onclick={() => moveTiebreak(i, -1)}>▲</button
        >
        <button
          type="button"
          class="remove"
          disabled={busy || i === tiebreaks.length - 1}
          title={$_("settings.moveDown")}
          onclick={() => moveTiebreak(i, 1)}>▼</button
        >
        <button
          type="button"
          class="remove"
          disabled={busy}
          title={$_("settings.removeTiebreak")}
          onclick={() => removeTiebreak(i)}>✕</button
        >
      </div>
    {/each}
    {#if tiebreaks.length === 0}
      <p class="muted">
        {$_("settings.noTiebreaks")}
      </p>
    {/if}
    {#if availableTiebreaks.length > 0}
      <div class="threshold-row tb-add">
        <select
          class="tb-select control-sm control-quiet"
          disabled={busy}
          value=""
          onchange={(e) => {
            addTiebreak(e.currentTarget.value as Tiebreak);
            e.currentTarget.value = "";
          }}
        >
          <option value="" disabled>{$_("settings.addTiebreakPlaceholder")}</option>
          {#each availableTiebreaks as t (t.code)}
            <option value={t.code} title={tiebreakTitle(t.code, $_)}
              >{tiebreakLabel(t.code, $_)} — {tiebreakTitle(t.code, $_)}</option
            >
          {/each}
        </select>
      </div>
    {/if}
  </div>
</section>

<style>
  .threshold-row.dragging {
    opacity: 0.4;
  }
  .threshold-row.drag-over {
    box-shadow: 0 -2px 0 0 var(--border-accent-strong);
  }
  .drag-handle {
    cursor: grab;
    color: var(--text-muted);
    font-size: 0.9rem;
    line-height: 1;
  }
  .tb-rank {
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
    width: 1.4rem;
    text-align: right;
  }
  .tb-label {
    min-width: 5.5rem;
    font-weight: 600;
    color: var(--text-strong);
  }
  /* The "add a tie-break" picker: its options carry the full description (DC's is
     very long). Without a definite width the select's flex-basis is its
     max-content, which stretches the row past the page. A definite width pins it;
     min-width:0 lets it shrink. The open option list still shows each description
     in full. Scoped to its own row so the compact ELO-knob selects keep their
     content width. */
  .tb-add {
    align-self: stretch;
  }
  .tb-add .tb-select {
    min-width: 0;
    width: 100%;
    max-width: 40rem;
  }
</style>
