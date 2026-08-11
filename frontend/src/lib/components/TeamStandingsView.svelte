<script lang="ts">
  // The team standings table, shown above the per-player one in team mode.
  //
  // Teams are what a team tournament ranks, so this is the primary table; the
  // player table below it stays as the per-player breakdown (and is what the
  // American grid exports). A team row expands to its members in board order,
  // with the games each of them won — the figures the team's board-wins
  // tie-break is the sum of.
  //
  // The ranking itself is the server's (`team_standings`); this only renders it.

  import { _ } from "svelte-i18n";
  import { TIEBREAKS } from "../types";
  import type { Player, Standing, TeamStanding, Tiebreak } from "../types";
  import { tiebreakLabel, tiebreakTitle } from "../tiebreaks";
  import { formatScore, HALF_POINT_TIEBREAKS } from "../score";

  interface Props {
    /** Ranked team standings, server-computed (the canonical ordering). */
    teamStandings: TeamStanding[];
    /** Per-player standings, for the member breakdown rows. */
    standings: Standing[];
    players: Player[];
    /** The ranking criteria in the referee's order, from the settings. */
    tiebreaks: Tiebreak[];
  }

  let { teamStandings, standings, players, tiebreaks }: Props = $props();

  const byId = $derived(new Map(players.map((p) => [p.id, p])));
  const standingById = $derived(new Map(standings.map((s) => [s.player_id, s])));
  const teamNameById = $derived(new Map(teamStandings.map((t) => [t.team_id, t])));

  /** The tie-break columns to show, resolved from the settings order. Unknown
   *  codes (from a newer save) are skipped, as in the player table. */
  const columns = $derived(
    tiebreaks
      .map((code) => TIEBREAKS.find((t) => t.code === code))
      .filter((t): t is (typeof TIEBREAKS)[number] => t != null)
      // The estimated ELO is a per-player quantity and ranks no team.
      .filter((t) => t.code !== "est_elo"),
  );

  /** Which teams are expanded to their member rows. */
  let expanded = $state(new Set<string>());

  function toggle(teamId: string) {
    // Reassigned rather than mutated so the derived rendering re-runs.
    const next = new Set(expanded);
    if (!next.delete(teamId)) next.add(teamId);
    expanded = next;
  }

  /**
   * The `TeamStanding` field a criterion reads.
   *
   * `TIEBREAKS.field` names the *player* field, and the two disagree on exactly
   * one criterion: a player's board wins are their own `victories`, while a
   * team's are its members' total (`board_wins`) — its `victories` are *matches*
   * won. Reading the player field here would silently show match wins under the
   * board-wins column, which is the one number the team tie-break turns on.
   */
  function teamField(code: Tiebreak): string {
    if (code === "board_wins") return "board_wins";
    return TIEBREAKS.find((t) => t.code === code)?.field ?? "";
  }

  function value(team: TeamStanding, code: Tiebreak): string {
    const raw = (team as unknown as Record<string, number>)[teamField(code)];
    if (raw == null) return "—";
    return HALF_POINT_TIEBREAKS.has(code) ? formatScore(raw) : String(raw);
  }

  function name(id: string): string {
    const p = byId.get(id);
    return p ? `${p.last_name} ${p.first_name}`.trim() : "—";
  }

  /** A member's own games won — what the team's board wins are the sum of. */
  function memberWins(id: string): string {
    const s = standingById.get(id);
    return s ? String(s.victories) : "—";
  }

  /** An opposing team's number, for the cross-table column. */
  function opponentNumber(teamId: string): string {
    return String(teamNameById.get(teamId)?.tournament_id ?? "?");
  }
</script>

<section class="team-standings">
  <h3>{$_("teamStandings.title")}</h3>
  <table>
    <thead>
      <tr>
        <th class="num">{$_("teamStandings.rank")}</th>
        <th class="num">{$_("resultsView.id")}</th>
        <th>{$_("teamStandings.team")}</th>
        {#each columns as column (column.code)}
          <th class="num" title={tiebreakTitle(column.code, $_)}>
            {tiebreakLabel(column.code, $_)}
          </th>
        {/each}
        <th>{$_("teamStandings.opponents")}</th>
      </tr>
    </thead>
    <tbody>
      {#each teamStandings as team, rank (team.team_id)}
        <tr class="team-row">
          <td class="num">{rank + 1}</td>
          <td class="num">{team.tournament_id}</td>
          <td>
            <button
              type="button"
              class="expand"
              aria-expanded={expanded.has(team.team_id)}
              title={$_("teamStandings.expandTitle")}
              onclick={() => toggle(team.team_id)}
            >
              <span class="chevron">{expanded.has(team.team_id) ? "▾" : "▸"}</span>
              {team.name}
            </button>
          </td>
          {#each columns as column (column.code)}
            <td class="num">{value(team, column.code)}</td>
          {/each}
          <td class="opponents">
            {#each team.opponents as opponent, i (i)}
              <span class="opp">{opponentNumber(opponent)}</span>
            {/each}
            {#if team.opponents.length === 0}—{/if}
          </td>
        </tr>
        {#if expanded.has(team.team_id)}
          {#each team.members as member, board (member)}
            <tr class="member-row">
              <td></td>
              <td class="num board">{board + 1}</td>
              <!-- One cell rather than one per tie-break column: a member's own
                   figures are not the team's, and lining them up under the team
                   columns would read as if they were. -->
              <td class="member" colspan={columns.length + 2}>
                {name(member)}
                <span class="member-note">
                  {$_("teamStandings.gamesWon", { values: { count: memberWins(member) } })}
                </span>
              </td>
            </tr>
          {/each}
        {/if}
      {/each}
    </tbody>
  </table>
</section>

<style>
  .team-standings {
    margin-bottom: 1.5rem;
  }
  h3 {
    margin: 0 0 0.4rem;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th,
  td {
    padding: 0.2rem 0.5rem;
    text-align: left;
    border-bottom: 1px solid var(--border);
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .expand {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: inherit;
    cursor: pointer;
    font-weight: 600;
  }
  .chevron {
    color: var(--muted);
    margin-right: 0.2rem;
  }
  .opponents {
    font-variant-numeric: tabular-nums;
  }
  .opp + .opp::before {
    content: " ";
  }
  .member-row td {
    border-bottom: none;
    color: var(--muted);
  }
  .member-row .member {
    color: inherit;
  }
  .member-note {
    font-size: 0.85em;
  }
</style>
