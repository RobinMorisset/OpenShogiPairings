<script lang="ts">
  import type { CupPodium, Player, Round, Standing, Tournament } from "../types";

  interface Props {
    tournament: Tournament;
    /** Ranked standings computed server-side (the canonical ordering). */
    standings: Standing[];
    /** The cup podium, once decided (adds medals; doesn't reorder the table). */
    cupPodium?: CupPodium | null;
  }

  let { tournament, standings, cupPodium = null }: Props = $props();

  // Player id → medal, from the cup podium (the table order stays pure-Swiss).
  const medalOf = $derived.by(() => {
    const m = new Map<string, string>();
    if (cupPodium) {
      m.set(cupPodium.champion, "🥇");
      m.set(cupPodium.runner_up, "🥈");
      m.set(cupPodium.third, "🥉");
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
    | { kind: "absent" }
    | { kind: "pending"; opponent: string }
    | PlayedCell;

  function cellFor(player: Player, round: Round): Cell {
    if (round.bye === player.id) return { kind: "bye" };
    const board = round.boards.find(
      (b) => b.player1 === player.id || b.player2 === player.id,
    );
    if (!board) return { kind: "absent" };
    const isP1 = board.player1 === player.id;
    const opponentId = numberOf.get(isP1 ? board.player2 : board.player1);
    const opponent = opponentId != null ? String(opponentId) : "?";
    if (!board.result) return { kind: "pending", opponent };
    const actualWon =
      (board.result === "player1" && isP1) ||
      (board.result === "player2" && !isP1);

    // A handicap game always counts as a win for the giver, whoever won.
    let effectiveWon = actualWon;
    let handicap: PlayedCell["handicap"];
    if (board.handicap) {
      const gave =
        (board.handicap.giver === "player1" && isP1) ||
        (board.handicap.giver === "player2" && !isP1);
      effectiveWon = gave;
      handicap = { code: board.handicap.handicap, gave };
    }
    // points_diff = points(player1) − points(player2), frozen at pairing time.
    // From this player's own perspective: a positive diff for player1 means
    // player1 outranked player2, i.e. player1 played down (▼); the sign flips
    // for player2.
    const diff = board.points_diff ?? 0;
    const pointsDiff = isP1 ? diff : -diff;

    return {
      kind: "played",
      opponent,
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

  /** Tooltip for the Points cell: the breakdown of any manual adjustments, or
   * the default explanation when the player has none. */
  function pointsTitle(player: Player): string {
    const adjustments = player.adjustments ?? [];
    if (adjustments.length === 0) return "Victories + MacMahon points";
    return adjustments
      .map((a) => `${a.delta > 0 ? "+" : ""}${a.delta} — ${a.reason}`)
      .join("\n");
  }
</script>

{#if tournament.players.length === 0}
  <p class="muted">No players registered yet.</p>
{:else}
  <div class="results">
  <div class="results-toolbar print-hide">
    <button type="button" class="ghost" onclick={() => window.print()}>🖨 Print</button>
  </div>
  {#if cupPodium}
    {@const nameOf = (id: string) => {
      const p = byId.get(id);
      return p ? `${p.last_name} ${p.first_name}`.trim() : "—";
    }}
    <div class="podium">
      <span class="cup-title">Cup</span>
      <span>🥇 <strong>{nameOf(cupPodium.champion)}</strong></span>
      <span>🥈 {nameOf(cupPodium.runner_up)}</span>
      <span>🥉 {nameOf(cupPodium.third)}</span>
    </div>
  {/if}
  <table>
    <thead>
      <tr>
        <th class="num">ID</th>
        <th>Last name</th>
        <th>First name</th>
        <th class="num">Rating</th>
        <th>Nat.</th>
        <th>Club</th>
        {#each completedRounds as round (round.number)}
          <th class="num">R{round.number}</th>
        {/each}
        <th class="num">Victories</th>
        <th class="num">Points</th>
        <th class="num" title="Sum of opponents' scores">SOS</th>
        <th class="num" title="Sum of defeated opponents' scores">SODOS</th>
        <th class="num" title="Sum of opponents' SOS">SOSOS</th>
      </tr>
    </thead>
    <tbody>
      {#each rows as { standing, player } (player.id)}
        <tr>
          <td class="num">{player.tournament_id ?? "—"}</td>
          <td>{#if medalOf.has(player.id)}<span class="medal">{medalOf.get(player.id)}</span> {/if}{player.last_name}</td>
          <td>{player.first_name || "—"}</td>
          <td class="num">{player.rating ?? "—"}</td>
          <td>{player.nationality ?? "—"}</td>
          <td>{player.club ?? "—"}</td>
          {#each completedRounds as round (round.number)}
            {@const cell = cellFor(player, round)}
            <td class="num result">
              {#if cell.kind === "bye"}
                <span class="win" title="Bye (counts as a win)">0+</span>
              {:else if cell.kind === "absent"}
                <span class="absent" title="Absent (counts as a loss)">0−</span>
              {:else if cell.kind === "pending"}
                <span class="pending">{cell.opponent}?</span>
              {:else}
                <span
                  class={cell.actualWon ? "win" : "loss"}
                  title={cell.handicap
                    ? `Handicap game — counts as a ${cell.effectiveWon ? "win" : "loss"} in the standings`
                    : undefined}>{playedLabel(cell)}</span
                >
              {/if}
            </td>
          {/each}
          <td class="num victories">{standing.victories}</td>
          <td
            class="num points"
            class:adjusted={(player.adjustments ?? []).length > 0}
            title={pointsTitle(player)}>{standing.points}</td
          >
          <td class="num tiebreak">{standing.sos}</td>
          <td class="num tiebreak">{standing.sodos}</td>
          <td class="num tiebreak">{standing.sosos}</td>
        </tr>
      {/each}
    </tbody>
  </table>

  {#if completedRounds.length === 0}
    <p class="muted note">
      No rounds completed yet — results appear here as rounds are completed.
    </p>
  {/if}
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
    border-bottom: 1px solid #2b2b31;
    text-align: left;
    white-space: nowrap;
  }
  th {
    color: #9a9aa2;
    font-weight: 600;
    font-size: 0.8rem;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .result {
    font-variant-numeric: tabular-nums;
  }
  .win {
    color: #3fb950;
  }
  .loss {
    color: #f85149;
  }
  .absent {
    color: #6a6a72;
  }
  .pending {
    color: #d29922;
  }
  .victories {
    font-weight: 600;
  }
  .points {
    font-weight: 700;
    color: #d2a8ff;
  }
  .points.adjusted {
    text-decoration: underline dotted;
    text-underline-offset: 0.2rem;
  }
  .tiebreak {
    color: #9a9aa2;
    font-variant-numeric: tabular-nums;
  }
  .podium {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.9rem;
    padding: 0.5rem 0.75rem;
    border: 1px solid #5a4711;
    background: #2a2410;
    border-radius: 0.5rem;
    font-size: 0.9rem;
  }
  .cup-title {
    font-weight: 700;
    color: #e3b341;
    text-transform: uppercase;
    font-size: 0.75rem;
    letter-spacing: 0.05em;
  }
  .medal {
    font-size: 0.85rem;
  }
  .muted {
    color: #9a9aa2;
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
    .tiebreak,
    .muted,
    .note,
    .podium,
    .cup-title {
      color: #000 !important;
      background: transparent !important;
      border-color: #000 !important;
    }
  }
</style>
