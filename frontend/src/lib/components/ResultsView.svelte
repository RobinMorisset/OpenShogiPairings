<script lang="ts">
  import { _ } from "svelte-i18n";
  import { TIEBREAKS } from "../types";
  import type { Board, CupPodium, Player, Round, Standing, Tiebreak, Tournament, Winner } from "../types";
  import { tiebreakLabel, tiebreakTitle } from "../tiebreaks";
  import { boardOutcome } from "../boardOutcome";
  import { partitionDropped } from "../tiebreak";
  import { formatScore, HALF_POINT_TIEBREAKS } from "../score";
  import { printPage } from "../platform";

  interface Props {
    tournament: Tournament;
    /** Ranked standings computed server-side (the canonical ordering). */
    standings: Standing[];
    /** The cup podium, once decided (adds medals; doesn't reorder the table). */
    cupPodium?: CupPodium | null;
    /** Winner that counts for standings/pairing per board, server-computed
     *  (respects the Wiel rule), indexed like `tournament.rounds[i].boards[j]`. */
    effectiveWinners: (Winner | null)[][];
  }

  let { tournament, standings, cupPodium = null, effectiveWinners }: Props = $props();

  // Round number → its index into `tournament.rounds` (and so into
  // `effectiveWinners`), since that array isn't filtered to completed rounds.
  const roundIndexByNumber = $derived(
    new Map(tournament.rounds.map((r, i) => [r.number, i])),
  );

  // The tie-break columns to show, in the referee-chosen order — resolved from
  // the settings to their label/field/tooltip. Unknown codes (from a newer save)
  // are skipped.
  const eloEstimateNeeded = $derived(
    tournament.settings.elo_pairing_enabled ?? false,
  );

  const tiebreakColumns = $derived(
    (tournament.settings.tiebreaks ?? [])
      // Estimated ELO only ranks when a live estimate is maintained (either ELO
      // mode); drop it as a column otherwise (defends against a loaded save
      // that still lists it).
      .filter((code) => code !== "est_elo" || eloEstimateNeeded)
      .map((code) => TIEBREAKS.find((t) => t.code === code))
      .filter((t): t is (typeof TIEBREAKS)[number] => t != null),
  );

  // The estimated-ELO column is only meaningful when a live estimate is
  // maintained (either ELO mode). Show it as a dedicated column there — unless
  // the referee already added it to the ranking criteria, in which case it
  // appears as a tie-break column (with its ranking position) and this one
  // would duplicate it.
  const showEstimatedElo = $derived(
    eloEstimateNeeded && !(tournament.settings.tiebreaks ?? []).includes("est_elo"),
  );

  // Player id → medal, from the cup podium (the table order stays pure-Swiss).
  // A place can be null when a double no-show left it undetermined (e.g. both
  // finalists absent), so each medal is awarded only if its winner exists.
  const medalOf = $derived.by(() => {
    const m = new Map<string, string>();
    if (cupPodium) {
      if (cupPodium.champion) m.set(cupPodium.champion, "🥇");
      if (cupPodium.runner_up) m.set(cupPodium.runner_up, "🥈");
      if (cupPodium.third) m.set(cupPodium.third, "🥉");
    }
    return m;
  });

  // One column per completed round.
  const completedRounds = $derived(tournament.rounds.filter((r) => r.completed));

  // Map player UUID → their tournament number, for opponent references.
  const numberOf = $derived(
    new Map(tournament.players.map((p) => [p.id, p.tournament_id])),
  );

  // Rows follow the server's ranked order, joined to each player's details.
  const byId = $derived(new Map(tournament.players.map((p) => [p.id, p])));

  // Standings keyed by player id, so a cell can look up an opponent's metrics
  // when explaining how a tie-break was summed.
  const standingById = $derived(new Map(standings.map((s) => [s.player_id, s])));

  // A player's full name for a tooltip, "Last First"; an em dash if unknown
  // (e.g. an opponent removed after the game).
  const nameOf = (id: string): string => {
    const p = byId.get(id);
    return p ? `${p.last_name} ${p.first_name}`.trim() : "—";
  };

  // Show a MacMahon column (between Wins and Points) when the starting score
  // matters — either someone has MacMahon starting points, or a manual
  // bonus/penalty adjusts a starting score. Its cell hosts the adjustment
  // underline that used to live on the Points cell.
  const showMacmahon = $derived(
    standings.some((s) => s.macmahon !== 0) ||
      tournament.players.some((p) => (p.adjustments ?? []).length > 0),
  );
  const rows = $derived(
    standings
      .map((standing) => ({ standing, player: byId.get(standing.player_id) }))
      .filter(
        (r): r is { standing: Standing; player: Player } => r.player != null,
      ),
  );

  type PlayedCell = {
    kind: "played";
    opponent: string;
    /** The opponent's full name, for the cell tooltip. */
    opponentName: string;
    /** This player actually won the game — drives the +/− sign and colour. */
    actualWon: boolean;
    /** Counts as a win in the standings (the giver always does). */
    effectiveWon: boolean;
    /** A draw occurred before the decisive game. */
    drawn: boolean;
    /** Present for a handicap game: the code, and whether this player conceded. */
    handicap?: { code: string; gave: boolean };
    /** Points this player had over the opponent at pairing time (negative = played up). */
    pointsDiff: number;
  };

  type Cell =
    | { kind: "bye" }
    // An absence: `half` when the tournament awards absences half a point (a
    // scored half-bye, `0=`), otherwise a plain zero (`0−`).
    | { kind: "absent"; half: boolean }
    // A no-show board: this player was the absentee (`0#`) or the one who
    // showed up and was credited the free point, bye-style (`0+`).
    | { kind: "no-show"; opponentName: string }
    | { kind: "no-show-win"; opponentName: string }
    | { kind: "pending"; opponent: string; opponentName: string }
    // A long (two-round) game started this round: its result belongs to the next
    // round's column, so this column shows a `0−` placeholder.
    | { kind: "long-pending"; opponentName: string }
    | PlayedCell;

  // A long (two-round) game lives in its starting round but its result is only
  // known in the next round, so it shows a `0−` placeholder in the starting
  // column and its result in the following column — matching the American Grid.
  function longAwareCell(player: Player, i: number): Cell {
    const onLong = (round: Round) =>
      round.boards.find(
        (b) => b.long && (b.player1 === player.id || b.player2 === player.id),
      );
    const here = onLong(completedRounds[i]);
    if (here) {
      const opp = here.player1 === player.id ? here.player2 : here.player1;
      return { kind: "long-pending", opponentName: nameOf(opp) };
    }
    if (i > 0) {
      const prev = completedRounds[i - 1];
      const idx = prev.boards.findIndex(
        (b) => b.long && (b.player1 === player.id || b.player2 === player.id),
      );
      if (idx >= 0) return cellForBoard(player, prev, prev.boards[idx], idx);
    }
    return cellFor(player, completedRounds[i]);
  }

  function cellFor(player: Player, round: Round): Cell {
    // The Swiss bye and the (rare) cup bye both read as a free point.
    if (round.bye === player.id || (round.cup_byes ?? []).includes(player.id))
      return { kind: "bye" };
    const boardIdx = round.boards.findIndex(
      (b) => b.player1 === player.id || b.player2 === player.id,
    );
    if (boardIdx < 0) {
      // No board and not the bye: absent. It scores a half point (a `0=`
      // half-bye) when the tournament awards absences half a point *and* the
      // player is in this round's absent list — matching the server's scoring.
      const half =
        (tournament.settings.half_point_absences ?? false) &&
        (round.absent ?? []).includes(player.id);
      return { kind: "absent", half };
    }
    return cellForBoard(player, round, round.boards[boardIdx], boardIdx);
  }

  /** The cross-table cell for a specific board (played / no-show / pending). */
  function cellForBoard(player: Player, round: Round, board: Board, boardIdx: number): Cell {
    const isP1 = board.player1 === player.id;
    const side: Winner = isP1 ? "player1" : "player2";
    const opponentUuid = isP1 ? board.player2 : board.player1;
    const opponentId = numberOf.get(opponentUuid);
    const opponent = opponentId != null ? String(opponentId) : "?";
    const opponentName = nameOf(opponentUuid);
    if (board.no_show) {
      // This side is a no-show for a single absence on their side, or when both
      // players were absent; otherwise they are the one who showed up.
      const iWasAbsent = board.no_show === side || board.no_show === "both";
      return iWasAbsent
        ? { kind: "no-show", opponentName }
        : { kind: "no-show-win", opponentName };
    }
    if (!board.result) return { kind: "pending", opponent, opponentName };

    const { actualWon, gave } = boardOutcome(board, side);
    // Server-computed, Wiel-rule-aware winner for this board (see
    // `TournamentResponse.effective_winners`).
    const roundIdx = roundIndexByNumber.get(round.number);
    const effectiveWon =
      roundIdx != null ? effectiveWinners[roundIdx]?.[boardIdx] === side : actualWon;
    const handicap: PlayedCell["handicap"] = board.handicap
      ? { code: board.handicap.handicap, gave }
      : undefined;
    // points_diff = points(player1) − points(player2), frozen at pairing time.
    // From this player's own perspective: a positive diff for player1 means
    // player1 outranked player2, i.e. player1 played down (▼); the sign flips
    // for player2.
    const diff = board.points_diff ?? 0;
    const pointsDiff = isP1 ? diff : -diff;

    return {
      kind: "played",
      opponent,
      opponentName,
      actualWon,
      effectiveWon,
      drawn: board.drawn ?? false,
      handicap,
      pointsDiff,
    };
  }

  /** Ascending/descending floater markers, e.g. `^`, `vv` — one per point of gap. */
  function floatMarkers(cell: PlayedCell): string {
    if (cell.pointsDiff === 0) return "";
    const marker = cell.pointsDiff < 0 ? "^" : "v";
    return marker.repeat(Math.abs(cell.pointsDiff));
  }

  /** The results-cell label, e.g. `3+`, `4=−`, `3+(+4p)`, `4=+(−2p)^`. */
  function playedLabel(cell: PlayedCell): string {
    const draw = cell.drawn ? "=" : "";
    const sign = cell.actualWon ? "+" : "−";
    const hc = cell.handicap
      ? `(${cell.handicap.gave ? "−" : "+"}${cell.handicap.code})`
      : "";
    return `${cell.opponent}${draw}${sign}${hc}${floatMarkers(cell)}`;
  }

  /** Tooltip for the MacMahon cell: the breakdown of any manual bonus/penalty
   * adjustments, or nothing when the player has none (the column header already
   * says what the plain value is). */
  function macmahonTitle(player: Player): string | undefined {
    const adjustments = player.adjustments ?? [];
    if (adjustments.length === 0) return undefined;
    return adjustments
      .map((a) => `${a.delta > 0 ? "+" : ""}${a.delta} — ${a.reason}`)
      .join("\n");
  }

  /** A tie-break value, and the player who contributed it — `4 (Doe Jane)`. A
   * `half`-unit field (points, SOSM, …) is rendered `1½`; a whole one as-is. The
   * `count` multiplies the contribution, so a two-round (long) game — which
   * records the opponent twice — shows once with the doubled score. */
  function opponentTerm(id: string, field: keyof Standing, half: boolean, count = 1): string {
    const value = ((standingById.get(id)?.[field] as number | undefined) ?? 0) * count;
    return `${half ? formatScore(value) : value} (${nameOf(id)})`;
  }

  /** Group repeated opponent ids (a long game records the same opponent twice)
   * into one entry per opponent, keeping first-occurrence order. */
  function groupIds(ids: string[]): { id: string; count: number }[] {
    const out: { id: string; count: number }[] = [];
    for (const id of ids) {
      const existing = out.find((e) => e.id === id);
      if (existing) existing.count += 1;
      else out.push({ id, count: 1 });
    }
    return out;
  }

  /** Join the opponents' contributions to an opponent-sum tie-break, e.g.
   * `3 (Doe Jane) + 2 (Roe Max)`; a placeholder when there are none yet. A long
   * game's opponent appears once with its score doubled. */
  function sumTerms(ids: string[], field: keyof Standing, half: boolean): string {
    if (ids.length === 0) return $_("resultsView.tiebreakNoOpponents");
    return groupIds(ids)
      .map(({ id, count }) => opponentTerm(id, field, half, count))
      .join(" + ");
  }

  /** Like [`sumTerms`] but for the Buchholz-cut metrics: sort the opponents by
   * their contribution, drop the `drop` lowest (noting who), and sum the rest —
   * mirroring the server's `sum_dropping_lowest`. */
  function droppedTerms(ids: string[], field: keyof Standing, drop: number, half: boolean): string {
    if (ids.length === 0) return $_("resultsView.tiebreakNoOpponents");
    const terms = ids.map((id) => ({
      term: opponentTerm(id, field, half),
      value: (standingById.get(id)?.[field] as number | undefined) ?? 0,
    }));
    const { kept, dropped } = partitionDropped(terms, (t) => t.value, drop);
    const keptStr = kept.length > 0 ? kept.map((t) => t.term).join(" + ") : "0";
    if (dropped.length === 0) return keptStr;
    return `${keptStr} ${$_("resultsView.tiebreakDropped", { values: { dropped: dropped.map((t) => t.term).join(", ") } })}`;
  }

  /** The per-cell explanation of how a tie-break was computed, appended under
   * the metric's description in the tooltip. `undefined` where a breakdown
   * doesn't apply (Points, which has its own columns; the ELO estimate). */
  function tiebreakBreakdown(code: Tiebreak, standing: Standing): string | undefined {
    switch (code) {
      case "sos_m":
        return sumTerms(standing.opponents, "points", true);
      case "sos_w":
        return sumTerms(standing.opponents, "victories", false);
      case "sodos_m":
        return sumTerms(standing.defeated, "points", true);
      case "sodos_w":
        return sumTerms(standing.defeated, "victories", false);
      case "sosos_m":
        return sumTerms(standing.opponents, "sosm", true);
      case "sosos_w":
        return sumTerms(standing.opponents, "sosw", false);
      case "sos_m1":
        return droppedTerms(standing.opponents, "points", 1, true);
      case "sos_m2":
        return droppedTerms(standing.opponents, "points", 2, true);
      case "sos_w1":
        return droppedTerms(standing.opponents, "victories", 1, false);
      case "sos_w2":
        return droppedTerms(standing.opponents, "victories", 2, false);
      case "cuss_m":
        return standing.running_points.length > 0
          ? standing.running_points.map((p) => formatScore(p)).join(" + ")
          : undefined;
      case "cuss_w":
        return standing.running_wins.length > 0
          ? standing.running_wins.join(" + ")
          : undefined;
      case "points":
      case "est_elo":
        return undefined;
    }
  }

  /** The full tooltip for a tie-break cell: the metric's description, plus its
   * per-player breakdown when one applies. */
  function tiebreakCellTitle(code: Tiebreak, standing: Standing): string {
    const title = tiebreakTitle(code, $_);
    const breakdown = tiebreakBreakdown(code, standing);
    return breakdown ? `${title}\n${breakdown}` : title;
  }

  // Native `title` tooltips only surface after a long browser delay. Drive our
  // own instead: one hover handler on the table reads the `data-tip` of the cell
  // under the cursor and shows a floating label immediately, following the mouse.
  let tip = $state<{ text: string; x: number; y: number; below: boolean } | null>(null);

  function trackTip(event: MouseEvent) {
    const el = (event.target as HTMLElement | null)?.closest?.(
      "[data-tip]",
    ) as HTMLElement | null;
    const text = el?.dataset.tip;
    if (!text) {
      tip = null;
      return;
    }
    // Clamp horizontally so a wide label stays on-screen; flip above the cursor
    // near the bottom edge so a tall breakdown isn't clipped.
    tip = {
      text,
      x: Math.max(8, Math.min(event.clientX + 14, window.innerWidth - 348)),
      y: event.clientY,
      below: event.clientY < window.innerHeight - 220,
    };
  }

  function clearTip() {
    tip = null;
  }
</script>

{#if tournament.players.length === 0}
  <p class="muted">{$_("resultsView.noPlayers")}</p>
{:else}
  <div class="results">
  <div class="results-toolbar print-hide">
    <button type="button" class="ghost" onclick={() => printPage(true)}>🖨 {$_("roundView.print")}</button>
  </div>
  {#if cupPodium && (cupPodium.champion || cupPodium.runner_up || cupPodium.third)}
    <div class="podium">
      <span class="cup-title">{$_("resultsView.cup")}</span>
      {#if cupPodium.champion}
        <span>🥇 <strong>{nameOf(cupPodium.champion)}</strong></span>
      {/if}
      {#if cupPodium.runner_up}
        <span>🥈 {nameOf(cupPodium.runner_up)}</span>
      {/if}
      {#if cupPodium.third}
        <span>🥉 {nameOf(cupPodium.third)}</span>
      {/if}
    </div>
  {/if}
  <table onmousemove={trackTip} onmouseleave={clearTip}>
    <thead>
      <tr>
        <th class="num">{$_("resultsView.id")}</th>
        <th>{$_("resultsView.lastName")}</th>
        <th>{$_("resultsView.firstName")}</th>
        <th class="num">{$_("resultsView.rating")}</th>
        {#if showEstimatedElo}
          <th class="num" data-tip={tiebreakTitle("est_elo", $_)}>{tiebreakLabel("est_elo", $_)}</th>
        {/if}
        <th>{$_("resultsView.nationality")}</th>
        <th>{$_("resultsView.club")}</th>
        {#each completedRounds as round (round.number)}
          <th class="num">{$_("resultsView.roundColumn", { values: { number: round.number } })}</th>
        {/each}
        <th class="num">{$_("resultsView.victories")}</th>
        {#if showMacmahon}
          <th class="num" data-tip={$_("resultsView.macmahonTitle")}>{$_("resultsView.macmahon")}</th>
        {/if}
        {#each tiebreakColumns as col (col.code)}
          <th class="num" data-tip={tiebreakTitle(col.code, $_)}>{tiebreakLabel(col.code, $_)}</th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each rows as { standing, player } (player.id)}
        <tr>
          <td class="num">{player.tournament_id ?? "—"}</td>
          <td>{#if medalOf.has(player.id)}<span class="medal">{medalOf.get(player.id)}</span> {/if}{player.last_name}</td>
          <td>{player.first_name || "—"}</td>
          <td class="num">{player.rating ?? "—"}</td>
          {#if showEstimatedElo}
            <td class="num est-elo">{standing.estimated_elo}</td>
          {/if}
          <td>{player.nationality ?? "—"}</td>
          <td>{player.club ?? "—"}</td>
          {#each completedRounds as round, i (round.number)}
            {@const cell = longAwareCell(player, i)}
            <td class="num result">
              {#if cell.kind === "bye"}
                <span class="win" data-tip={$_("resultsView.byeTitle")}>0+</span>
              {:else if cell.kind === "long-pending"}
                <span
                  class="pending"
                  data-tip={$_("resultsView.longPendingTitle", {
                    values: { name: cell.opponentName },
                  })}>0−</span
                >
              {:else if cell.kind === "absent"}
                {#if cell.half}
                  <span class="absent" data-tip={$_("resultsView.halfByeTitle")}>0=</span>
                {:else}
                  <span class="absent" data-tip={$_("resultsView.absentTitle")}>0−</span>
                {/if}
              {:else if cell.kind === "no-show"}
                <span
                  class="absent"
                  data-tip={$_("resultsView.noShowTitle", { values: { name: cell.opponentName } })}
                  >0#</span
                >
              {:else if cell.kind === "no-show-win"}
                <span
                  class="win"
                  data-tip={$_("resultsView.noShowWinTitle", { values: { name: cell.opponentName } })}
                  >0+</span
                >
              {:else if cell.kind === "pending"}
                <span
                  class="pending"
                  data-tip={$_("resultsView.opponentTitle", { values: { name: cell.opponentName } })}
                  >{cell.opponent}?</span
                >
              {:else}
                {@const opponentTitle = $_("resultsView.opponentTitle", { values: { name: cell.opponentName } })}
                <span
                  class={cell.actualWon ? "win" : "loss"}
                  data-tip={cell.handicap && cell.effectiveWon !== cell.actualWon
                    ? `${opponentTitle}\n${$_(
                        cell.effectiveWon
                          ? "resultsView.handicapGameWin"
                          : "resultsView.handicapGameLoss",
                      )}`
                    : opponentTitle}>{playedLabel(cell)}</span
                >
              {/if}
            </td>
          {/each}
          <td class="num victories">{standing.victories}</td>
          {#if showMacmahon}
            <td
              class="num macmahon"
              class:adjusted={(player.adjustments ?? []).length > 0}
              data-tip={macmahonTitle(player)}>{formatScore(standing.macmahon)}</td
            >
          {/if}
          {#each tiebreakColumns as col (col.code)}
            {#if col.code === "points"}
              <td class="num points" data-tip={$_("resultsView.victoriesPlusMacmahon")}
                >{formatScore(standing.points)}</td
              >
            {:else}
              <td class="num tiebreak" data-tip={tiebreakCellTitle(col.code, standing)}
                >{HALF_POINT_TIEBREAKS.has(col.code)
                  ? formatScore(standing[col.field] as number)
                  : standing[col.field]}</td
              >
            {/if}
          {/each}
        </tr>
      {/each}
    </tbody>
  </table>

  {#if completedRounds.length === 0}
    <p class="muted note">
      {$_("resultsView.noRoundsCompleted")}
    </p>
  {/if}
  </div>
{/if}

{#if tip}
  <div
    class="cell-tip print-hide"
    style="left: {tip.x}px; top: {tip.y}px; transform: translateY({tip.below
      ? '18px'
      : 'calc(-100% - 14px)'});"
  >
    {tip.text}
  </div>
{/if}

<style>
  .results-toolbar {
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
    padding: 0.3rem 0.55rem;
    border-bottom: 1px solid var(--border-divider);
    text-align: left;
    white-space: nowrap;
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
    font-variant-numeric: tabular-nums;
  }
  .result {
    font-variant-numeric: tabular-nums;
  }
  .win {
    color: var(--color-success);
  }
  .loss {
    color: var(--color-danger);
  }
  .absent {
    color: var(--text-tertiary);
  }
  .pending {
    color: var(--color-warning);
  }
  .victories {
    font-weight: 600;
  }
  .points {
    font-weight: 700;
    color: var(--color-purple);
  }
  .macmahon {
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
  }
  .macmahon.adjusted {
    text-decoration: underline dotted;
    text-underline-offset: 0.2rem;
  }
  .tiebreak {
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
  }
  .est-elo {
    color: var(--color-accent-strong);
    font-variant-numeric: tabular-nums;
  }
  .podium {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.9rem;
    padding: 0.5rem 0.75rem;
    border: 1px solid var(--border-warning);
    background: var(--bg-warning);
    border-radius: 0.5rem;
    font-size: 0.9rem;
  }
  .cup-title {
    font-weight: 700;
    color: var(--color-warning-strong);
    text-transform: uppercase;
    font-size: 0.75rem;
    letter-spacing: 0.05em;
  }
  .medal {
    font-size: 0.85rem;
  }
  .cell-tip {
    position: fixed;
    z-index: 1000;
    max-width: 340px;
    padding: 0.35rem 0.55rem;
    border-radius: 0.4rem;
    background: var(--text);
    color: var(--bg-surface);
    font-size: 0.78rem;
    line-height: 1.4;
    white-space: pre-line;
    pointer-events: none;
    box-shadow: 0 4px 14px var(--shadow-dropdown);
  }
  .muted {
    color: var(--text-secondary);
  }
  .note {
    font-size: 0.85rem;
    margin-top: 0.75rem;
    text-align: center;
  }

  /* A named page so only this tab's print job goes landscape — the table is
     wide (one column per round plus the tie-breaks), unlike the pairings view. */
  @page results-landscape {
    size: landscape;
  }

  @media print {
    .print-hide {
      display: none;
    }
    .results {
      page: results-landscape;
    }
    table,
    th,
    td,
    .win,
    .loss,
    .absent,
    .pending,
    .points,
    .macmahon,
    .tiebreak,
    .est-elo,
    .muted,
    .note,
    .podium,
    .cup-title {
      color: #000 !important;
      background: transparent !important;
      border-color: #000 !important;
    }
    tbody tr:nth-child(even) {
      background: transparent !important;
    }
  }
</style>
