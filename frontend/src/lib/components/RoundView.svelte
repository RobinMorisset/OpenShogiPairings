<script lang="ts">
  import type { Player, Round, Winner } from "../types";

  interface Props {
    round: Round;
    players: Player[];
    /** Register a click on a board's player (toggles the winner). */
    onClickWinner: (boardIndex: number, clicked: Winner) => void;
    busy?: boolean;
  }

  let { round, players, onClickWinner, busy = false }: Props = $props();

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
            <td>
              <button
                type="button"
                class="player"
                class:winner={board.result === "player1"}
                class:loser={board.result === "player2"}
                disabled={busy}
                title="Click to set as winner"
                onclick={() => onClickWinner(i, "player1")}
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
                title="Click to set as winner"
                onclick={() => onClickWinner(i, "player2")}
              >
                {name(board.player2)}
              </button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    <p class="hint">Click a player to record them as the winner.</p>
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
    padding: 0.3rem 0.6rem;
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
    border-color: #3a3a42;
    background: #26262c;
  }
  .player.winner {
    color: #3fb950;
    font-weight: 600;
  }
  .player.winner::before {
    content: "✓ ";
  }
  .player.loser {
    color: #6a6a72;
  }

  .hint,
  .empty {
    color: #6a6a72;
    font-size: 0.8rem;
    margin-top: 0.6rem;
  }
  .bye {
    margin-top: 1rem;
    color: #9a9aa2;
    font-size: 0.9rem;
  }
</style>
