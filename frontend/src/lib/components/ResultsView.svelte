<script lang="ts">
  import { _ } from "svelte-i18n";
  import { TIEBREAKS } from "../types";
  import type {
    Board,
    CupPodium,
    Player,
    PlayerCategory,
    Round,
    Sitout,
    SitoutValue,
    Standing,
    Tiebreak,
    Tournament,
    Winner,
  } from "../types";
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
    /** Referee-defined categories, for the highlight filter and leader marks. */
    categories?: PlayerCategory[];
    /** Re-score one player's sit-out in one round. Omitted for a read-only
     *  table, which leaves the sit-out cells as plain text. */
    onSetSitoutValue?: (roundNumber: number, player: number, value: SitoutValue) => void;
  }

  let {
    tournament,
    standings,
    cupPodium = null,
    effectiveWinners,
    categories = [],
    onSetSitoutValue,
  }: Props = $props();

  // Round number → its index into `tournament.rounds` (and so into
  // `effectiveWinners`), since that array isn't filtered to completed rounds.
  const roundIndexByNumber = $derived(
    new Map(tournament.rounds.map((r, i) => [r.number, i])),
  );

  // The tie-break columns to show, in the referee-chosen order — resolved from
  // the settings to their label/field/tooltip. Unknown codes (from a newer save)
  // are skipped.
  // A live ELO estimate is maintained — so the estimate is a meaningful ranking
  // and column — in either of the server's two cases: ELO pairing mode, or Swiss
  // with MacMahon drawn from the estimate against at least one ELO threshold.
  // Mirrors the server's `elo_estimate_needed() || macmahon_from_estimate_active()`
  // (and TournamentSettingsView's `eloEstimateLive`) — keep the three in sync, or
  // the estimate the server ranks by would be hidden here.
  const eloEstimateNeeded = $derived.by(() => {
    const pairing = tournament.settings.pairing;
    if (pairing.kind === "elo") return true;
    return (
      pairing.macmahon.source.kind === "from_estimate" &&
      pairing.macmahon.thresholds.some((t) => t.criterion.kind === "elo")
    );
  });

  const tiebreakColumns = $derived(
    (tournament.settings.tiebreaks ?? [])
      // Estimated ELO only ranks when a live estimate is maintained; drop it as a
      // column otherwise (defends against a loaded save that still lists it).
      .filter((code) => code !== "est_elo" || eloEstimateNeeded)
      .map((code) => TIEBREAKS.find((t) => t.code === code))
      .filter((t): t is (typeof TIEBREAKS)[number] => t != null),
  );

  // The estimated-ELO column is only meaningful when a live estimate is
  // maintained. Show it as a dedicated column there — unless the referee already
  // added it to the ranking criteria, in which case it appears as a tie-break
  // column (with its ranking position) and this one would duplicate it.
  const showEstimatedElo = $derived(
    eloEstimateNeeded && !(tournament.settings.tiebreaks ?? []).includes("est_elo"),
  );

  // Player id → medal, from the cup podium (the table order stays pure-Swiss).
  // A place can be null when a double no-show left it undetermined (e.g. both
  // finalists absent), so each medal is awarded only if its winner exists.
  // Cup podium places are tournament numbers, not registration ids.
  const medalOf = $derived.by(() => {
    const m = new Map<number, string>();
    if (cupPodium) {
      if (cupPodium.champion != null) m.set(cupPodium.champion, "🥇");
      if (cupPodium.runner_up != null) m.set(cupPodium.runner_up, "🥈");
      if (cupPodium.third != null) m.set(cupPodium.third, "🥉");
    }
    return m;
  });

  // One column per completed round.
  const completedRounds = $derived(tournament.rounds.filter((r) => r.completed));

  // Rows follow the server's ranked order, joined to each player's details.
  const byId = $derived(new Map(tournament.players.map((p) => [p.id, p])));

  // Boards, byes and the cup podium all reference players by their tournament
  // number (`Player.tournament_id`), not the registration id — this maps
  // number → player for those lookups. Rounds only exist once registration is
  // finalized, at which point every player has a number.
  const byTid = $derived(
    new Map(
      tournament.players
        .filter((p): p is Player & { tournament_id: number } => p.tournament_id != null)
        .map((p) => [p.tournament_id, p]),
    ),
  );

  /** A player's tournament number — guaranteed present once rounds exist. */
  function tid(p: Player): number {
    return p.tournament_id!;
  }

  // Standings keyed by player id, so a cell can look up an opponent's metrics
  // when explaining how a tie-break was summed.
  const standingById = $derived(new Map(standings.map((s) => [s.player_id, s])));

  // A player's full name for a tooltip, "Last First"; an em dash if unknown
  // (e.g. an opponent removed after the game).
  const nameOf = (id: string): string => {
    const p = byId.get(id);
    return p ? `${p.last_name} ${p.first_name}`.trim() : "—";
  };

  // Same as `nameOf`, but for a player referenced by tournament number.
  const nameOfTid = (id: number): string => {
    const p = byTid.get(id);
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

  // The category leaders: for each category, the highest-ranked player in it
  // (rows are already in the server's ranked order). Keyed by player id → the
  // names of every category that player tops, so one ⭐ carries a tooltip
  // listing them all (a player can lead more than one).
  const categoryLeaders = $derived.by(() => {
    const leaders = new Map<string, string[]>();
    for (const cat of categories) {
      const leader = rows.find(({ player }) => (player.categories ?? []).includes(cat.id));
      if (!leader) continue;
      const led = leaders.get(leader.player.id) ?? [];
      led.push(cat.name);
      leaders.set(leader.player.id, led);
    }
    return leaders;
  });

  // The one category the referee has selected to highlight, or null for none —
  // highlighting two at once isn't useful, so picking a category replaces any
  // previous choice.
  let highlightedCategory = $state<string | null>(null);
  const filtering = $derived(highlightedCategory !== null);

  // Drop the selection if it points at a now-deleted category, so filtering
  // never stays "active" with nothing left to highlight.
  $effect(() => {
    const valid = new Set(categories.map((c) => c.id));
    if (highlightedCategory !== null && !valid.has(highlightedCategory)) {
      highlightedCategory = null;
    }
  });

  function toggleHighlight(id: string) {
    highlightedCategory = highlightedCategory === id ? null : id;
  }

  // Whether the player belongs to the currently-highlighted category.
  function isHighlighted(player: Player): boolean {
    return highlightedCategory !== null && (player.categories ?? []).includes(highlightedCategory);
  }

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

  // A round the player sat out — a bye of some sort, or an absence — showing
  // what it scored (`0+` / `0=` / `0−`). The only editable cell: clicking it
  // re-scores that one round for that one player.
  type SitoutCell = {
    kind: "sitout";
    /** Which round it belongs to, for the re-scoring call. */
    roundNumber: number;
    player: number;
    sitout: Sitout;
  };

  type Cell =
    | SitoutCell
    // A no-show board: this player was the absentee (`0#`) or the one who
    // showed up and was credited the free point, bye-style (`0+`).
    | { kind: "no-show"; opponentName: string }
    | { kind: "no-show-win"; opponentName: string }
    | { kind: "pending"; opponent: string; opponentName: string }
    // A long (two-round) game started this round: its result belongs to the next
    // round's column, so this column shows a `0−` placeholder.
    | { kind: "long-pending"; opponentName: string }
    // The player wasn't in this round at all (e.g. registered later).
    | { kind: "not-in-round" }
    | PlayedCell;

  // A long (two-round) game lives in its starting round but its result is only
  // known in the next round, so it shows a `0−` placeholder in the starting
  // column and its result in the following column — matching the American Grid.
  function longAwareCell(player: Player, i: number): Cell {
    const pid = tid(player);
    const onLong = (round: Round) =>
      round.boards.find((b) => b.long && (b.player1 === pid || b.player2 === pid));
    const here = onLong(completedRounds[i]);
    if (here) {
      const opp = here.player1 === pid ? here.player2 : here.player1;
      return { kind: "long-pending", opponentName: nameOfTid(opp) };
    }
    if (i > 0) {
      const prev = completedRounds[i - 1];
      const idx = prev.boards.findIndex(
        (b) => b.long && (b.player1 === pid || b.player2 === pid),
      );
      if (idx >= 0) return cellForBoard(player, prev, prev.boards[idx], idx);
    }
    return cellFor(player, completedRounds[i]);
  }

  function cellFor(player: Player, round: Round): Cell {
    const pid = tid(player);
    // No board: a bye or an absence, showing whatever it was scored.
    const sitout = (round.sitouts ?? []).find((s) => s.player === pid);
    if (sitout)
      return { kind: "sitout", roundNumber: round.number, player: pid, sitout };
    const boardIdx = round.boards.findIndex((b) => b.player1 === pid || b.player2 === pid);
    if (boardIdx < 0) return { kind: "not-in-round" };
    return cellForBoard(player, round, round.boards[boardIdx], boardIdx);
  }

  /** The cross-table cell for a specific board (played / no-show / pending). */
  function cellForBoard(player: Player, round: Round, board: Board, boardIdx: number): Cell {
    const isP1 = board.player1 === tid(player);
    const side: Winner = isP1 ? "player1" : "player2";
    const opponentTid = isP1 ? board.player2 : board.player1;
    const opponent = String(opponentTid);
    const opponentName = nameOfTid(opponentTid);
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

  /** The three values a sit-out can take, in the order the picker lists them. */
  const SITOUT_VALUES: SitoutValue[] = ["full", "half", "zero"];

  /** The cross-table token for a sit-out value: `0+`, `0=` or `0−`. */
  function sitoutLabel(value: SitoutValue): string {
    return { full: "0+", half: "0=", zero: "0−" }[value];
  }

  /** Whether a sit-out is a bye of some kind (as opposed to an absence) — it
   *  colours the cell and names it in the tooltip. */
  function isBye(sitout: Sitout): boolean {
    return sitout.kind !== "absent";
  }

  /** The tooltip for a sit-out cell: why the player sat out, what it scored,
   *  and (when editable) that it can be clicked. */
  function sitoutTitle(cell: SitoutCell): string {
    const kind = cell.sitout.kind;
    const reason =
      kind === "absent"
        ? $_("resultsView.sitoutAbsent")
        : kind === "forced_bye"
          ? $_("resultsView.sitoutForcedBye")
          : kind === "bye"
            ? $_("resultsView.sitoutBye")
            : $_("resultsView.sitoutCupBye");
    const worth = $_(`resultsView.sitoutWorth.${cell.sitout.value}`);
    return onSetSitoutValue ? `${reason}\n${worth}\n${$_("resultsView.sitoutEditHint")}` : `${reason}\n${worth}`;
  }

  // The sit-out cell whose picker is open, if any. Keyed by round + player,
  // since that pair identifies a cell uniquely.
  let picking = $state<{ roundNumber: number; player: number } | null>(null);

  const isPicking = (cell: SitoutCell) =>
    picking?.roundNumber === cell.roundNumber && picking?.player === cell.player;

  function togglePicker(cell: SitoutCell) {
    picking = isPicking(cell)
      ? null
      : { roundNumber: cell.roundNumber, player: cell.player };
  }

  function chooseValue(cell: SitoutCell, value: SitoutValue) {
    picking = null;
    if (value !== cell.sitout.value) {
      onSetSitoutValue?.(cell.roundNumber, cell.player, value);
    }
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
        <span>🥇 <strong>{nameOfTid(cupPodium.champion)}</strong></span>
      {/if}
      {#if cupPodium.runner_up}
        <span>🥈 {nameOfTid(cupPodium.runner_up)}</span>
      {/if}
      {#if cupPodium.third}
        <span>🥉 {nameOfTid(cupPodium.third)}</span>
      {/if}
    </div>
  {/if}
  {#if categories.length > 0}
    <div class="category-filter print-hide">
      <span class="filter-label">{$_("resultsView.highlightCategory")}</span>
      {#each categories as cat (cat.id)}
        <button
          type="button"
          class="cat-chip"
          class:active={highlightedCategory === cat.id}
          onclick={() => toggleHighlight(cat.id)}
        >{cat.name}</button>
      {/each}
      {#if filtering}
        <button
          type="button"
          class="cat-chip clear"
          onclick={() => (highlightedCategory = null)}
        >{$_("resultsView.clearHighlight")}</button>
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
        <tr class:cat-highlight={filtering && isHighlighted(player)} class:cat-dim={filtering && !isHighlighted(player)}>
          <td class="num">{player.tournament_id ?? "—"}</td>
          <td>{#if player.tournament_id != null && medalOf.has(player.tournament_id)}<span class="medal">{medalOf.get(player.tournament_id)}</span> {/if}{#if categoryLeaders.has(player.id)}<span class="cat-star" title={$_("resultsView.categoryLeader", { values: { categories: categoryLeaders.get(player.id)?.join(", ") } })}>⭐</span> {/if}{player.last_name}</td>
          <td>{player.first_name || "—"}</td>
          <td class="num">{player.rating ?? "—"}</td>
          {#if showEstimatedElo}
            <td class="num est-elo">{standing.estimated_elo ?? "—"}</td>
          {/if}
          <td>{player.nationality ?? "—"}</td>
          <td>{player.club ?? "—"}</td>
          {#each completedRounds as round, i (round.number)}
            {@const cell = longAwareCell(player, i)}
            <td class="num result">
              {#if cell.kind === "sitout"}
                {@const tone = isBye(cell.sitout) && cell.sitout.value === "full" ? "win" : "absent"}
                {#if onSetSitoutValue}
                  <span class="sitout-cell">
                    <button
                      type="button"
                      class="sitout {tone}"
                      data-testid="sitout-{cell.roundNumber}-{cell.player}"
                      aria-haspopup="true"
                      aria-expanded={isPicking(cell)}
                      data-tip={sitoutTitle(cell)}
                      onclick={() => togglePicker(cell)}>{sitoutLabel(cell.sitout.value)}</button
                    >
                    {#if isPicking(cell)}
                      <span class="sitout-picker print-hide" role="menu">
                        {#each SITOUT_VALUES as value (value)}
                          <button
                            type="button"
                            role="menuitem"
                            class:current={value === cell.sitout.value}
                            data-testid="sitout-option-{value}"
                            data-tip={$_(`resultsView.sitoutWorth.${value}`)}
                            onclick={() => chooseValue(cell, value)}>{sitoutLabel(value)}</button
                          >
                        {/each}
                      </span>
                    {/if}
                  </span>
                {:else}
                  <span class={tone} data-tip={sitoutTitle(cell)}>{sitoutLabel(cell.sitout.value)}</span>
                {/if}
              {:else if cell.kind === "not-in-round"}
                <span class="absent" data-tip={$_("resultsView.notInRoundTitle")}>0−</span>
              {:else if cell.kind === "long-pending"}
                <span
                  class="pending"
                  data-tip={$_("resultsView.longPendingTitle", {
                    values: { name: cell.opponentName },
                  })}>0−</span
                >
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
  /* A sit-out cell is a button, but reads as the plain token until hovered. */
  .sitout-cell {
    position: relative;
    display: inline-block;
  }
  button.sitout {
    padding: 0 0.15rem;
    border: 1px solid transparent;
    border-radius: 0.25rem;
    background: none;
    font: inherit;
    font-variant-numeric: tabular-nums;
    color: inherit;
    cursor: pointer;
  }
  button.sitout:hover,
  button.sitout[aria-expanded="true"] {
    border-color: var(--border-divider);
    background: var(--bg-stripe);
  }
  .sitout-picker {
    position: absolute;
    z-index: 900;
    top: calc(100% + 0.2rem);
    right: 0;
    display: flex;
    gap: 0.15rem;
    padding: 0.2rem;
    border: 1px solid var(--border-divider);
    border-radius: 0.35rem;
    background: var(--bg-surface);
    box-shadow: 0 4px 14px var(--shadow-dropdown);
  }
  .sitout-picker button {
    padding: 0.1rem 0.35rem;
    border: 1px solid transparent;
    border-radius: 0.25rem;
    background: none;
    font: inherit;
    font-variant-numeric: tabular-nums;
    color: var(--text);
    cursor: pointer;
  }
  .sitout-picker button:hover {
    background: var(--bg-stripe);
  }
  .sitout-picker button.current {
    border-color: var(--color-accent-strong);
    font-weight: 700;
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
  .cat-star {
    font-size: 0.85rem;
    cursor: help;
  }
  .category-filter {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
    margin-bottom: 0.6rem;
  }
  .filter-label {
    font-size: 0.85rem;
    color: var(--text-secondary);
  }
  .cat-chip {
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border-soft);
    border-radius: 999px;
    background: var(--bg-inset);
    color: inherit;
    font: inherit;
    font-size: 0.82rem;
    cursor: pointer;
  }
  .cat-chip:hover {
    background: var(--bg-hover);
  }
  .cat-chip.active {
    background: var(--bg-accent);
    border-color: var(--border-accent-strong);
    color: var(--text-on-accent);
  }
  .cat-chip.clear {
    border-style: dashed;
  }
  tr.cat-dim {
    opacity: 0.35;
  }
  /* Override the zebra stripe (tbody tr:nth-child(even)) so every highlighted
     row is the same color regardless of its odd/even position. */
  tbody tr.cat-highlight {
    background: var(--bg-hover-strong);
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
    /* The sit-out token is a <button> carrying its tone class (.win/.absent),
       both of which force border-color: #000 above. Keep its border invisible
       in print so no square is drawn around the 0+ / 0= / 0− tokens. */
    button.sitout {
      border-color: transparent !important;
    }
    /* The medals (🥇🥈🥉) and category-leader star (⭐) are colour emoji, whose
       glyph colour ignores `color: #000`. Desaturate them so the printout stays
       black & white while still marking podium places and category leaders. */
    .medal,
    .cat-star,
    .podium {
      filter: grayscale(1);
    }
  }
</style>
