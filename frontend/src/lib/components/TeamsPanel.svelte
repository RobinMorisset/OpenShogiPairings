<script lang="ts">
  // The Teams panel of the Players tab, shown only in team mode.
  //
  // Registration is players-first: players are registered exactly as in an
  // individual tournament, and teams are a grouping laid over them. So this
  // panel is about the grouping alone — who is in which team, in which board
  // order — plus the pairing ELO an unrated member needs when MacMahon starting
  // points are in use.
  //
  // Every action here is registration-time only; the server freezes the rosters
  // at finalization, and the panel goes read-only to match.

  import { _ } from "svelte-i18n";
  import type { Player, Team } from "../types";
  import { pairingRating, teamAverageRating } from "../teams";

  interface Props {
    teams: Team[];
    players: Player[];
    /** Players per team, from the settings — the size each roster must reach. */
    size: number;
    /** Registration finalized: rosters and board order are frozen. */
    finalized?: boolean;
    /** MacMahon starting points are in use, so every member needs a pairing
     *  rating and unrated ones get the "pairing ELO" field. */
    macmahonInUse?: boolean;
    onAdd: (name: string) => void;
    onRename: (teamId: string, name: string) => void;
    onRemove: (teamId: string) => void;
    onAddMember: (teamId: string, playerId: string) => void;
    onRemoveMember: (teamId: string, playerId: string) => void;
    onSetBoardOrder: (teamId: string, order: string[]) => void;
    onSortByRating: (teamId: string) => void;
    onSetPairingRating: (playerId: string, rating: number | null) => void;
    /** Apply a manual point bonus/malus to a team. Unlike everything else here
     *  this stays available after finalization — a fair-play bonus or a penalty
     *  is decided mid-tournament, not while the rosters are being built. */
    onAddAdjustment: (teamId: string, delta: number, reason: string) => void;
    onRemoveAdjustment: (teamId: string, adjustmentId: string) => void;
    busy?: boolean;
  }

  let {
    teams,
    players,
    size,
    finalized = false,
    macmahonInUse = false,
    onAdd,
    onRename,
    onRemove,
    onAddMember,
    onRemoveMember,
    onSetBoardOrder,
    onSortByRating,
    onSetPairingRating,
    onAddAdjustment,
    onRemoveAdjustment,
    busy = false,
  }: Props = $props();

  let newName = $state("");
  /** The team being renamed, and the text being typed into it. */
  let renaming: string | null = $state(null);
  let renameText = $state("");

  const byId = $derived(new Map(players.map((p) => [p.id, p])));
  const assigned = $derived(new Set(teams.flatMap((t) => t.members)));
  const unassigned = $derived(players.filter((p) => !assigned.has(p.id)));

  /** A team's members as players, in board order (skipping any the roster
   *  references but the player list doesn't hold — only a stale view). */
  function members(team: Team): Player[] {
    return team.members.map((id) => byId.get(id)).filter((p): p is Player => p != null);
  }

  function name(p: Player): string {
    return `${p.last_name} ${p.first_name}`.trim();
  }

  function submitNew(e: Event) {
    e.preventDefault();
    const trimmed = newName.trim();
    if (!trimmed) return;
    onAdd(trimmed);
    newName = "";
  }

  function startRename(team: Team) {
    renaming = team.id;
    renameText = team.name;
  }

  function commitRename(team: Team) {
    const trimmed = renameText.trim();
    if (trimmed && trimmed !== team.name) onRename(team.id, trimmed);
    renaming = null;
  }

  /** Move a member one place up or down the board order. */
  function move(team: Team, index: number, delta: number) {
    const order = [...team.members];
    const to = index + delta;
    if (to < 0 || to >= order.length) return;
    [order[index], order[to]] = [order[to], order[index]];
    onSetBoardOrder(team.id, order);
  }

  /** The pairing-ELO field's value as typed: empty clears it. */
  function commitPairingRating(player: Player, raw: string) {
    const trimmed = raw.trim();
    const value = trimmed === "" ? null : Number(trimmed);
    if (value !== null && !Number.isFinite(value)) return;
    if (value === (player.pairing_rating ?? null)) return;
    onSetPairingRating(player.id, value);
  }

  /** Whether this member still needs a pairing ELO before finalization. */
  function needsPairingRating(p: Player): boolean {
    return macmahonInUse && pairingRating(p) == null;
  }

  // Which team's adjustments panel is open, and the working values for its
  // "add" form.
  let adjustingId: string | null = $state(null);
  let adjustmentDelta = $state("");
  let adjustmentReason = $state("");

  function toggleAdjustments(teamId: string) {
    adjustingId = adjustingId === teamId ? null : teamId;
    adjustmentDelta = "";
    adjustmentReason = "";
  }

  function adjustmentTotal(team: Team): number {
    return (team.adjustments ?? []).reduce((sum, a) => sum + a.delta, 0);
  }

  function submitAdjustment(teamId: string) {
    const delta = Number(adjustmentDelta);
    const reason = adjustmentReason.trim();
    // The server enforces both too; refusing here keeps the referee from
    // sending a request that can only come back as an error.
    if (!Number.isFinite(delta) || delta === 0 || reason === "") return;
    onAddAdjustment(teamId, delta, reason);
    adjustmentDelta = "";
    adjustmentReason = "";
  }
</script>

<section class="teams">
  <div class="head">
    <h3>{$_("teams.title")}</h3>
    <span class="count">{$_("teams.count", { values: { count: teams.length } })}</span>
  </div>

  {#if !finalized}
    <form class="new-team" onsubmit={submitNew}>
      <input
        type="text"
        bind:value={newName}
        placeholder={$_("teams.namePlaceholder")}
        disabled={busy}
        data-testid="new-team-name"
      />
      <button type="submit" disabled={busy || newName.trim() === ""}>
        {$_("teams.add")}
      </button>
    </form>
  {/if}

  {#if teams.length === 0}
    <p class="empty">{$_("teams.none")}</p>
  {/if}

  <div class="cards">
    {#each teams as team (team.id)}
      {@const roster = members(team)}
      {@const average = teamAverageRating(roster)}
      <div class="card" class:incomplete={roster.length !== size}>
        <div class="card-head">
          {#if renaming === team.id}
            <input
              type="text"
              class="rename"
              bind:value={renameText}
              disabled={busy}
              onblur={() => commitRename(team)}
              onkeydown={(e) => {
                if (e.key === "Enter") commitRename(team);
                if (e.key === "Escape") renaming = null;
              }}
            />
          {:else}
            <button
              type="button"
              class="team-name"
              disabled={busy || finalized}
              title={finalized ? team.name : $_("teams.clickToRename")}
              onclick={() => !finalized && startRename(team)}
            >
              {#if team.tournament_id != null}<span class="num">{team.tournament_id}</span>{/if}
              {team.name}
            </button>
          {/if}
          <!-- One group, so a header too narrow for everything moves the whole
               set of controls down together instead of splitting it. -->
          <span class="head-meta">
            <span class="size" class:short={roster.length !== size}>
              {roster.length}/{size}
            </span>
            <span class="avg" title={$_("teams.averageTitle")}>
              {average ?? "—"}
            </span>
            {#if adjustmentTotal(team) !== 0}
              <span class="adj-badge">
                {adjustmentTotal(team) > 0 ? "+" : ""}{adjustmentTotal(team)}
              </span>
            {/if}
            <!-- Not gated on `finalized`: a bonus or penalty is decided during
                 the tournament, which is exactly when the rosters are frozen. -->
            <button
              type="button"
              class="small"
              disabled={busy}
              title={$_("teams.adjustmentTitle")}
              onclick={() => toggleAdjustments(team.id)}
            >
              ±
            </button>
            {#if !finalized}
              <button
                type="button"
                class="small"
                disabled={busy || roster.length < 2}
                title={$_("teams.sortByRatingTitle")}
                onclick={() => onSortByRating(team.id)}
              >
                {$_("teams.sortByRating")}
              </button>
              <button
                type="button"
                class="small danger"
                disabled={busy}
                title={$_("teams.removeTitle")}
                onclick={() => onRemove(team.id)}
              >
                ✕
              </button>
            {/if}
          </span>
        </div>

        {#if adjustingId === team.id}
          <div class="adjustments">
            {#if (team.adjustments ?? []).length > 0}
              <ul class="adjustments-list">
                {#each team.adjustments ?? [] as adj (adj.id)}
                  <li>
                    <span class:bonus={adj.delta > 0} class:malus={adj.delta < 0}>
                      {adj.delta > 0 ? "+" : ""}{adj.delta}
                    </span>
                    <span class="reason">{adj.reason}</span>
                    <button
                      type="button"
                      class="small danger"
                      title={$_("teams.removeAdjustment")}
                      disabled={busy}
                      onclick={() => onRemoveAdjustment(team.id, adj.id)}>✕</button
                    >
                  </li>
                {/each}
              </ul>
            {/if}
            <div class="adjustment-form">
              <input
                class="adj-delta"
                type="number"
                placeholder={$_("teams.adjustmentPointsPlaceholder")}
                bind:value={adjustmentDelta}
                disabled={busy}
              />
              <input
                class="adj-reason"
                type="text"
                placeholder={$_("teams.adjustmentReasonPlaceholder")}
                bind:value={adjustmentReason}
                disabled={busy}
                onkeydown={(e) => e.key === "Enter" && submitAdjustment(team.id)}
              />
              <button
                type="button"
                class="small"
                disabled={busy}
                onclick={() => submitAdjustment(team.id)}>{$_("teams.addAdjustment")}</button
              >
            </div>
          </div>
        {/if}

        <ol class="members">
          {#each roster as member, index (member.id)}
            <li>
              <span class="board">{index + 1}</span>
              <span class="member-name">{name(member)}</span>
              <span class="rating" class:unofficial={member.rating == null}>
                {pairingRating(member) ?? "—"}
              </span>
              {#if macmahonInUse && member.rating == null && !finalized}
                <input
                  type="number"
                  class="pairing-elo"
                  class:missing={needsPairingRating(member)}
                  value={member.pairing_rating ?? ""}
                  placeholder={$_("teams.pairingEloPlaceholder")}
                  title={$_("teams.pairingEloTitle")}
                  disabled={busy}
                  onchange={(e) => commitPairingRating(member, e.currentTarget.value)}
                />
              {/if}
              {#if !finalized}
                <button
                  type="button"
                  class="small"
                  disabled={busy || index === 0}
                  title={$_("teams.moveUp")}
                  onclick={() => move(team, index, -1)}>▲</button
                >
                <button
                  type="button"
                  class="small"
                  disabled={busy || index === roster.length - 1}
                  title={$_("teams.moveDown")}
                  onclick={() => move(team, index, 1)}>▼</button
                >
                <button
                  type="button"
                  class="small danger"
                  disabled={busy}
                  title={$_("teams.removeMemberTitle")}
                  onclick={() => onRemoveMember(team.id, member.id)}>✕</button
                >
              {/if}
            </li>
          {/each}
        </ol>

        {#if !finalized && roster.length < size && unassigned.length > 0}
          <select
            class="assign"
            disabled={busy}
            value=""
            onchange={(e) => {
              const id = e.currentTarget.value;
              e.currentTarget.value = "";
              if (id) onAddMember(team.id, id);
            }}
          >
            <option value="">{$_("teams.addMember")}</option>
            {#each unassigned as p (p.id)}
              <option value={p.id}>{name(p)}</option>
            {/each}
          </select>
        {/if}
      </div>
    {/each}
  </div>

  {#if !finalized}
    <div class="pool">
      <h4>
        {$_("teams.unassigned", { values: { count: unassigned.length } })}
      </h4>
      {#if unassigned.length === 0}
        <p class="empty">{$_("teams.allAssigned")}</p>
      {:else}
        <ul class="pool-list">
          {#each unassigned as p (p.id)}
            <li>
              {name(p)}
              <span class="rating">{pairingRating(p) ?? "—"}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</section>

<style>
  .teams {
    margin-top: 1rem;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
  }
  h3 {
    margin: 0;
  }
  .count,
  .avg,
  .rating {
    color: var(--muted);
    font-size: 0.9em;
  }
  .new-team {
    display: flex;
    gap: 0.5rem;
    margin: 0.5rem 0;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(18rem, 1fr));
    gap: 0.75rem;
  }
  .card {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.5rem 0.75rem;
  }
  /* A roster that isn't the configured size blocks finalization, so it is
     visible at a glance rather than only in the error afterwards. */
  .card.incomplete {
    border-color: var(--warning, #c90);
  }
  .card-head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  /* The name is the only elastic part of the header: a `0` basis keeps it from
     ever being what pushes the controls onto a second line, so every card's
     header is one line whatever its team is called. A name too long for the
     space left ellipsises instead (renaming shows it in full). */
  .team-name,
  .rename {
    flex: 1 1 0;
    min-width: 0;
  }
  .head-meta {
    flex: 0 0 auto;
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  .team-name {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    font-weight: 600;
    color: inherit;
    cursor: pointer;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .team-name:disabled {
    cursor: default;
  }
  .num {
    color: var(--muted);
    font-weight: 400;
    margin-right: 0.2rem;
  }
  .size.short {
    color: var(--warning, #c90);
    font-weight: 600;
  }
  .members {
    list-style: none;
    margin: 0.4rem 0 0;
    padding: 0;
  }
  .members li {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.1rem 0;
  }
  .board {
    color: var(--muted);
    width: 1.2em;
    text-align: right;
  }
  .member-name {
    flex: 1;
  }
  /* A pairing ELO is not a real rating, and must never read as one. */
  .rating.unofficial {
    font-style: italic;
  }
  .pairing-elo {
    width: 5rem;
  }
  .adj-badge {
    font-variant-numeric: tabular-nums;
    color: var(--muted);
  }
  .adjustments {
    margin-top: 0.4rem;
    padding: 0.35rem 0.5rem;
    border-left: 2px solid var(--border);
  }
  .adjustments-list {
    list-style: none;
    margin: 0 0 0.35rem;
    padding: 0;
  }
  .adjustments-list li {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  .bonus {
    color: var(--win, green);
  }
  .malus {
    color: var(--loss, crimson);
  }
  .reason {
    flex: 1;
  }
  .adjustment-form {
    display: flex;
    gap: 0.3rem;
  }
  .adj-delta {
    width: 4rem;
  }
  .adj-reason {
    flex: 1;
    min-width: 4rem;
  }
  .pairing-elo.missing {
    border-color: var(--warning, #c90);
  }
  .small {
    padding: 0 0.35rem;
    font-size: 0.85em;
  }
  .assign {
    margin-top: 0.4rem;
    width: 100%;
  }
  .pool {
    margin-top: 1rem;
  }
  .pool h4 {
    margin: 0 0 0.25rem;
  }
  .pool-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem 1rem;
  }
  .empty {
    color: var(--muted);
    margin: 0.25rem 0;
  }
</style>
