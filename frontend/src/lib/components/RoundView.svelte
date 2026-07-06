<script lang="ts">
  import type { Player, Round } from "../types";

  interface Props {
    round: Round;
    players: Player[];
  }

  let { round, players }: Props = $props();

  // Resolve player ids to display names.
  const byId = $derived(new Map(players.map((p) => [p.id, p])));

  function name(id: string): string {
    const p = byId.get(id);
    if (!p) return "(unknown)";
    const full = `${p.last_name} ${p.first_name}`.trim();
    return p.rating != null ? `${full} (${p.rating})` : full;
  }
</script>

<div class="round">
  {#if round.boards.length === 0}
    <p class="empty">No boards in this round.</p>
  {:else}
    <table>
      <thead>
        <tr>
          <th class="num">Board</th>
          <th>Player 1</th>
          <th>Player 2</th>
        </tr>
      </thead>
      <tbody>
        {#each round.boards as board, i (i)}
          <tr>
            <td class="num">{i + 1}</td>
            <td>{name(board.player1)}</td>
            <td>{name(board.player2)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  {#if round.bye}
    <p class="bye"><strong>Bye:</strong> {name(round.bye)}</p>
  {/if}
</div>

<style>
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.9rem;
  }
  th,
  td {
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid #2b2b31;
    text-align: left;
  }
  th {
    color: #9a9aa2;
    font-weight: 600;
    font-size: 0.8rem;
  }
  .num {
    text-align: right;
    width: 3.5rem;
    font-variant-numeric: tabular-nums;
  }
  .empty {
    color: #9a9aa2;
    font-size: 0.9rem;
  }
  .bye {
    margin-top: 1rem;
    color: #9a9aa2;
    font-size: 0.9rem;
  }
</style>
