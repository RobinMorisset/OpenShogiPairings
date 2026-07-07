<script lang="ts">
  import {
    HANDICAPS,
    type Board,
    type Handicap,
    type HandicapPolicy,
    type Player,
    type Round,
    type Standing,
    type Winner,
  } from "../types";
  import { sourceBadge } from "../pairingSource";

  interface Props {
    round: Round;
    players: Player[];
    /** Ranked standings (canonical order), used to sort boards by top player. */
    standings: Standing[];
    /** Whether/how handicap games are shown: none, allowed, or suggested. */
    handicapPolicy: HandicapPolicy;
    /** Suggested handicap per board, indexed like `round.boards`. */
    suggestedHandicaps: (Handicap | null)[];
    /** Register a click on a board's player (toggles the winner). */
    onClickWinner: (boardIndex: number, clicked: Winner) => void;
    /** Set/clear the "a draw occurred" flag on a board. */
    onToggleDrawn: (boardIndex: number, drawn: boolean) => void;
    /** Set (or clear, with null) a board's handicap. */
    onSetHandicap: (boardIndex: number, handicap: Handicap | null) => void;
    busy?: boolean;
  }

  let {
    round,
    players,
    standings,
    handicapPolicy,
    suggestedHandicaps,
    onClickWinner,
    onToggleDrawn,
    onSetHandicap,
    busy = false,
  }: Props = $props();

  // Resolve player ids to display names.
  const byId = $derived(new Map(players.map((p) => [p.id, p])));

  // Rank of each player per the results tab's canonical ordering (0 = first
  // place). Missing from standings (e.g. not yet part of the tournament)
  // sorts last.
  const rankById = $derived(new Map(standings.map((s, i) => [s.player_id, i])));
  function rank(id: string): number {
    return rankById.get(id) ?? Infinity;
  }

  // Smart sort: cup games before non-cup, then by the rank of the game's
  // best-placed player (per the results tab's criteria).
  const orderedBoards = $derived(
    round.boards
      .map((board, index) => ({ board, index }))
      .sort((a, b) => {
        const aCup = sourceBadge(a.board.source).kind === "cup" ? 0 : 1;
        const bCup = sourceBadge(b.board.source).kind === "cup" ? 0 : 1;
        if (aCup !== bCup) return aCup - bCup;
        const aRank = Math.min(rank(a.board.player1), rank(a.board.player2));
        const bRank = Math.min(rank(b.board.player1), rank(b.board.player2));
        return aRank - bRank;
      }),
  );

  function name(id: string): string {
    const p = byId.get(id);
    if (!p) return "(unknown)";
    const full = `${p.last_name} ${p.first_name}`.trim();
    return p.rating != null ? `${full} (${p.rating})` : full;
  }

  // A handicap needs an unambiguous giver — the higher-rated player — so it is
  // only offered when the two ratings differ (both unrated counts as equal).
  function handicapAllowed(board: Board): boolean {
    const r1 = byId.get(board.player1)?.rating ?? null;
    const r2 = byId.get(board.player2)?.rating ?? null;
    return r1 !== r2;
  }

  // Cup games are always played even — no picker, no suggestion, empty cells.
  function isCup(board: Board): boolean {
    return sourceBadge(board.source).kind === "cup";
  }

  function handicapLabel(h: Handicap): string {
    return HANDICAPS.find((x) => x.value === h)?.label ?? h;
  }

  // A single emoji flagging a non-Swiss pairing (forced or cup); blank for a
  // normal Swiss pairing. Kept as a narrow, header-less column between the two
  // players so it doesn't compete for attention with the pairing itself.
  function sourceEmoji(board: Board): string {
    switch (sourceBadge(board.source).kind) {
      case "forced":
        return "\u{1F512}";
      case "cup":
        return "\u{1F3C6}";
      default:
        return "";
    }
  }

  // Name of the frozen handicap giver, for the "X gives" hint.
  function giverName(board: Board): string {
    if (!board.handicap) return "";
    const id = board.handicap.giver === "player1" ? board.player1 : board.player2;
    const p = byId.get(id);
    return p ? `${p.last_name} ${p.first_name}`.trim() : "(unknown)";
  }

  function onHandicapChange(index: number, value: string) {
    onSetHandicap(index, value === "" ? null : (value as Handicap));
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
          <th class="src-col"></th>
          <th>Player 2</th>
          <th class="draw-col">Draw</th>
          {#if handicapPolicy !== "none"}
            <th>Handicap</th>
            {#if handicapPolicy === "suggested"}
              <th>Suggested</th>
            {/if}
          {/if}
        </tr>
      </thead>
      <tbody>
        {#each orderedBoards as { board, index } (index)}
          <tr>
            <td class="num">{index + 1}</td>
            <td>
              <button
                type="button"
                class="player"
                class:winner={board.result === "player1"}
                class:loser={board.result === "player2"}
                disabled={busy}
                title="Click to set as winner"
                onclick={() => onClickWinner(index, "player1")}
              >
                {name(board.player1)}
              </button>
            </td>
            <td class="src-col" title={sourceBadge(board.source).text}>{sourceEmoji(board)}</td>
            <td>
              <button
                type="button"
                class="player"
                class:winner={board.result === "player2"}
                class:loser={board.result === "player1"}
                disabled={busy}
                title="Click to set as winner"
                onclick={() => onClickWinner(index, "player2")}
              >
                {name(board.player2)}
              </button>
            </td>
            <td class="draw-col">
              <button
                type="button"
                class="draw"
                class:active={board.drawn}
                disabled={busy}
                title="A draw occurred before the decisive game (recorded for ELO)"
                aria-pressed={board.drawn ?? false}
                onclick={() => onToggleDrawn(index, !board.drawn)}
              >
                =
              </button>
            </td>
            {#if handicapPolicy !== "none"}
              <td>
                {#if !isCup(board)}
                  {#if handicapAllowed(board)}
                    <select
                      class="handicap"
                      disabled={busy}
                      value={board.handicap?.handicap ?? ""}
                      onchange={(e) => onHandicapChange(index, e.currentTarget.value)}
                    >
                      <option value=""></option>
                      {#each HANDICAPS as h (h.value)}
                        <option
                          value={h.value}
                          style={suggestedHandicaps[index] === h.value ? "font-weight:700" : ""}
                        >
                          {suggestedHandicaps[index] === h.value ? "★ " : ""}{h.label}
                        </option>
                      {/each}
                    </select>
                    {#if board.handicap}
                      <span class="giver" title="The higher-rated player concedes the odds"
                        >{giverName(board)} gives</span
                      >
                    {/if}
                  {:else}
                    <span class="na" title="Needs two players with different ratings">—</span>
                  {/if}
                {/if}
              </td>
              {#if handicapPolicy === "suggested"}
                <td class="suggested">
                  {#if !isCup(board) && suggestedHandicaps[index]}
                    {handicapLabel(suggestedHandicaps[index] as Handicap)}
                  {/if}
                </td>
              {/if}
            {/if}
          </tr>
        {/each}
      </tbody>
    </table>
    {#if round.completed}
      <p class="hint warning">
        ⚠ This round was already completed — changing a result here updates the
        recorded standings.
      </p>
    {:else}
      <p class="hint">Click a player to record them as the winner.</p>
    {/if}
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
  .src-col {
    width: 1.6rem;
    text-align: center;
    padding-left: 0.2rem;
    padding-right: 0.2rem;
    font-size: 0.95rem;
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

  .draw-col {
    text-align: center;
    width: 3.5rem;
  }
  .draw {
    width: 1.9rem;
    height: 1.9rem;
    border: 1px solid #3a3a42;
    border-radius: 0.4rem;
    background: transparent;
    color: #9a9aa2;
    font: inherit;
    font-weight: 700;
    cursor: pointer;
  }
  .draw:hover:not(:disabled) {
    background: #26262c;
  }
  .draw.active {
    border-color: #d29922;
    color: #d29922;
    background: #2a2410;
  }

  .handicap {
    background: #1c1c22;
    color: inherit;
    border: 1px solid #3a3a42;
    border-radius: 0.4rem;
    padding: 0.2rem 0.35rem;
    font: inherit;
  }
  .giver {
    margin-left: 0.5rem;
    color: #6a6a72;
    font-size: 0.8rem;
  }
  .suggested {
    color: #d29922;
    font-size: 0.85rem;
  }
  .na {
    color: #4a4a52;
  }

  .hint,
  .empty {
    color: #6a6a72;
    font-size: 0.8rem;
    margin-top: 0.6rem;
  }
  .hint.warning {
    color: #d29922;
  }
  .bye {
    margin-top: 1rem;
    color: #9a9aa2;
    font-size: 0.9rem;
  }
</style>
