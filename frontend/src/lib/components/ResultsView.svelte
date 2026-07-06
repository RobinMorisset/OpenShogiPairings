<script lang="ts">
  import type { Player, Round, Tournament } from "../types";

  interface Props {
    tournament: Tournament;
  }

  let { tournament }: Props = $props();

  // One column per completed round.
  const completedRounds = $derived(tournament.rounds.filter((r) => r.completed));

  // Map player UUID → their tournament number, for opponent references.
  const numberOf = $derived(
    new Map(tournament.players.map((p) => [p.id, p.tournament_id])),
  );

  // Rows ordered by tournament number (unnumbered players last).
  const rows = $derived(
    [...tournament.players].sort(
      (a, b) => (a.tournament_id ?? Infinity) - (b.tournament_id ?? Infinity),
    ),
  );

  type Cell =
    | { kind: "bye" }
    | { kind: "absent" }
    | { kind: "pending"; opponent: string }
    | { kind: "played"; opponent: string; won: boolean };

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
    const won =
      (board.result === "player1" && isP1) ||
      (board.result === "player2" && !isP1);
    return { kind: "played", opponent, won };
  }

  function victories(player: Player): number {
    let count = 0;
    for (const round of completedRounds) {
      const cell = cellFor(player, round);
      if (cell.kind === "played" && cell.won) count++;
    }
    return count;
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
      </tr>
    </thead>
    <tbody>
      {#each rows as player (player.id)}
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
                <span class="bye">bye</span>
              {:else if cell.kind === "absent"}
                <span class="absent">·</span>
              {:else if cell.kind === "pending"}
                <span class="pending">{cell.opponent}?</span>
              {:else}
                <span class={cell.won ? "win" : "loss"}
                  >{cell.opponent}{cell.won ? "+" : "−"}</span
                >
              {/if}
            </td>
          {/each}
          <td class="num victories">{victories(player)}</td>
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
  .bye,
  .absent {
    color: #6a6a72;
  }
  .victories {
    font-weight: 600;
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
