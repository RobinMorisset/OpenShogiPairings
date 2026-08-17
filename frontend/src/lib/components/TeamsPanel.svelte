<script lang="ts">
  // The Teams tab, next to Players and shown only in team mode.
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
  import type { PickerOption } from "../picker";
  import { pairingRating, sortedByRating, teamAverageRating } from "../teams";
  import Combobox from "./Combobox.svelte";

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
    // The team's id only exists once the server has created it, so remember
    // what was asked for and pick the card up by name when it arrives.
    pendingTeamName = trimmed;
  }

  function startRename(team: Team) {
    renaming = team.id;
    renameText = team.name;
  }

  /** Drop the edit and keep the name as it was. */
  function cancelRename() {
    renaming = null;
    renameText = "";
  }

  function commitRename(team: Team) {
    // Escape clears `renaming` first, so this is a blur that follows a cancel:
    // the edit is already abandoned and must not be written back.
    if (renaming !== team.id) return;
    renaming = null;
    const trimmed = renameText.trim();
    if (trimmed && trimmed !== team.name) onRename(team.id, trimmed);
  }

  function renameKeydown(event: KeyboardEvent, team: Team) {
    if (event.key === "Enter") {
      event.preventDefault();
      commitRename(team);
    } else if (event.key === "Escape") {
      event.preventDefault();
      cancelRename();
    }
  }

  // --- The member picker -------------------------------------------------
  //
  // A `Combobox` over the unassigned pool rather than a free-text field: every
  // candidate is already registered, so there is no "create player" fallback
  // (that is the Players tab's job, and team mode refuses late registration
  // anyway). It keeps no choice of its own — a picked member moves to the
  // roster and the field goes back to empty, ready for the next one — so its
  // `text` stays "".

  /** The unassigned pool as the picker's options, each showing the rating its
   *  row in the roster would. */
  const memberOptions = $derived<PickerOption[]>(
    unassigned.map((p) => ({
      key: p.id,
      label: name(p),
      meta: ratingLabel(p),
      metaUnofficial: p.rating == null,
    })),
  );

  /** The card pickers and the new-team field, so focus can be placed after a
   *  request completes. */
  const pickerInputs: Record<string, { focus(): void } | undefined> = {};
  let newTeamInput: HTMLInputElement | undefined = $state();
  let refocusTeam: string | null = $state(null);
  /** A team just asked for, waiting on the server to create it. */
  let pendingTeamName: string | null = $state(null);

  // A new team's first job is being filled, so focus its picker as soon as the
  // card exists. Matched by name — the id is the server's to mint — the same
  // way the server itself compares them, so it also lands on nothing if the
  // name was refused as a duplicate.
  $effect(() => {
    if (!pendingTeamName || busy) return;
    const wanted = pendingTeamName.trim().toLowerCase();
    const created = teams.find((t) => t.name.trim().toLowerCase() === wanted);
    pendingTeamName = null;
    if (created) pickerInputs[created.id]?.focus();
  });

  // Adding a member disables the input while the request is in flight, which
  // drops focus (and blurs the picker shut). Put it back when the request
  // lands, so a roster can be filled by typing one surname after another; the
  // input's own `onfocus` reopens the list. The member that fills a team ends
  // that card's work — its picker is gone anyway — so focus goes to the
  // new-team field, which is where the referee is heading next.
  $effect(() => {
    if (refocusTeam && !busy) {
      const filled = teams.find((t) => t.id === refocusTeam)?.members.length === size;
      const input = filled ? newTeamInput : pickerInputs[refocusTeam];
      refocusTeam = null;
      input?.focus();
    }
  });

  function pickMember(teamId: string, playerId: string) {
    onAddMember(teamId, playerId);
    refocusTeam = teamId;
  }

  /** Move a member one place up or down the board order. */
  function move(team: Team, index: number, delta: number) {
    const order = [...team.members];
    const to = index + delta;
    if (to < 0 || to >= order.length) return;
    [order[index], order[to]] = [order[to], order[index]];
    onSetBoardOrder(team.id, order);
  }

  /** How a member's rating reads on their row: a real one as it is, a pairing
   *  ELO starred (and italic, see the stylesheet) because the player is unrated,
   *  and an em dash when there is neither. */
  function ratingLabel(p: Player): string {
    if (p.rating != null) return String(p.rating);
    return p.pairing_rating != null ? `${p.pairing_rating}*` : "—";
  }

  /** A member's rating cell is editable when it is the *pairing* ELO — an
   *  unrated player, MacMahon starting points in use (the server refuses one
   *  otherwise), rosters not yet frozen. A rated player's ELO belongs to the
   *  Players tab. */
  function editableRating(p: Player): boolean {
    return macmahonInUse && p.rating == null && !finalized;
  }

  /** Whether this member still needs a pairing ELO before finalization. */
  function needsPairingRating(p: Player): boolean {
    return macmahonInUse && pairingRating(p) == null;
  }

  /** The member whose pairing ELO is being typed, and the text so far. */
  let editingRating: string | null = $state(null);
  let ratingDraft = $state("");

  function startRating(p: Player) {
    if (busy) return;
    editingRating = p.id;
    ratingDraft = p.pairing_rating != null ? String(p.pairing_rating) : "";
  }

  /** Commit the pairing ELO as typed: empty clears it, anything that isn't a
   *  non-negative whole number is refused rather than sent as a wrong value. */
  function commitRating(p: Player) {
    if (editingRating !== p.id) return;
    editingRating = null;
    const raw = ratingDraft.trim();
    const value = raw === "" ? null : Number(raw);
    if (value !== null && (!Number.isInteger(value) || value < 0)) return;
    if (value === (p.pairing_rating ?? null)) return;
    onSetPairingRating(p.id, value);
  }

  function ratingKeydown(event: KeyboardEvent, p: Player) {
    if (event.key === "Enter") {
      event.preventDefault();
      commitRating(p);
    } else if (event.key === "Escape") {
      event.preventDefault();
      editingRating = null;
    }
  }

  function autofocus(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  // Which team's adjustments panel is open, and the working values for its
  // "add" form.
  let adjustingId: string | null = $state(null);
  let adjustmentDelta = $state("");
  let adjustmentReason = $state("");

  /**
   * Deleting a team means two different things either side of finalization, so
   * ask in the words that match: before it the members go back to the
   * unassigned pool, after it there is no pool to go back to — finalization is
   * what requires it to be empty — and they leave the tournament with their
   * team. A one-line "delete this team?" would be true in both and informative
   * in neither.
   */
  function confirmRemove(team: Team, members: number) {
    const message = finalized
      ? $_("teams.confirmRemoveWithPlayers", {
          values: { name: team.name, count: members },
        })
      : $_("teams.confirmRemove", { values: { name: team.name } });
    if (window.confirm(message)) onRemove(team.id);
  }

  function toggleAdjustments(teamId: string) {
    adjustingId = adjustingId === teamId ? null : teamId;
    adjustmentDelta = "";
    adjustmentReason = "";
  }

  /** Close the form, dropping whatever was typed — the way out of a misclick. */
  function closeAdjustments() {
    adjustingId = null;
    adjustmentDelta = "";
    adjustmentReason = "";
  }

  function adjustmentKeydown(event: KeyboardEvent, teamId: string) {
    if (event.key === "Enter") {
      event.preventDefault();
      submitAdjustment(teamId);
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeAdjustments();
    }
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
        class="control-sm control-quiet"
        bind:this={newTeamInput}
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
              class="rename control-sm control-quiet"
              bind:value={renameText}
              disabled={busy}
              onblur={() => commitRename(team)}
              onkeydown={(e) => renameKeydown(e, team)}
              use:autofocus
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
                disabled={busy || roster.length < 2 || sortedByRating(roster)}
                title={$_("teams.sortByRatingTitle")}
                onclick={() => onSortByRating(team.id)}
              >
                {$_("teams.sortByRating")}
              </button>
            {/if}
            <!-- Not gated on `finalized` either: a team that never played a
                 match can still be dropped, which is how a no-show team leaves.
                 The server refuses one that has played, naming why. -->
            <button
              type="button"
              class="small danger"
              disabled={busy}
              title={$_(finalized ? "teams.removeWithPlayersTitle" : "teams.removeTitle")}
              onclick={() => confirmRemove(team, roster.length)}
            >
              ✕
            </button>
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
                class="adj-delta control-quiet"
                type="number"
                placeholder={$_("teams.adjustmentPointsPlaceholder")}
                bind:value={adjustmentDelta}
                disabled={busy}
                onkeydown={(e) => adjustmentKeydown(e, team.id)}
              />
              <input
                class="adj-reason control-quiet"
                type="text"
                placeholder={$_("teams.adjustmentReasonPlaceholder")}
                bind:value={adjustmentReason}
                disabled={busy}
                onkeydown={(e) => adjustmentKeydown(e, team.id)}
              />
              <button
                type="button"
                class="small"
                disabled={busy}
                onclick={() => submitAdjustment(team.id)}>{$_("teams.addAdjustment")}</button
              >
              <!-- Opening this is one click and closing it was another click on
                   the same ±, which is not something a misclick suggests. -->
              <button type="button" class="small" onclick={closeAdjustments}>
                {$_("teams.cancelAdjustment")}
              </button>
            </div>
          </div>
        {/if}

        <ol class="members">
          {#each roster as member, index (member.id)}
            <li>
              <span class="board">{index + 1}</span>
              <span class="member-name">{name(member)}</span>
              {#if editingRating === member.id}
                <!-- Same shape as the Players tab's editable cells: type=text
                     with a numeric keypad (type=number ignores `size`), commit
                     on Enter or blur, Escape to drop it. -->
                <input
                  class="rating-input"
                  type="text"
                  inputmode="numeric"
                  size="1"
                  title={$_("teams.pairingEloTitle")}
                  bind:value={ratingDraft}
                  onkeydown={(e) => ratingKeydown(e, member)}
                  onblur={() => commitRating(member)}
                  use:autofocus
                />
              {:else if editableRating(member)}
                <button
                  type="button"
                  class="rating unofficial rating-btn"
                  class:missing={needsPairingRating(member)}
                  disabled={busy}
                  title={$_("teams.pairingEloTitle")}
                  onclick={() => startRating(member)}
                >
                  {ratingLabel(member)}
                </button>
              {:else}
                <span class="rating" class:unofficial={member.rating == null}>
                  {ratingLabel(member)}
                </span>
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
          <div class="assign">
            <Combobox
              bind:this={pickerInputs[team.id]}
              id={`team-picker-${team.id}`}
              options={memberOptions}
              placeholder={$_("teams.addMember")}
              noMatch={$_("teams.noMemberMatch")}
              disabled={busy}
              width="100%"
              onPick={(option) => pickMember(team.id, option.key)}
            />
          </div>
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
              <span class="rating" class:unofficial={p.rating == null}>
                {ratingLabel(p)}
              </span>
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
    color: var(--text-secondary);
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
    border-color: var(--color-warning);
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
    color: var(--text-secondary);
    font-weight: 400;
    margin-right: 0.2rem;
  }
  .size.short {
    color: var(--color-warning);
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
    color: var(--text-secondary);
    width: 1.2em;
    text-align: right;
  }
  .member-name {
    flex: 1;
  }
  /* A pairing ELO is not a real rating, and must never read as one: italic, and
     starred in `ratingLabel`. */
  .rating.unofficial {
    font-style: italic;
  }
  /* The pairing ELO is edited in place, so its cell is a bare button that reads
     as the text it replaces — the styling the Players tab's editable cells use. */
  .rating-btn {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    font-size: 0.9em;
    color: inherit;
    cursor: pointer;
  }
  .rating-btn:hover:not(:disabled) {
    text-decoration: underline;
  }
  /* Still missing, and MacMahon needs it before the tournament can start. */
  .rating-btn.missing {
    color: var(--color-warning);
    font-weight: 600;
  }
  .rating-input {
    width: 4rem;
    font-size: 0.9em;
  }
  .adj-badge {
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
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
    color: var(--color-success);
  }
  .malus {
    color: var(--color-danger);
  }
  .reason {
    flex: 1;
  }
  .adjustment-form {
    display: flex;
    align-items: center;
    gap: 0.3rem;
  }
  /* Sized to the `.small` buttons they sit between, not to the panel's
     full-size fields — this is a mini-form inside a card. */
  .adj-delta,
  .adj-reason {
    font-size: 0.85em;
    padding: 0.15rem 0.35rem;
  }
  .adj-delta {
    width: 4rem;
  }
  .adj-reason {
    flex: 1;
    min-width: 4rem;
  }
  .small {
    padding: 0 0.35rem;
    font-size: 0.85em;
  }
  /* The member picker sits at the foot of the card; the field and its list are
     `Combobox`'s own. */
  .assign {
    margin-top: 0.4rem;
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
    color: var(--text-secondary);
    margin: 0.25rem 0;
  }
</style>
