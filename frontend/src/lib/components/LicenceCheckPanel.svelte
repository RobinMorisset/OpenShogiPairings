<!--
  Who of one nationality has not paid their federation licence.

  The referee picks a nationality and a CSV list of the players whose licence is
  up to date (as exported from the federation's back office), and gets back the
  registered players of that nationality the list doesn't carry. Read-only: this
  reports, it never edits the roster — what to do about someone who hasn't paid
  is the referee's call, not the software's.

  The comparison itself runs server-side (`osp-core`'s `check_licences`), on the
  same parser as a CSV roster import, so the two agree on columns, delimiters and
  accent folding. This component only picks the inputs and renders the answer.
-->
<script lang="ts">
  import { _ } from "svelte-i18n";
  import { registeredNationalities, withoutNationality } from "../nationalities";
  import type { LicenceCheck, Player } from "../types";

  interface Props {
    /** The registered players, for the nationality list and the names shown. */
    players: Player[];
    /**
     * The last check's answer together with the nationality it was run for.
     * Kept as a pair so an answer can never be shown under another nationality's
     * heading — a stale one is dropped instead (see `shown`).
     */
    result: { nationality: string; check: LicenceCheck } | null;
    /** Ask for a licence list (a file picker) and check `nationality` against it. */
    onCheck: (nationality: string) => void;
    /** True while a request is in flight. */
    busy?: boolean;
  }

  let { players, result, onCheck, busy = false }: Props = $props();

  // The nationalities to offer (see `nationalities.ts` for why they come from
  // the roster rather than a text field).
  const nationalities = $derived(registeredNationalities(players));

  // Registered players the check cannot cover, whatever is picked: with no
  // nationality of their own they belong to no list. Said out loud next to the
  // answer, because a roster where half the field has a blank nationality would
  // otherwise come back reassuringly clean.
  const unchecked = $derived(withoutNationality(players));

  let nationality = $state("");

  // Keep the selection meaningful as the roster changes: drop one whose last
  // player just left, and save a click when there is only one to pick.
  $effect(() => {
    if (nationalities.length === 1) {
      nationality = nationalities[0][0];
    } else if (nationality && !nationalities.some(([nat]) => nat === nationality)) {
      nationality = "";
    }
  });

  // Only ever show an answer that belongs to what is selected now.
  const shown = $derived(result && result.nationality === nationality ? result.check : null);

  // An answer describes the roster it was run against. If that roster has since
  // gained or lost a player of this nationality — a late registration, a
  // withdrawal, a nationality typed in afterwards — the answer is not about the
  // field any more, and a stale "everyone is on the list" is precisely the
  // reassurance nobody should be given. So it is dropped rather than shown
  // qualified, and the check has to be re-run.
  const registeredNow = $derived(nationalities.find(([nat]) => nat === nationality)?.[1] ?? 0);

  // Each missing player, resolved against the roster being rendered.
  const missing = $derived.by(() => {
    if (!shown) return [];
    const byId = new Map(players.map((p) => [p.id, p]));
    return shown.missing.flatMap((m) => {
      const player = byId.get(m.id);
      return player ? [{ player, nearMisses: m.near_misses }] : [];
    });
  });

  // An answer describes the roster it was run against. It is dropped, rather
  // than shown for what can still be made of it, when either half stops
  // matching: the count of registered players of this nationality has moved (a
  // late registration, a withdrawal, a nationality typed in afterwards), or a
  // reported player cannot be found in the roster at all.
  //
  // The second half is not paranoia about a passing race. Whatever the cause —
  // a player removed by another referee, or a server that does not answer in
  // the shape this build expects — quietly dropping what cannot be resolved
  // turns "three of them have not paid" into "everyone is on the list", which
  // is the one wrong answer this whole feature exists to avoid. A shortened
  // list is not an answer, so it is not shown as one.
  const stale = $derived(
    shown !== null &&
      (shown.checked !== registeredNow || missing.length !== shown.missing.length),
  );
</script>

<div class="licence-panel">
  <p class="small hint">{$_("licenceCheck.hint")}</p>

  {#if nationalities.length === 0}
    <p class="small">{$_("licenceCheck.noNationalities")}</p>
  {:else}
    <div class="controls">
      <label for="licence-nat">{$_("licenceCheck.nationality")}</label>
      <select id="licence-nat" bind:value={nationality} disabled={busy}>
        <option value="">{$_("licenceCheck.chooseNationality")}</option>
        {#each nationalities as [nat, count] (nat)}
          <option value={nat}>{nat} ({count})</option>
        {/each}
      </select>
      <button
        type="button"
        class="ghost small"
        data-testid="licence-load-list"
        disabled={busy || !nationality}
        onclick={() => onCheck(nationality)}
      >
        {$_("licenceCheck.loadList")}
      </button>
    </div>
  {/if}

  {#if stale}
    <p class="small warn">{$_("licenceCheck.rosterChanged")}</p>
  {:else if shown}
    <p class="small summary">
      {$_("licenceCheck.summary", {
        values: { nat: nationality, checked: shown.checked, listed: shown.listed },
      })}
      {#if unchecked > 0}
        <span class="warn">
          {$_("licenceCheck.unchecked", { values: { count: unchecked } })}
        </span>
      {/if}
    </p>
    {#if missing.length === 0}
      <p class="small covered">{$_("licenceCheck.allCovered", { values: { nat: nationality } })}</p>
    {:else}
      <p class="small missing-heading">{$_("licenceCheck.missingHeading")}</p>
      <ul class="missing" data-testid="licence-missing">
        {#each missing as { player, nearMisses } (player.id)}
          <li>
            <span class="name">{player.last_name} {player.first_name}</span>
            {#if player.club}<span class="club">{player.club}</span>{/if}
            {#if nearMisses.length > 0}
              <span class="near-miss">
                {$_("licenceCheck.nearMiss", { values: { names: nearMisses.join(", ") } })}
              </span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>

<style>
  .licence-panel {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.75rem;
    border: 1px solid var(--border-soft);
    border-radius: 0.5rem;
    background: var(--bg-inset);
  }
  .small {
    font-size: 0.8rem;
  }
  .hint {
    margin: 0;
    color: var(--text-secondary);
    max-width: 60rem;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  select {
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-inset);
    color: inherit;
    font: inherit;
  }
  .summary,
  .covered,
  .missing-heading {
    margin: 0;
  }
  .summary {
    color: var(--text-secondary);
  }
  .warn {
    color: var(--color-warning);
  }
  .covered {
    color: var(--color-success);
  }
  .missing-heading {
    font-weight: 600;
  }
  ul.missing {
    margin: 0;
    padding-left: 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }
  ul.missing .club {
    color: var(--text-secondary);
    font-size: 0.85em;
    margin-left: 0.5rem;
  }
  /* On its own line under the name: it is a second thought about that player,
     and it is long enough to push the club off the row otherwise. */
  ul.missing .near-miss {
    display: block;
    color: var(--color-warning);
    font-size: 0.85em;
  }
</style>
