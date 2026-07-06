<script lang="ts">
  import type { Player, Round, Standing, Tournament } from "../types";

  interface Props {
    tournament: Tournament;
    /** Ranked standings computed server-side (the canonical ordering). */
    standings: Standing[];
  }

  let { tournament, standings }: Props = $props();

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
    return {
      kind: "played",
      opponent,
      actualWon,
      effectiveWon,
      drawn: board.drawn ?? false,
      handicap,
    };
  }

  /** The results-cell label, e.g. `3+`, `4=−`, `3+(+4p)`, `4=+(−2p)`. */
  function playedLabel(cell: PlayedCell): string {
    const draw = cell.drawn ? "=" : "";
    const sign = cell.actualWon ? "+" : "−";
    const hc = cell.handicap
      ? `(${cell.handicap.gave ? "−" : "+"}${cell.handicap.code})`
      : "";
    return `${cell.opponent}${draw}${sign}${hc}`;
  }
</script>

{#if tournament.players.length === 0}
  <p class="muted">No players registered yet.</p>
{:else}
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
          <td>{player.last_name}</td>
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
                <span class="loss" title="Absent (counts as a loss)">0−</span>
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
          <td class="num points" title="Victories + MacMahon points">{standing.points}</td>
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
{/if}

<style>
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
  .tiebreak {
    color: #9a9aa2;
    font-variant-numeric: tabular-nums;
  }
  .muted {
    color: #9a9aa2;
  }
  .note {
    font-size: 0.85rem;
    margin-top: 0.75rem;
    text-align: center;
  }
</style>
