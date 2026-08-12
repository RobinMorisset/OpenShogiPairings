<script lang="ts">
  // The registered players, before there is anything to rank.
  //
  // Both public pages show this — the live reader page and the static export —
  // because before round 1 it is the whole point of looking: players check that
  // they are in the list. Everyone sits at their MacMahon start, so a standings
  // table would be a list of ties.
  import type { Player } from "../types";

  interface Props {
    players: Player[];
  }

  let { players }: Props = $props();
</script>

<ol class="entrants">
  {#each players as player (player.id)}
    <li>
      <span class="entrant-name">{player.last_name} {player.first_name}</span>
      {#if player.club}<span class="entrant-club">{player.club}</span>{/if}
      {#if player.rating != null}<span class="entrant-rating">{player.rating}</span>{/if}
    </li>
  {/each}
</ol>

<style>
  .entrants {
    margin: 1rem 0 0;
    padding-left: 1.5rem;
  }
  .entrants li {
    padding: 0.2rem 0;
    display: flex;
    gap: 0.75rem;
    align-items: baseline;
  }
  .entrant-name {
    font-weight: 600;
  }
  .entrant-club,
  .entrant-rating {
    color: var(--text-secondary);
    font-size: 0.85rem;
  }
</style>
