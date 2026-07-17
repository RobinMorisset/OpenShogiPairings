<script lang="ts">
  import { _ } from "svelte-i18n";
  import type { DraftUpdate } from "../api";
  import type { Player, RoundDraft } from "../types";

  interface Props {
    draft: RoundDraft;
    players: Player[];
    /** Players the cup bracket will pair this round (not Swiss-customizable). */
    cupPlayers?: number[];
    /** Push the edited draft to the server. */
    onUpdate: (update: DraftUpdate) => void;
    /** Confirm the draft: pair remaining players and start the round. */
    onConfirm: () => void;
    busy?: boolean;
  }

  let {
    draft,
    players,
    cupPlayers = [],
    onUpdate,
    onConfirm,
    busy = false,
  }: Props = $props();

  // Round drafting only happens once registration is finalized (or for a
  // player added afterwards, which gets a number immediately on registration),
  // so every player here is guaranteed a `tournament_id`.
  function tid(p: Player): number {
    return p.tournament_id!;
  }

  const byId = $derived(new Map(players.map((p) => [tid(p), p])));
  const absentSet = $derived(new Set(draft.absent));
  const cupSet = $derived(new Set(cupPlayers));
  const forcedIds = $derived(
    new Set([
      ...draft.forced_boards.flatMap((b) => [b.player1, b.player2]),
      ...draft.forced_byes,
    ]),
  );

  function byNumber(a: Player, b: Player) {
    return (a.tournament_id ?? Infinity) - (b.tournament_id ?? Infinity);
  }
  const allSorted = $derived([...players].sort(byNumber));
  // The Swiss pool: present players the cup hasn't already taken.
  const present = $derived(
    allSorted.filter((p) => !absentSet.has(tid(p)) && !cupSet.has(tid(p))),
  );
  // Present players not already fixed into a forced pairing / bye.
  const forceable = $derived(present.filter((p) => !forcedIds.has(tid(p))));

  // Cup players the referee has marked absent — their bracket game is still
  // created, so the referee is warned to record the forfeit.
  const absentCupPlayers = $derived(
    allSorted.filter((p) => cupSet.has(tid(p)) && absentSet.has(tid(p))),
  );

  function label(id: number): string {
    const p = byId.get(id);
    if (!p) return "(unknown)";
    const num = p.tournament_id != null ? `${p.tournament_id}. ` : "";
    return `${num}${p.last_name} ${p.first_name}`.trim();
  }

  /** Parse a `<select>`'s raw string value into a tournament number, or `""`
   *  for the empty/placeholder option. */
  function parseId(raw: string): number | "" {
    return raw === "" ? "" : Number(raw);
  }

  /** The current draft as an update payload. */
  function base(): DraftUpdate {
    return {
      absent: [...draft.absent],
      forced_boards: draft.forced_boards.map((b) => ({
        player1: b.player1,
        player2: b.player2,
      })),
      forced_byes: [...draft.forced_byes],
    };
  }

  function toggleAbsent(id: number) {
    const willBeAbsent = !absentSet.has(id);
    const update = base();
    update.absent = willBeAbsent
      ? [...update.absent, id]
      : update.absent.filter((x) => x !== id);
    if (willBeAbsent) {
      // An absent player can't be in a forced pairing or on a forced bye.
      update.forced_boards = update.forced_boards.filter(
        (b) => b.player1 !== id && b.player2 !== id,
      );
      update.forced_byes = update.forced_byes.filter((x) => x !== id);
    }
    onUpdate(update);
  }

  // Transient selection for the "add forced pairing" controls. As soon as
  // both are picked, the pairing is forced immediately — no separate button.
  let pairA = $state<number | "">("");
  let pairB = $state<number | "">("");

  function addForcedPair() {
    if (pairA === "" || pairB === "" || pairA === pairB) return;
    const update = base();
    update.forced_boards = [...update.forced_boards, { player1: pairA, player2: pairB }];
    pairA = "";
    pairB = "";
    onUpdate(update);
  }

  function selectPairA(id: number | "") {
    pairA = id;
    addForcedPair();
  }

  function selectPairB(id: number | "") {
    pairB = id;
    addForcedPair();
  }

  function removeForcedPair(index: number) {
    const update = base();
    update.forced_boards = update.forced_boards.filter((_, i) => i !== index);
    onUpdate(update);
  }

  // An even field pairs up exactly, so it needs no bye and this section is shut:
  // the round would have to bye *two* players to stay pairable, which is not what
  // a referee reaching for "forced bye" means. To sit someone out of an even
  // field, mark them absent instead and set what the round scored them from the
  // standings. (The engine itself is happy either way — that's what lets the
  // importers rebuild a round with several byes.)
  const byesClosed = $derived(present.length % 2 === 0);

  function addForcedBye(id: number | "") {
    if (id === "" || draft.forced_byes.includes(id)) return;
    const update = base();
    update.forced_byes = [...update.forced_byes, id];
    onUpdate(update);
  }

  // Removing stays possible even while the section is shut: marking someone
  // absent can flip the field to even *after* a bye was forced, and the referee
  // must be able to take it back.
  function removeForcedBye(id: number) {
    const update = base();
    update.forced_byes = update.forced_byes.filter((x) => x !== id);
    onUpdate(update);
  }

  // Client-side validation (the server validates authoritatively too). The forced
  // byes need no parity check: whatever they leave over, the engine byes one more
  // player if the count is odd.
  const problem = $derived.by<string | null>(() => {
    if (cupPlayers.length === 0 && present.length < 2)
      return $_("roundDraftView.needAtLeastTwoPresent");
    return null;
  });
</script>

<div class="draft">
  <p class="summary">
    {#if cupPlayers.length > 0}
      {$_("roundDraftView.summaryWithCup", { values: { number: draft.number, present: present.length, cup: cupPlayers.length, absent: draft.absent.length } })}
    {:else}
      {$_("roundDraftView.summary", { values: { number: draft.number, present: present.length, absent: draft.absent.length } })}
    {/if}
  </p>

  {#if cupPlayers.length > 0}
    <p class="cup-note">
      {$_("roundDraftView.cupNote", { values: { count: cupPlayers.length } })}
    </p>
  {/if}

  {#if absentCupPlayers.length > 0}
    <p class="hint warning">
      ⚠ {$_(
        absentCupPlayers.length === 1
          ? "roundDraftView.absentCupWarningSingular"
          : "roundDraftView.absentCupWarningPlural",
        { values: { names: absentCupPlayers.map((p) => label(tid(p))).join(", ") } },
      )}
    </p>
  {/if}

  <section>
    <h3>{$_("roundDraftView.absentThisRound")}</h3>
    <p class="muted small">{$_("roundDraftView.absentDefaultHint")}</p>
    <div class="players-grid">
      {#each allSorted as p (p.id)}
        <label class="chk">
          <input
            type="checkbox"
            checked={absentSet.has(tid(p))}
            disabled={busy}
            onchange={() => toggleAbsent(tid(p))}
          />
          {label(tid(p))}{#if cupSet.has(tid(p))}<span class="cup-tag">{$_("roundDraftView.cupTag")}</span>{/if}
        </label>
      {/each}
    </div>
  </section>

  <section>
    <h3>{$_("roundDraftView.forcedPairings")}</h3>
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
              title={$_("roundDraftView.removeForcedPairing")}>✕</button
            >
          </li>
        {/each}
      </ul>
    {/if}
    <div class="add-pair">
      <select
        value={pairA}
        disabled={busy}
        onchange={(e) => selectPairA(parseId(e.currentTarget.value))}
      >
        <option value="">{$_("roundDraftView.playerEllipsis")}</option>
        {#each forceable as p (p.id)}
          <option value={tid(p)}>{label(tid(p))}</option>
        {/each}
      </select>
      <span class="vs">{$_("roundDraftView.vs")}</span>
      <select
        value={pairB}
        disabled={busy}
        onchange={(e) => selectPairB(parseId(e.currentTarget.value))}
      >
        <option value="">{$_("roundDraftView.playerEllipsis")}</option>
        {#each forceable as p (p.id)}
          {#if tid(p) !== pairA}
            <option value={tid(p)}>{label(tid(p))}</option>
          {/if}
        {/each}
      </select>
    </div>
  </section>

  <section class:disabled={byesClosed && draft.forced_byes.length === 0}>
    <h3>{$_("roundDraftView.forcedBye")}</h3>
    <p class="muted small">{$_("roundDraftView.forcedByeHint")}</p>
    {#if draft.forced_byes.length > 0}
      <ul class="forced-list">
        {#each draft.forced_byes as id (id)}
          <li>
            <span>{label(id)}</span>
            <button
              type="button"
              class="remove"
              disabled={busy}
              onclick={() => removeForcedBye(id)}
              title={$_("roundDraftView.removeForcedBye")}>✕</button
            >
          </li>
        {/each}
      </ul>
    {/if}
    {#if byesClosed && draft.forced_byes.length > 0}
      <p class="hint warning">⚠ {$_("roundDraftView.forcedByeEvenWarning")}</p>
    {/if}
    <select
      value=""
      disabled={busy || byesClosed || forceable.length === 0}
      onchange={(e) => addForcedBye(parseId(e.currentTarget.value))}
    >
      <option value="">{$_("roundDraftView.automaticBye")}</option>
      {#each forceable as p (p.id)}
        <option value={tid(p)}>{label(tid(p))}</option>
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
      data-testid="confirm-round"
      disabled={busy || problem !== null}
      onclick={onConfirm}
    >
      {$_("roundDraftView.startRound", { values: { number: draft.number } })}
    </button>
  </div>
  <p class="muted small">
    {$_("roundDraftView.useUndoHintPrefix")} <strong>{$_("app.undo")}</strong>{$_(
      "roundDraftView.useUndoHintSuffix",
    )}
  </p>
</div>

<style>
  .draft {
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
  }
  .summary {
    margin: 0;
    color: var(--text-strong);
  }
  .cup-note {
    margin: 0;
    color: var(--text-secondary);
    font-size: 0.85rem;
  }
  .hint.warning {
    margin: 0;
    color: var(--color-warning);
    font-size: 0.85rem;
    line-height: 1.4;
  }
  .cup-tag {
    margin-left: 0.35rem;
    padding: 0 0.3rem;
    border-radius: 0.6rem;
    font-size: 0.68rem;
    font-weight: 600;
    color: var(--color-warning-strong);
    border: 1px solid var(--border-warning);
    background: var(--bg-warning);
  }
  section {
    border: 1px solid var(--border-divider);
    border-radius: 0.6rem;
    padding: 0.75rem 1rem;
  }
  section.disabled {
    opacity: 0.45;
  }
  h3 {
    margin: 0 0 0.4rem;
    font-size: 0.95rem;
  }
  .muted {
    color: var(--text-secondary);
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
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-inset);
    color: inherit;
    font: inherit;
  }
  .vs {
    color: var(--text-secondary);
    font-size: 0.85rem;
  }
  .remove {
    padding: 0.1rem 0.4rem;
    color: var(--color-danger);
    background: transparent;
    border: 1px solid transparent;
    border-radius: 0.35rem;
    cursor: pointer;
  }
  .remove:hover:not(:disabled) {
    border-color: var(--color-danger);
  }
  .confirm-row {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 1rem;
  }
  .problem {
    color: var(--color-warning);
    font-size: 0.85rem;
  }
  .primary:not(:disabled) {
    border-color: var(--border-accent-strong);
    background: var(--bg-accent);
    color: var(--text-on-accent);
  }
</style>
