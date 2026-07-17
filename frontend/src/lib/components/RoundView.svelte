<script lang="ts">
  import { _ } from "svelte-i18n";
  import {
    HANDICAPS,
    type Board,
    type BoardLedger,
    type Counterfactual,
    type CounterfactualMode,
    type Handicap,
    type NoShow,
    type Player,
    type Round,
    type RoundExplanation,
    type RuleId,
    type Winner,
  } from "../types";
  import { sourceBadge } from "../pairingSource";
  import { handicapGiverId } from "../boardOutcome";
  import { absent, toggledNoShow } from "../noShow";
  import { printPage } from "../platform";
  import type { HandicapChoice } from "../handicap";

  interface Props {
    round: Round;
    players: Player[];
    /** Whether/how handicap games are shown: none, allowed, or suggested. */
    handicapPolicy: HandicapChoice;
    /** Suggested handicap per board, indexed like `round.boards`. */
    suggestedHandicaps: (Handicap | null)[];
    /** Why the engine chose these pairings (per-board rule ledger + report).
     *  `null` while loading or unavailable. */
    explanation?: RoundExplanation | null;
    /** Ask what forcing/forbidding the pairing `a`–`b` would cost. Absent =
     *  probe disabled. */
    onProbe?: (a: number, b: number, mode: CounterfactualMode) => Promise<Counterfactual>;
    /** Whether this round can be re-paired (it is the current round and has no
     *  recorded results). Gates the "force this pairing" action. */
    canForce?: boolean;
    /** Apply a forced pairing: re-pair the round with `a`–`b` fixed. */
    onForcePairing?: (a: number, b: number) => Promise<void>;
    /** Register a click on a board's player (toggles the winner). */
    onClickWinner: (boardIndex: number, clicked: Winner) => void;
    /** Set/clear the "a draw occurred" flag on a board. */
    onToggleDrawn: (boardIndex: number, drawn: boolean) => void;
    /** Mark (or clear, with null) a board as a no-show: `absent` names the
     *  side(s) that failed to appear (one player, or `"both"`). */
    onSetNoShow: (boardIndex: number, absent: NoShow | null) => void;
    /** Set (or clear, with null) a board's handicap. */
    onSetHandicap: (boardIndex: number, handicap: Handicap | null) => void;
    /** Whether long (two-round) games are enabled — shows the per-board checkbox. */
    longEnabled?: boolean;
    /** Whether the viewed round is the current round (the long flag is only
     *  editable on the current round). */
    isCurrentRound?: boolean;
    /** Flag/unflag a board as a two-round long game. */
    onSetLong?: (boardIndex: number, long: boolean) => void;
    /** Pending long boards carried in from the previous round, so the referee can
     *  record their result while running this round. */
    carriedLongBoards?: { index: number; board: Board }[];
    /** Record the winner on a carried (previous-round) long board. */
    onCarriedWinner?: (boardIndex: number, clicked: Winner) => void;
    busy?: boolean;
  }

  let {
    round,
    players,
    handicapPolicy,
    suggestedHandicaps,
    explanation = null,
    onProbe,
    canForce = false,
    onForcePairing,
    onClickWinner,
    onToggleDrawn,
    onSetNoShow,
    onSetHandicap,
    longEnabled = false,
    isCurrentRound = false,
    onSetLong,
    carriedLongBoards = [],
    onCarriedWinner,
    busy = false,
  }: Props = $props();

  // The other side of a board.
  function other(side: Winner): Winner {
    return side === "player1" ? "player2" : "player1";
  }

  // A side's outcome, from the recorded result or a no-show. An opponent who
  // failed to appear hands `side` the point, but under `"both"` nobody won —
  // so winning compares `no_show` strictly, while losing goes through
  // `absent()`, which counts `"both"` as covering each side.
  function isWinner(board: Board, side: Winner): boolean {
    return board.result === side || board.no_show === other(side);
  }
  function isLoser(board: Board, side: Winner): boolean {
    return board.result === other(side) || absent(board.no_show, side);
  }

  // The long checkbox is editable only on the current round; turning it *on* also
  // needs the board still undecided (turning it off after a result is the demote
  // path). Mirrors the server's `set_board_long` guards.
  function longToggleDisabled(board: Board): boolean {
    if (busy || !isCurrentRound) return true;
    const decided = board.result != null || board.no_show != null;
    return decided && !board.long;
  }

  // Toggle `side`'s no-show independently, so the two buttons together cover all
  // of none / player1 / player2 / both (each side can be a no-show at once).
  function toggleNoShow(index: number, board: Board, side: Winner) {
    onSetNoShow(index, toggledNoShow(board.no_show, side));
  }

  // Resolve tournament numbers to display names.
  const byId = $derived(
    new Map(
      players
        .filter((p): p is Player & { tournament_id: number } => p.tournament_id != null)
        .map((p) => [p.tournament_id, p]),
    ),
  );

  // --- Pairing explanation -------------------------------------------------

  // Order-independent key for a board's two players, to align a ledger (which
  // covers only Swiss boards, in engine order) with the displayed boards.
  function pairKey(a: number, b: number): string {
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

  // A board ledger as readable text: "A vs B" or "X (bye)".
  function ledgerLabel(b: BoardLedger): string {
    if (!b.player2) {
      return $_("roundView.explanation.byeBoard", { values: { name: name(b.player1) } });
    }
    return $_("roundView.explanation.board", {
      values: { a: name(b.player1), b: name(b.player2) },
    });
  }

  // The boards a given rule fired on, with that rule's units on each — so the
  // expanded report shows *which* boards each rule affected, not just how many.
  function boardsForRule(rule: RuleId): { label: string; units: number }[] {
    const all: BoardLedger[] = [...(explanation?.boards ?? [])];
    if (explanation?.bye) all.push(explanation.bye);
    const out: { label: string; units: number }[] = [];
    for (const b of all) {
      const c = b.contributions.find((x) => x.rule === rule);
      if (c) out.push({ label: ledgerLabel(b), units: c.units });
    }
    return out;
  }

  // --- Byes -----------------------------------------------------------------

  // Everyone who sits this round out with no opponent: the engine's bye, any
  // the referee forced, and the rare cup bye. Absences aren't here — they never
  // reach the pairing, so the round view has no row for them.
  const byeSitouts = $derived((round.sitouts ?? []).filter((s) => s.kind !== "absent"));

  // The bye the *engine* chose, if any. Only this one was the engine's decision,
  // so it's the only one the pairing explanations can speak about.
  const swissBye = $derived(
    (round.sitouts ?? []).find((s) => s.kind === "bye")?.player ?? null,
  );

  // --- Alphabetical (lookup) mode -------------------------------------------

  // A pairing sheet to print at the start of a round: every board appears twice,
  // once from each player's side, so a player finds their own name in a single
  // alphabetical scan of the left column and reads their board and opponent
  // across. It is a lookup sheet, not an entry sheet — nothing here is
  // clickable, and the referee flips back to the default view to record results.
  let alphabetical = $state(false);

  /** One printed line: a player, their board, and who they face. */
  interface AlphaRow {
    key: string;
    /** The board number, as numbered in the default (engine-order) view. */
    number: number;
    /** The player this line is filed under, by tournament number. */
    left: number;
    /** Their opponent, or null on a sit-out (a bye has no one to swap with). */
    right: number | null;
    /** The board, or null on a sit-out row. */
    board: Board | null;
    /** Which side `left` plays on `board` — the flipped copy reads its result
     *  from `player2`, so a recorded win stays on the right player. */
    leftSide: Winner;
    /** A sit-out row from the cup bracket rather than the Swiss bye. */
    cupBye: boolean;
    /** Index into `round.boards` / `suggestedHandicaps`; -1 on a sit-out row. */
    index: number;
  }

  // Last name, then first name — the order players expect to look themselves up
  // in, and not the order `name()` would give (it appends the rating).
  function byLastName(x: number, y: number): number {
    const a = byId.get(x);
    const b = byId.get(y);
    return (
      (a?.last_name ?? "").localeCompare(b?.last_name ?? "") ||
      (a?.first_name ?? "").localeCompare(b?.first_name ?? "")
    );
  }

  const alphaRows = $derived.by(() => {
    const rows: AlphaRow[] = [];
    round.boards.forEach((board, index) => {
      const common = { number: index + 1, board, cupBye: false, index };
      rows.push({
        ...common,
        key: `${index}-1`,
        left: board.player1,
        right: board.player2,
        leftSide: "player1",
      });
      rows.push({
        ...common,
        key: `${index}-2`,
        left: board.player2,
        right: board.player1,
        leftSide: "player2",
      });
    });
    // Sit-outs keep the numbers the default view gives them (after the boards),
    // but sort in with everyone else so a bye-taker's single scan finds them.
    byeSitouts.forEach((sitout, i) => {
      rows.push({
        key: `bye-${sitout.player}`,
        number: round.boards.length + i + 1,
        left: sitout.player,
        right: null,
        board: null,
        leftSide: "player1",
        cupBye: typeof sitout.kind !== "string",
        index: -1,
      });
    });
    return rows.sort((a, b) => byLastName(a.left, b.left));
  });

  // --- Counterfactual probe ("why not pair A and B?") ----------------------

  // The bye sentinel used by the server's counterfactual/force-pairing APIs
  // (`PHANTOM = 0u32`) — never a real player, since tournament numbers start
  // at 1.
  const PHANTOM = 0;

  // The engine-paired players of this round (Swiss boards + the bye), the only
  // ones a counterfactual can reason about, sorted by name for the pickers.
  const swissPlayers = $derived.by(() => {
    const ids = new Set<number>();
    for (const b of round.boards) {
      if (!b.source || b.source.kind === "swiss") {
        ids.add(b.player1);
        ids.add(b.player2);
      }
    }
    if (swissBye != null) ids.add(swissBye);
    return [...ids].sort((x, y) => name(x).localeCompare(name(y)));
  });

  let probeOpen = $state(false);
  let probeMode = $state<CounterfactualMode>("force");
  let probeA = $state<number | "">("");
  let probeB = $state<number | "">("");
  let probeBusy = $state(false);
  let probeResult = $state<Counterfactual | null>(null);
  // The mode the current result was computed for (so the "force" action only
  // shows for a force probe, not after the pickers/mode change).
  let resultMode = $state<CounterfactualMode>("force");
  let probeError = $state("");

  // Reset the probe whenever the round changes (its pairings changed under us).
  function resetProbe() {
    probeA = "";
    probeB = "";
    probeResult = null;
    probeError = "";
  }
  $effect(() => {
    void round.number;
    void round.boards;
    resetProbe();
  });

  // Changing either picker or the mode invalidates a previously computed result:
  // clear it (and any error) so the stale preview — and the "apply" action that
  // acts on the *current* selection — never outlives the pairing it was computed
  // for (which would let "Apply" force a different pair than the one shown).
  $effect(() => {
    void probeA;
    void probeB;
    void probeMode;
    probeResult = null;
    probeError = "";
  });

  // Each player's opponent in this round's Swiss boards (both directions). The
  // bye-taker's "opponent" is the bye sentinel, so forbidding it reads as
  // forbidding the bye itself (i.e. forcing them to play).
  const opponentOf = $derived.by(() => {
    const m = new Map<number, number>();
    for (const b of round.boards) {
      if (!b.source || b.source.kind === "swiss") {
        m.set(b.player1, b.player2);
        m.set(b.player2, b.player1);
      }
    }
    if (swissBye != null) m.set(swissBye, PHANTOM);
    return m;
  });

  // Forbid ("why paired?") only has something to say about a player's *actual*
  // opponent, so it needs a single pick — the partner is derived. Force ("why
  // not?") proposes a new pairing, so it needs both.
  const forbidPartner = $derived(
    probeMode === "forbid" && probeA !== "" ? opponentOf.get(probeA) : undefined,
  );
  const effectiveB = $derived(probeMode === "force" ? probeB : (forbidPartner ?? ""));

  const canProbe = $derived(
    !!onProbe && probeA !== "" && effectiveB !== "" && probeA !== effectiveB && !probeBusy,
  );

  async function runProbe() {
    if (!onProbe || !canProbe || probeA === "" || effectiveB === "") return;
    probeBusy = true;
    probeError = "";
    probeResult = null;
    try {
      probeResult = await onProbe(probeA, effectiveB, probeMode);
      resultMode = probeMode;
    } catch (err) {
      probeError = err instanceof Error ? err.message : String(err);
    } finally {
      probeBusy = false;
    }
  }

  // The "force this pairing" action is offered only after a *force* probe that
  // actually changes something, on a round that can still be re-paired.
  const canApplyForce = $derived(
    !!onForcePairing &&
      canForce &&
      resultMode === "force" &&
      (probeResult?.changed.length ?? 0) > 0 &&
      !probeResult?.scoped_out &&
      !probeBusy,
  );

  async function applyForce() {
    if (!onForcePairing || !canApplyForce || probeA === "" || probeB === "") return;
    const a = probeA;
    const b = probeB;
    probeBusy = true;
    probeError = "";
    try {
      await onForcePairing(a, b);
      // The round re-paired; the fresh pairings make the old result stale.
      resetProbe();
    } catch (err) {
      probeError = err instanceof Error ? err.message : String(err);
    } finally {
      probeBusy = false;
    }
  }

  function probeName(id: number): string {
    return id === PHANTOM ? $_("roundView.probe.bye") : name(id);
  }

  // A changed board as readable text: "X vs Y" or "X takes the bye".
  function changedBoardText(board: BoardLedger): string {
    if (!board.player2) {
      return $_("roundView.probe.newBye", { values: { name: probeName(board.player1) } });
    }
    return $_("roundView.probe.newBoard", {
      values: { a: probeName(board.player1), b: probeName(board.player2) },
    });
  }

  // Rules that got worse / better under the probe, as label lists.
  const worseRules = $derived(
    (probeResult?.cost_delta ?? []).filter((d) => d.units > 0).map((d) => ruleLabel(d.rule)),
  );
  const betterRules = $derived(
    (probeResult?.cost_delta ?? []).filter((d) => d.units < 0).map((d) => ruleLabel(d.rule)),
  );

  function name(id: number): string {
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
    const id = handicapGiverId(board);
    if (id == null) return "";
    const p = byId.get(id);
    return p ? `${p.last_name} ${p.first_name}`.trim() : $_("roundView.unknownPlayer");
  }

  function onHandicapChange(index: number, value: string) {
    onSetHandicap(index, value === "" ? null : (value as Handicap));
  }
</script>

<div class="round">
  <div class="round-toolbar print-hide">
    <button
      type="button"
      class="ghost"
      class:active={alphabetical}
      aria-pressed={alphabetical}
      title={$_("roundView.alphabeticalTitle")}
      onclick={() => (alphabetical = !alphabetical)}
    >
      🔤 {$_("roundView.alphabetical")}
    </button>
    <button type="button" class="ghost" onclick={() => printPage()}>🖨 {$_("roundView.print")}</button>
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
              <div class="report-rule-head">
                <span class="report-rule">{ruleLabel(total.rule)}</span>
                <span class="report-boards"
                  >{$_("roundView.explanation.boardsCount", {
                    values: { count: total.boards },
                  })}</span
                >
              </div>
              <ul class="report-affected">
                {#each boardsForRule(total.rule) as board (board.label)}
                  <li>
                    <span>{board.label}</span>
                    <span class="report-units">{board.units}</span>
                  </li>
                {/each}
              </ul>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
  {#if carriedLongBoards.length > 0}
    <div class="carried">
      <p class="carried-title">{$_("roundView.carriedTitle")}</p>
      <table class="carried-table">
        <tbody>
          {#each carriedLongBoards as { index, board } (index)}
            <tr>
              <td class="p1-col">
                <button
                  type="button"
                  class="player"
                  class:winner={isWinner(board, "player1")}
                  class:loser={isLoser(board, "player1")}
                  disabled={busy || !onCarriedWinner}
                  title={$_("roundView.clickToSetWinner")}
                  onclick={() => onCarriedWinner?.(index, "player1")}
                >
                  {name(board.player1)}
                </button>
              </td>
              <td>
                <button
                  type="button"
                  class="player"
                  class:winner={isWinner(board, "player2")}
                  class:loser={isLoser(board, "player2")}
                  disabled={busy || !onCarriedWinner}
                  title={$_("roundView.clickToSetWinner")}
                  onclick={() => onCarriedWinner?.(index, "player2")}
                >
                  {name(board.player2)}
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="hint">{$_("roundView.carriedHint")}</p>
    </div>
  {/if}
  {#if round.boards.length === 0 && byeSitouts.length === 0}
    <p class="empty">{$_("roundView.noBoards")}</p>
  {:else if alphabetical}
    <table>
      <thead>
        <tr>
          <th class="num">{$_("roundView.board")}</th>
          <th>{$_("roundView.playerColumn")}</th>
          <th>{$_("roundView.opponentColumn")}</th>
          {#if handicapPolicy === "suggested"}
            <th class="suggested-col">{$_("roundView.suggested")}</th>
          {/if}
        </tr>
      </thead>
      <tbody>
        {#each alphaRows as row (row.key)}
          <tr class:bye-row={!row.board}>
            <td class="num"
              >{row.number}{#if row.board?.long}<span
                  class="long-badge"
                  title={$_("roundView.longTitle")}>★2R</span
                >{/if}</td
            >
            <td>
              <span
                class="player"
                class:winner={!row.board || isWinner(row.board, row.leftSide)}
                class:loser={row.board && isLoser(row.board, row.leftSide)}>{name(row.left)}</span
              >
            </td>
            <td>
              {#if row.right == null}
                <span class="player bye-opponent"
                  >{$_(row.cupBye ? "roundView.cupByeOpponent" : "roundView.byeOpponent")}</span
                >
              {:else}
                <span
                  class="player"
                  class:winner={isWinner(row.board!, other(row.leftSide))}
                  class:loser={isLoser(row.board!, other(row.leftSide))}>{name(row.right)}</span
                >
              {/if}
            </td>
            {#if handicapPolicy === "suggested"}
              <td class="suggested suggested-col">
                {#if row.board && !isCup(row.board) && suggestedHandicaps[row.index]}
                  <span title={HANDICAPS.find((h) => h.value === suggestedHandicaps[row.index])?.label}
                    >{suggestedHandicaps[row.index]}</span
                  >
                {/if}
              </td>
            {/if}
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <table>
      <thead>
        <tr>
          <th class="src-col"></th>
          <th class="num">{$_("roundView.board")}</th>
          <th class="p1-col">{$_("roundView.player1")}</th>
          <th>{$_("roundView.player2")}</th>
          <th class="draw-col">{$_("roundView.draw")}</th>
          <th class="noshow-col">{$_("roundView.noShow")}</th>
          {#if longEnabled}
            <th class="long-col">{$_("roundView.long")}</th>
          {/if}
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
            <td class="num"
              >{index + 1}{#if board.long}<span class="long-badge" title={$_("roundView.longTitle")}
                  >★2R</span
                >{/if}</td
            >
            <td class="p1-col">
              <button
                type="button"
                class="player"
                class:winner={isWinner(board, "player1")}
                class:loser={isLoser(board, "player1")}
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
                class:winner={isWinner(board, "player2")}
                class:loser={isLoser(board, "player2")}
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
            <td class="noshow-col">
              <div class="noshow">
                <button
                  type="button"
                  class="noshow-btn"
                  class:active={absent(board.no_show, "player1")}
                  disabled={busy}
                  aria-pressed={absent(board.no_show, "player1")}
                  title={$_("roundView.noShowTitle", { values: { name: name(board.player1) } })}
                  onclick={() => toggleNoShow(index, board, "player1")}
                >
                  ◀
                </button>
                <button
                  type="button"
                  class="noshow-btn"
                  class:active={absent(board.no_show, "player2")}
                  disabled={busy}
                  aria-pressed={absent(board.no_show, "player2")}
                  title={$_("roundView.noShowTitle", { values: { name: name(board.player2) } })}
                  onclick={() => toggleNoShow(index, board, "player2")}
                >
                  ▶
                </button>
              </div>
            </td>
            {#if longEnabled}
              <td class="long-col">
                <!-- Cup boards can be long too; the server couples every cup board
                     of the round, so ticking one ticks them all. -->
                <input
                  type="checkbox"
                  class="long-check"
                  checked={board.long ?? false}
                  disabled={longToggleDisabled(board)}
                  title={$_("roundView.longTitle")}
                  onchange={(e) => onSetLong?.(index, e.currentTarget.checked)}
                />
              </td>
            {/if}
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
        {#each byeSitouts as sitout, i (sitout.player)}
          {@const isCup = typeof sitout.kind !== "string"}
          <tr class="bye-row">
            <td class="src-col">
              {#if isCup}
                🏆
              {:else if sitout.kind === "bye" && isNoteworthy(byeLedger)}
                <span class="compromise print-hide" title={ledgerTooltip(byeLedger!)}>⚠</span>
              {/if}
            </td>
            <td class="num">{round.boards.length + i + 1}</td>
            <td class="p1-col">
              <span class="player winner">{name(sitout.player)}</span>
            </td>
            <td>
              <span class="player bye-opponent"
                >{$_(isCup ? "roundView.cupByeOpponent" : "roundView.byeOpponent")}</span
              >
            </td>
            <td class="draw-col"></td>
            <td class="noshow-col"></td>
            {#if longEnabled}
              <td class="long-col"></td>
            {/if}
            {#if handicapPolicy !== "none"}
              <td class="handicap-col"></td>
              {#if handicapPolicy === "suggested"}
                <td class="suggested-col"></td>
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

  {#if onProbe && swissPlayers.length >= 4}
    <div class="probe print-hide">
      <button
        type="button"
        class="probe-toggle"
        aria-expanded={probeOpen}
        onclick={() => (probeOpen = !probeOpen)}
      >
        <span class="probe-title">{$_("roundView.probe.title")}</span>
        <span class="report-caret">{probeOpen ? "▾" : "▸"}</span>
      </button>
      {#if probeOpen}
        <div class="probe-body">
          <div class="probe-modes">
            <button
              type="button"
              class="probe-mode"
              class:active={probeMode === "force"}
              disabled={probeBusy}
              onclick={() => (probeMode = "force")}
            >
              {$_("roundView.probe.modeForce")}
            </button>
            <button
              type="button"
              class="probe-mode"
              class:active={probeMode === "forbid"}
              disabled={probeBusy}
              onclick={() => (probeMode = "forbid")}
            >
              {$_("roundView.probe.modeForbid")}
            </button>
          </div>
          <p class="probe-hint">
            {probeMode === "force"
              ? $_("roundView.probe.hintForce")
              : $_("roundView.probe.hintForbid")}
          </p>
          <div class="probe-controls">
            <select bind:value={probeA} disabled={probeBusy}>
              <option value="">{$_("roundView.probe.pick")}</option>
              {#each swissPlayers as id (id)}
                <option value={id}>{name(id)}</option>
              {/each}
            </select>
            {#if probeMode === "force"}
              <span class="probe-vs">{$_("roundView.probe.and")}</span>
              <select bind:value={probeB} disabled={probeBusy}>
                <option value="">{$_("roundView.probe.pick")}</option>
                {#each swissPlayers as id (id)}
                  <option value={id}>{name(id)}</option>
                {/each}
                {#if swissBye != null}
                  <option value={PHANTOM}>{$_("roundView.probe.bye")}</option>
                {/if}
              </select>
            {:else}
              <span class="probe-vs">{$_("roundView.probe.pairedWith")}</span>
              <span class="probe-partner">
                {#if probeA}
                  {forbidPartner ? probeName(forbidPartner) : $_("roundView.probe.noPartner")}
                {:else}
                  —
                {/if}
              </span>
            {/if}
            <button type="button" class="ghost" disabled={!canProbe} onclick={runProbe}>
              {$_("roundView.probe.submit")}
            </button>
          </div>

          {#if probeBusy}
            <p class="probe-status">{$_("roundView.probe.working")}</p>
          {:else if probeError}
            <p class="probe-status error">{probeError}</p>
          {:else if probeResult}
            {#if probeResult.scoped_out}
              <p class="probe-status">
                {$_(`roundView.probe.scopedOut.${probeResult.scoped_out}`)}
              </p>
            {:else if probeResult.changed.length === 0}
              <p class="probe-status">{$_("roundView.probe.noChange")}</p>
            {:else}
              <div class="probe-result">
                {#if worseRules.length}
                  <p class="probe-cost">
                    <strong>{$_("roundView.probe.worseOn")}</strong>
                    {worseRules.join(", ")}
                  </p>
                {/if}
                {#if betterRules.length}
                  <p class="probe-cost">
                    <strong>{$_("roundView.probe.betterOn")}</strong>
                    {betterRules.join(", ")}
                  </p>
                {/if}
                <p class="probe-boards-label">{$_("roundView.probe.newBoardsLabel")}</p>
                <ul class="probe-boards">
                  {#each probeResult.changed as board, i (i)}
                    <li>{changedBoardText(board)}</li>
                  {/each}
                </ul>
                {#if canApplyForce}
                  <button type="button" class="probe-apply" disabled={probeBusy} onclick={applyForce}>
                    {$_("roundView.probe.apply")}
                  </button>
                {/if}
              </div>
            {/if}
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .round-toolbar {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-bottom: 0.5rem;
  }
  /* Matches `.probe-mode.active`: an accent border on the ghost button's
     transparent background. (`--text-on-accent` is for text *on* an accent
     fill, so it would be invisible here.) */
  .round-toolbar .ghost.active {
    border-color: var(--color-accent, var(--border-strong));
    color: var(--text-primary);
    font-weight: 600;
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
  /* Match every body cell to the height of the tallest control (the 1.9rem
     draw/no-show buttons), so the bye and cup-bye rows — which have no such
     controls — are the same height as the game rows. */
  tbody td {
    height: 1.9rem;
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
  .report-list > li {
    padding: 0.25rem 0;
  }
  .report-rule-head {
    display: flex;
    gap: 0.75rem;
    align-items: baseline;
  }
  .report-rule {
    min-width: 10rem;
    font-weight: 600;
  }
  .report-boards {
    color: var(--text-secondary);
  }
  .report-affected {
    margin: 0.2rem 0 0;
    padding: 0 0 0 1rem;
    list-style: none;
    color: var(--text-secondary);
  }
  .report-affected li {
    display: flex;
    justify-content: space-between;
    gap: 0.75rem;
    max-width: 22rem;
    padding: 0.05rem 0;
  }
  .report-units {
    font-variant-numeric: tabular-nums;
    color: var(--text-tertiary);
  }

  .probe {
    margin-top: 1rem;
    font-size: 0.85rem;
  }
  .probe-toggle {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border-divider);
    border-radius: 0.4rem;
    background: transparent;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .probe-toggle:hover {
    background: var(--bg-hover);
  }
  .probe-title {
    font-weight: 600;
  }
  .probe-body {
    margin-top: 0.5rem;
    padding: 0.5rem;
    border: 1px solid var(--border-divider);
    border-radius: 0.4rem;
  }
  .probe-modes {
    display: flex;
    gap: 0.4rem;
    margin-bottom: 0.5rem;
  }
  .probe-mode {
    padding: 0.25rem 0.6rem;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    background: transparent;
    color: var(--text-secondary);
    font: inherit;
    cursor: pointer;
  }
  .probe-mode:hover:not(:disabled) {
    background: var(--bg-hover);
  }
  .probe-mode.active {
    border-color: var(--color-accent, var(--border-strong));
    color: var(--text-primary);
    font-weight: 600;
  }
  .probe-hint {
    margin: 0 0 0.5rem;
    color: var(--text-secondary);
  }
  .probe-apply {
    margin-top: 0.6rem;
    padding: 0.35rem 0.7rem;
    border: 1px solid var(--color-warning);
    border-radius: 0.4rem;
    background: var(--bg-warning);
    color: var(--color-warning);
    font: inherit;
    font-weight: 600;
    cursor: pointer;
  }
  .probe-apply:hover:not(:disabled) {
    filter: brightness(1.05);
  }
  .probe-apply:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .probe-controls {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .probe-controls select {
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    /* Theme-aware inset (like .tb-select), not `transparent`: a transparent
       native <select> falls back to the OS light combobox on Windows/WebView2,
       giving white-on-white with the inherited light text in dark mode. */
    background: var(--bg-inset);
    color: inherit;
    font: inherit;
  }
  .probe-vs {
    color: var(--text-secondary);
  }
  .probe-partner {
    font-weight: 600;
  }
  .probe-status {
    margin: 0.6rem 0 0;
    color: var(--text-secondary);
  }
  .probe-status.error {
    color: var(--color-warning);
  }
  .probe-result {
    margin-top: 0.6rem;
  }
  .probe-cost {
    margin: 0.2rem 0;
  }
  .probe-boards-label {
    margin: 0.5rem 0 0.2rem;
    color: var(--text-secondary);
  }
  .probe-boards {
    margin: 0;
    padding-left: 1.1rem;
  }
  .probe-boards li {
    padding: 0.1rem 0;
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
  /* Only the clickable player buttons get the hover affordance; the bye and
     cup-bye rows render the player as a plain (non-interactive) span. */
  button.player:hover:not(:disabled) {
    border-color: var(--border-soft);
    background: var(--bg-hover);
  }
  .player::before {
    content: "✓ ";
    visibility: hidden;
  }
  .player.winner {
    color: var(--color-success);
  }
  .player.winner::before {
    visibility: visible;
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

  .noshow-col {
    text-align: center;
    width: 4rem;
  }
  .noshow {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.2rem;
  }
  .noshow-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1.5rem;
    height: 1.9rem;
    padding: 0;
    border: 1px solid var(--border-soft);
    border-radius: 0.4rem;
    background: transparent;
    color: var(--text-tertiary);
    font: inherit;
    cursor: pointer;
  }
  .noshow-btn:hover:not(:disabled) {
    background: var(--bg-hover);
  }
  .noshow-btn.active {
    border-color: var(--color-danger);
    color: var(--color-danger);
    background: var(--bg-warning);
  }

  .long-col {
    text-align: center;
    width: 3rem;
  }
  .long-check {
    cursor: pointer;
  }
  .long-check:disabled {
    cursor: default;
  }
  /* A small badge on long boards, so which games run two rounds is visible at a
     glance on screen and survives into the printed pairing sheet. */
  .long-badge {
    margin-left: 0.35rem;
    padding: 0 0.25rem;
    border: 1px solid var(--border-soft);
    border-radius: 0.3rem;
    color: var(--text-secondary);
    font-size: 0.7rem;
    font-weight: 700;
    white-space: nowrap;
  }

  .carried {
    margin-bottom: 0.9rem;
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--border-divider);
    border-radius: 0.4rem;
    background: var(--bg-stripe);
  }
  .carried-title {
    margin: 0 0 0.3rem;
    font-weight: 600;
    font-size: 0.85rem;
  }
  .carried-table {
    width: auto;
  }
  .carried-table td {
    border-bottom: none;
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
  .bye-opponent {
    color: var(--text-tertiary);
    font-style: italic;
  }

  @media print {
    .print-hide {
      display: none;
    }
    .draw-col,
    .noshow-col,
    .long-col,
    .handicap-col {
      display: none;
    }
    /* On screen the table fills the width; in print, size columns to their
       content so the visible columns (board #, players, suggested handicap)
       sit close together instead of being stretched apart. */
    table {
      width: auto;
    }
    .player {
      width: auto;
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
