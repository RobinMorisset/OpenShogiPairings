<script lang="ts">
  // The referee's end of the result sheets (see `lib/resultSheets.ts`): the
  // round-tab button and the two things only the referee knows — how many rounds
  // the tournament is planned to have, and how many blank slips to keep for
  // players who turn up after it started.
  //
  // It lives in the round tabs, which only exist once registration is finalized,
  // so the slips always have a tournament number to print.
  import { _ } from "svelte-i18n";
  import {
    DEFAULT_ROUNDS,
    MAX_BLANK_SHEETS,
    MAX_ROUNDS,
    sheetsPerPage,
  } from "../resultSheets";

  interface Props {
    /** How many players get a filled-in slip. */
    playerCount: number;
    /** Whether the slips carry a MacMahon row — one more row per slip, which can
     *  tip a page from six slips down to four. */
    macMahonRow: boolean;
    onPrint: (rounds: number, blanks: number) => void;
    busy?: boolean;
  }

  let { playerCount, macMahonRow, onPrint, busy = false }: Props = $props();

  let open = $state(false);
  // Nothing in the tournament data records a planned round count, so the referee
  // is asked each time, starting from the commonest length.
  let rounds = $state<number | null>(DEFAULT_ROUNDS);
  let blanks = $state<number | null>(0);

  // `bind:value` on a number input gives `null` for an empty or unparsable
  // field, so this rejects that as well as out-of-range counts. The Print button
  // stays disabled rather than quietly printing a clamped number of rows.
  const valid = $derived(
    rounds !== null &&
      Number.isInteger(rounds) &&
      rounds >= 1 &&
      rounds <= MAX_ROUNDS &&
      blanks !== null &&
      Number.isInteger(blanks) &&
      blanks >= 0 &&
      blanks <= MAX_BLANK_SHEETS,
  );

  const perPage = $derived(sheetsPerPage((rounds ?? 0) + (macMahonRow ? 1 : 0)));
  const slips = $derived(playerCount + (blanks ?? 0));
  const pages = $derived(Math.ceil(slips / perPage));

  function print() {
    if (!valid) return;
    open = false;
    onPrint(rounds!, blanks!);
  }
</script>

<div class="sheets">
  <button
    type="button"
    class="ghost"
    class:active={open}
    data-testid="print-result-sheets"
    aria-expanded={open}
    title={$_("resultSheets.buttonTitle")}
    onclick={() => (open = !open)}
  >
    🖨 {$_("resultSheets.button")}
  </button>
  {#if open}
    <div class="panel">
      <label class="field">
        <span title={$_("resultSheets.roundsTitle")}>{$_("resultSheets.rounds")}</span>
        <input
          class="control-sm control-quiet"
          type="number"
          min="1"
          max={MAX_ROUNDS}
          data-testid="result-sheets-rounds"
          bind:value={rounds}
        />
      </label>
      <label class="field">
        <span title={$_("resultSheets.blanksTitle")}>{$_("resultSheets.blanks")}</span>
        <input
          class="control-sm control-quiet"
          type="number"
          min="0"
          max={MAX_BLANK_SHEETS}
          data-testid="result-sheets-blanks"
          bind:value={blanks}
        />
      </label>
      {#if valid}
        <p class="hint">
          {$_("resultSheets.pagesHint", { values: { slips, pages, perPage } })}
        </p>
      {/if}
      <div class="actions">
        <button type="button" class="ghost" onclick={() => (open = false)}>
          {$_("resultSheets.cancel")}
        </button>
        <button type="button" disabled={!valid || busy} onclick={print}>
          {$_("resultSheets.print")}
        </button>
      </div>
    </div>
  {/if}
</div>

<style>
  .sheets {
    position: relative;
  }
  .panel {
    position: absolute;
    z-index: 900;
    top: calc(100% + 0.2rem);
    /* The toolbars this sits in are right-aligned, so the panel hangs inward
       rather than off the edge of the card. */
    right: 0;
    width: 15rem;
    padding: 0.6rem 0.7rem;
    border: 1px solid var(--border-divider);
    border-radius: 0.5rem;
    background: var(--bg-surface);
    box-shadow: 0 8px 24px var(--shadow-dropdown);
  }
  .field {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.5rem;
    margin-bottom: 0.4rem;
    font-size: 0.9rem;
  }
  /* Raised: this popover sits above the page, not in a form on it. */
  .field input {
    width: 4.5rem;
    background: var(--bg-raised);
  }
  .hint {
    margin: 0 0 0.5rem;
    color: var(--text-secondary);
    font-size: 0.8rem;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.4rem;
  }
  .ghost.active {
    border-color: var(--border-accent);
  }
</style>
