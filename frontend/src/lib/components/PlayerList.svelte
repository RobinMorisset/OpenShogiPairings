<script lang="ts">
  import type { Player } from "../types";

  interface Props {
    players: Player[];
    onRemove: (id: string) => void;
    busy?: boolean;
  }

  let { players, onRemove, busy = false }: Props = $props();
</script>

{#if players.length === 0}
  <p class="empty">No players registered yet.</p>
{:else}
  <table>
    <thead>
      <tr>
        <th class="num">#</th>
        <th>Name</th>
        <th class="num">Rating</th>
        <th>Club</th>
        <th aria-label="Actions"></th>
      </tr>
    </thead>
    <tbody>
      {#each players as player, i (player.id)}
        <tr>
          <td class="num">{i + 1}</td>
          <td>{player.name}</td>
          <td class="num">{player.rating ?? "—"}</td>
          <td>{player.club ?? "—"}</td>
          <td class="actions">
            <button
              type="button"
              class="remove"
              title="Remove player"
              disabled={busy}
              onclick={() => onRemove(player.id)}
            >
              ✕
            </button>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}

<style>
  .empty {
    color: #9a9aa2;
    font-size: 0.9rem;
  }
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
    font-variant-numeric: tabular-nums;
  }
  .actions {
    text-align: right;
    width: 2rem;
  }
  .remove {
    padding: 0.1rem 0.4rem;
    font-size: 0.8rem;
    color: #f85149;
    border-color: transparent;
    background: transparent;
  }
  .remove:hover:not(:disabled) {
    border-color: #f85149;
  }
</style>
