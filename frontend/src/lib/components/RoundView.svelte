<script lang="ts">
  import { _ } from "svelte-i18n";
  import {
    HANDICAPS,
    type Board,
    type BoardLedger,
    type Handicap,
    type HandicapPolicy,
    type Player,
    type Round,
    type RoundExplanation,
    type RuleId,
    type Winner,
  } from "../types";
  import { sourceBadge } from "../pairingSource";

  interface Props {
    round: Round;
    players: Player[];
    /** Whether/how handicap games are shown: none, allowed, or suggested. */
    handicapPolicy: HandicapPolicy;
    /** Suggested handicap per board, indexed like `round.boards`. */
    suggestedHandicaps: (Handicap | null)[];
    /** Why the engine chose these pairings (per-board rule ledger + report).
     *  `null` while loading or unavailable. */
    explanation?: RoundExplanation | null;
    /** Register a click on a board's player (toggles the winner). */
    onClickWinner: (boardIndex: number, clicked: Winner) => void;
    /** Set/clear the "a draw occurred" flag on a board. */
    onToggleDrawn: (boardIndex: number, drawn: boolean) => void;
    /** Set (or clear, with null) a board's handicap. */
    onSetHandicap: (boardIndex: number, handicap: Handicap | null) => void;
    busy?: boolean;
  }

  let {
    round,
    players,
    handicapPolicy,
    suggestedHandicaps,
    explanation = null,
    onClickWinner,
    onToggleDrawn,
    onSetHandicap,
    busy = false,
  }: Props = $props();

  // Resolve player ids to display names.
  const byId = $derived(new Map(players.map((p) => [p.id, p])));

  // --- Pairing explanation -------------------------------------------------

  // Order-independent key for a board's two players, to align a ledger (which
  // covers only Swiss boards, in engine order) with the displayed boards.
  function pairKey(a: string, b: string): string {
    return a < b ? `${a}|${b}` : `${b}|${a}`;
  }

  const ledgerByPair = $derived.by(() => {
    const map = new Map<string, BoardLedger>();
    for (const board of explanation?.boards ?? []) {
      if (board.player2) map.set(pairKey(board.player1, board.player2), board);
    }
    return map;
  });

  function boardLedger(board: Board): BoardLedger | undefined {
    return ledgerByPair.get(pairKey(board.player1, board.player2));
  }

  // Fold deviation fires on nearly every board, so it carries no signal — a
  // board is only "noteworthy" if a higher-priority rule (score gap, float, …)
  // was relaxed. Fold still shows in the full report, never as a per-board flag.
  function isNoteworthy(ledger: BoardLedger | undefined): boolean {
    return !!ledger && ledger.contributions.some((c) => c.rule !== "fold");
  }

  function ruleLabel(rule: RuleId): string {
    return $_(`roundView.explanation.rule.${rule}`);
  }

  // Tooltip listing the relaxed (non-fold) rules on a board, e.g.
  // "Compromise on: score gap, repeat float".
  function ledgerTooltip(ledger: BoardLedger): string {
    const rules = ledger.contributions
      .filter((c) => c.rule !== "fold")
      .map((c) => ruleLabel(c.rule))
      .join(", ");
    return $_("roundView.explanation.compromiseOn", { values: { rules } });
  }

  // The src column shows one glyph: the source emoji for forced/cup boards, or a
  // compromise flag for a noteworthy Swiss board (the two are mutually exclusive
  // — only Swiss boards get a ledger, and their source emoji is blank). The
  // compromise flag is a screen-only aid, dropped from the printed pairings.
  function cellTitle(board: Board): string {
    const emoji = sourceEmoji(board);
    if (emoji) return sourceBadge($_, board.source).text;
    const ledger = boardLedger(board);
    return isNoteworthy(ledger) ? ledgerTooltip(ledger!) : "";
  }

  // The bye's own ledger, flagged the same way as a board.
  const byeLedger = $derived(explanation?.bye);

  // Report line: rules relaxed this round, with counts, in priority order.
  const hasReport = $derived((explanation?.report.length ?? 0) > 0);
  let reportOpen = $state(false);

  function name(id: string): string {
    const p = byId.get(id);
    if (!p) return $_("roundView.unknownPlayer");
    const full = `${p.last_name} ${p.first_name}`.trim();
    return p.rating != null ? `${full} (${p.rating})` : full;
  }

  // A handicap needs an unambiguous giver — the higher-rated player — so it is
  // only offered when the two ratings differ (both unrated counts as equal).
  function handicapAllowed(board: Board): boolean {
    const r1 = byId.get(board.player1)?.rating ?? null;
    const r2 = byId.get(board.player2)?.rating ?? null;
    return r1 !== r2;
  }

  // Cup games are always played even — no picker, no suggestion, empty cells.
  function isCup(board: Board): boolean {
    return sourceBadge($_, board.source).kind === "cup";
  }

  // A single emoji flagging a non-Swiss pairing (forced or cup); blank for a
  // normal Swiss pairing. Kept as a narrow, header-less column between the two
  // players so it doesn't compete for attention with the pairing itself.
  function sourceEmoji(board: Board): string {
    switch (sourceBadge($_, board.source).kind) {
      case "forced":
        return "\u{1F512}";
      case "cup":
        return "\u{1F3C6}";
      default:
        return "";
    }
  }

  // Name of the frozen handicap giver, for the "X gives" hint.
  function giverName(board: Board): string {
    if (!board.handicap) return "";
    const id = board.handicap.giver === "player1" ? board.player1 : board.player2;
    const p = byId.get(id);
    return p ? `${p.last_name} ${p.first_name}`.trim() : $_("roundView.unknownPlayer");
  }

  function onHandicapChange(index: number, value: string) {
    onSetHandicap(index, value === "" ? null : (value as Handicap));
  }
</script>

<div class="round">
  <div class="round-toolbar print-hide">
    <button type="button" class="ghost" onclick={() => window.print()}>🖨 {$_("roundView.print")}</button>
  </div>
  {#if hasReport}
    <div class="report print-hide">
      <button
        type="button"
        class="report-toggle"
        aria-expanded={reportOpen}
        onclick={() => (reportOpen = !reportOpen)}
      >
        <span class="report-title">{$_("roundView.explanation.reportTitle")}</span>
        <span class="report-summary"
          >{explanation?.report
            .map((t) =>
              $_("roundView.explanation.ruleCount", {
                values: { count: t.boards, rule: ruleLabel(t.rule) },
              }),
            )
            .join(" · ")}</span
        >
        <span class="report-caret">{reportOpen ? "▾" : "▸"}</span>
      </button>
      {#if reportOpen}
        <ul class="report-list">
          {#each explanation?.report ?? [] as total (total.rule)}
            <li>
              <span class="report-rule">{ruleLabel(total.rule)}</span>
              <span class="report-boards"
                >{$_("roundView.explanation.boardsCount", { values: { count: total.boards } })}</span
              >
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
  {#if round.boards.length === 0}
    <p class="empty">{$_("roundView.noBoards")}</p>
  {:else}
    <table>
      <thead>
        <tr>
          <th class="src-col"></th>
          <th class="num">{$_("roundView.board")}</th>
          <th class="p1-col">{$_("roundView.player1")}</th>
          <th>{$_("roundView.player2")}</th>
          <th class="draw-col">{$_("roundView.draw")}</th>
          {#if handicapPolicy !== "none"}
            <th class="handicap-col">{$_("roundView.handicap")}</th>
            {#if handicapPolicy === "suggested"}
              <th class="suggested-col">{$_("roundView.suggested")}</th>
            {/if}
          {/if}
        </tr>
      </thead>
      <tbody>
        {#each round.boards as board, index (index)}
          <tr>
            <td
              class="src-col src-{sourceBadge($_, board.source).kind}"
              title={cellTitle(board)}
              >{#if sourceEmoji(board)}{sourceEmoji(board)}{:else if isNoteworthy(
                  boardLedger(board),
                )}<span class="compromise print-hide">⚠</span>{/if}</td
            >
            <td class="num">{index + 1}</td>
            <td class="p1-col">
              <button
                type="button"
                class="player"
                class:winner={board.result === "player1"}
                class:loser={board.result === "player2"}
                disabled={busy}
                title={$_("roundView.clickToSetWinner")}
                onclick={() => onClickWinner(index, "player1")}
              >
                {name(board.player1)}
              </button>
            </td>
            <td>
              <button
                type="button"
                class="player"
                class:winner={board.result === "player2"}
                class:loser={board.result === "player1"}
                disabled={busy}
                title={$_("roundView.clickToSetWinner")}
                onclick={() => onClickWinner(index, "player2")}
              >
                {name(board.player2)}
              </button>
            </td>
            <td class="draw-col">
              <button
                type="button"
                class="draw"
                class:active={board.drawn}
                disabled={busy}
                title={$_("roundView.drawTitle")}
                aria-pressed={board.drawn ?? false}
                onclick={() => onToggleDrawn(index, !board.drawn)}
              >
                =
              </button>
            </td>
            {#if handicapPolicy !== "none"}
              <td class="handicap-col">
                {#if !isCup(board)}
                  {#if handicapAllowed(board)}
                    <select
                      class="handicap"
                      disabled={busy}
                      value={board.handicap?.handicap ?? ""}
                      onchange={(e) => onHandicapChange(index, e.currentTarget.value)}
                    >
                      <option value=""></option>
                      {#each HANDICAPS as h (h.value)}
                        <option
                          value={h.value}
                          title={h.label}
                          style={suggestedHandicaps[index] === h.value ? "font-weight:700" : ""}
                        >
                          {suggestedHandicaps[index] === h.value ? "★ " : ""}{h.value}
                        </option>
                      {/each}
                    </select>
                    {#if board.handicap}
                      <span class="giver" title={$_("roundView.giverTitle")}
                        >{$_("roundView.giverGives", { values: { name: giverName(board) } })}</span
                      >
                    {/if}
                  {:else}
                    <span class="na" title={$_("roundView.needsDifferentRatings")}>—</span>
                  {/if}
                {/if}
              </td>
              {#if handicapPolicy === "suggested"}
                <td class="suggested suggested-col">
                  {#if !isCup(board) && suggestedHandicaps[index]}
                    <span title={HANDICAPS.find((h) => h.value === suggestedHandicaps[index])?.label}
                      >{suggestedHandicaps[index]}</span
                    >
                  {/if}
                </td>
              {/if}
            {/if}
          </tr>
        {/each}
      </tbody>
    </table>
    {#if round.completed}
      <p class="hint warning">
        ⚠ {$_("roundView.alreadyCompletedWarning")}
      </p>
    {:else}
      <p class="hint">{$_("roundView.clickToRecordWinner")}</p>
    {/if}
  {/if}

  {#if round.bye}
    <p class="bye">
      <strong>{$_("roundView.byeLabel")}</strong> {name(round.bye)}
      {#if isNoteworthy(byeLedger)}
        <span class="compromise print-hide" title={ledgerTooltip(byeLedger!)}>⚠</span>
      {/if}
    </p>
  {/if}
</div>

<style>
  .round-toolbar {
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
    padding: 0.3rem 0.6rem;
    border-bottom: 1px solid var(--border-divider);
    text-align: left;
  }
  th {
    color: var(--text-secondary);
    font-weight: 600;
    font-size: 0.8rem;
  }
  tbody tr:nth-child(even) {
    background: var(--bg-stripe);
  }
  .num {
    text-align: right;
    width: 3.5rem;
    font-variant-numeric: tabular-nums;
  }
  .src-col {
    width: 1.6rem;
    text-align: center;
    padding-left: 0.2rem;
    padding-right: 0.2rem;
    font-size: 0.95rem;
  }
  .compromise {
    color: var(--color-warning);
    cursor: help;
  }

  .report {
    margin-bottom: 0.75rem;
    font-size: 0.85rem;
  }
  .report-toggle {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    width: 100%;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border-divider);
    border-radius: 0.4rem;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .report-toggle:hover {
    background: var(--bg-hover);
  }
  .report-title {
    font-weight: 600;
  }
  .report-summary {
    color: var(--text-secondary);
    flex: 1;
  }
  .report-caret {
    color: var(--text-tertiary);
  }
  .report-list {
    margin: 0.4rem 0 0;
    padding: 0 0 0 0.5rem;
    list-style: none;
  }
  .report-list li {
    display: flex;
    gap: 0.75rem;
    padding: 0.15rem 0;
  }
  .report-rule {
    min-width: 10rem;
  }
  .report-boards {
    color: var(--text-secondary);
  }

  .p1-col {
    text-align: right;
  }
  .p1-col .player {
    text-align: right;
  }

  .player {
    width: 100%;
    text-align: left;
    padding: 0.3rem 0.5rem;
    border: 1px solid transparent;
    border-radius: 0.4rem;
    background: transparent;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .player:hover:not(:disabled) {
    border-color: var(--border-soft);
    background: var(--bg-hover);
  }
  .player.winner {
    color: var(--color-success);
    font-weight: 600;
  }
  .player.winner::before {
    content: "✓ ";
  }
  .player.loser {
    color: var(--text-tertiary);
  }

  .draw-col {
    text-align: center;
    width: 3.5rem;
  }
  .draw {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1.9rem;
    height: 1.9rem;
    padding: 0;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    background: transparent;
    color: var(--text-secondary);
    font: inherit;
    font-weight: 700;
    cursor: pointer;
  }
  .draw:hover:not(:disabled) {
    background: var(--bg-hover);
  }
  .draw.active {
    border-color: var(--color-warning);
    color: var(--color-warning);
    background: var(--bg-warning);
  }

  .handicap {
    width: 4.5rem;
    background: var(--bg-inset);
    color: inherit;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    padding: 0.2rem 0.35rem;
    font: inherit;
  }
  .giver {
    margin-left: 0.5rem;
    color: var(--text-tertiary);
    font-size: 0.8rem;
  }
  .suggested {
    color: var(--text-strong);
    font-size: 0.85rem;
  }
  .na {
    color: var(--text-muted);
  }

  .hint,
  .empty {
    color: var(--text-tertiary);
    font-size: 0.8rem;
    margin-top: 0.6rem;
  }
  .hint.warning {
    color: var(--color-warning);
  }
  .bye {
    margin-top: 1rem;
    color: var(--text-secondary);
    font-size: 0.9rem;
  }

  @media print {
    .print-hide {
      display: none;
    }
    .draw-col,
    .handicap-col {
      display: none;
    }
    /* Only the forced-pairing lock is dropped from print — the cup trophy stays. */
    .src-forced {
      visibility: hidden;
    }
    table,
    th,
    td,
    .player,
    .giver,
    .na,
    .hint,
    .bye,
    .suggested {
      color: #000 !important;
      background: transparent !important;
      border-color: #000 !important;
    }
    .player.winner,
    .player.loser {
      color: #000 !important;
    }
    .player {
      border-color: transparent !important;
    }
    tbody tr:nth-child(even) {
      background: transparent !important;
    }
  }
</style>
