<script lang="ts">
  import { _ } from "svelte-i18n";
  import { TIEBREAKS } from "../types";
  import type {
    Board,
    CupPodium,
    Player,
    PlayerCategory,
    Round,
    Sitout,
    SitoutValue,
    Standing,
    TeamMatchView,
    TeamStanding,
    Tiebreak,
    Tournament,
    Winner,
  } from "../types";
  import { teamAverageRating } from "../teams";
  import { tiebreakLabel, tiebreakTitle } from "../tiebreaks";
  import { boardOutcome, drawnOf, forfeitOf, winnerOf } from "../boardOutcome";
  import { absenceKind } from "../noShow";
  import { crossTableColumns, type CrossTableColumn } from "../crossTable";
  import { partitionDropped } from "../tiebreak";
  import { formatScore, HALF_POINT_TIEBREAKS } from "../score";
  import { printPage } from "../platform";

  interface Props {
    tournament: Tournament;
    /** Ranked standings computed server-side (the canonical ordering). */
    standings: Standing[];
    /** Ranked team standings, in team mode — the table is then one list of
     *  teams, each followed by its players in board order. Empty otherwise. */
    teamStandings?: TeamStanding[];
    /** The cup podium, once decided (adds medals; doesn't reorder the table). */
    cupPodium?: CupPodium | null;
    /** Winner that counts for standings/pairing per board, server-computed
     *  (respects the Wiel rule), indexed like `tournament.rounds[i].boards[j]`. */
    effectiveWinners: (Winner | null)[][];
    /** The team matches of each round, scored, as the server derived them
     *  (`TournamentResponse.team_matches`), indexed like `tournament.rounds`.
     *  Empty outside team mode. */
    teamMatches?: TeamMatchView[][];
    /** Referee-defined categories, for the highlight filter and leader marks. */
    categories?: PlayerCategory[];
    /** Rendering into a static HTML file (docs/archive/public-access.md phase 2), where
     *  no script will ever run: drop the controls that would be dead there —
     *  the same ones `print-hide` drops on paper, for the same reason. */
    staticPage?: boolean;
    /** Re-score one player's sit-out in one round. Omitted for a read-only
     *  table, which leaves the sit-out cells as plain text. */
    onSetSitoutValue?: (roundNumber: number, player: number, value: SitoutValue) => void;
    /** Re-score a whole team's sit-out, writing the value to every member at
     *  once — the only way to change one in team mode. */
    onSetTeamSitoutValue?: (roundNumber: number, teamId: string, value: SitoutValue) => void;
  }

  let {
    tournament,
    standings,
    teamStandings = [],
    cupPodium = null,
    effectiveWinners,
    teamMatches = [],
    categories = [],
    staticPage = false,
    onSetSitoutValue,
    onSetTeamSitoutValue,
  }: Props = $props();

  // Round number → its index into `tournament.rounds` (and so into
  // `effectiveWinners`), so a cell can find its board's server-computed winner
  // from the round it belongs to rather than from its column position — which a
  // long game's result, shown one column along, doesn't share.
  const roundIndexByNumber = $derived(
    new Map(tournament.rounds.map((r, i) => [r.number, i])),
  );

  // The estimated ELO gets a column of its own only in **ELO pairing mode**,
  // where it is the quantity the tournament is actually run on. Under
  // estimate-based MacMahon the estimate is an input to the MacMahon points, and
  // shows up in *their* tooltip instead (see `macmahonTitle`) — a standalone
  // column there would read like the decorative "estimated rating" other pairing
  // software parks next to a ranking it has no effect on.
  const eloPairingMode = $derived(tournament.settings.pairing.kind === "elo");

  // "Apply estimates to unrated players only" (k = 0): rated players are pinned to
  // their registration rating, so the server sends no estimate for them at all and
  // the criterion stops ranking. Mirrors the server's `elo_estimate_rated_pinned`.
  const eloRatedPinned = $derived.by(() => {
    const pairing = tournament.settings.pairing;
    const estimator =
      pairing.kind === "elo"
        ? pairing.estimator
        : pairing.macmahon.source.kind === "from_estimate"
          ? pairing.macmahon.source.estimator
          : null;
    return estimator?.k_multiplier === 0;
  });

  // Mirrors the server's `est_elo_ranks` — keep the two in sync, or the estimate
  // the server ranks by would be hidden here (or vice versa).
  const estEloRanks = $derived(eloPairingMode && !eloRatedPinned);

  // The tie-break columns to show, in the referee-chosen order — resolved from
  // the settings to their label/field/tooltip. Unknown codes (from a newer save)
  // are skipped.
  const tiebreakColumns = $derived(
    (tournament.settings.tiebreaks ?? [])
      // Estimated ELO only ranks in ELO pairing mode, and only while rated players
      // are estimated; drop it as a column otherwise (defends against a loaded save
      // that still lists it).
      .filter((code) => code !== "est_elo" || estEloRanks)
      .map((code) => TIEBREAKS.find((t) => t.code === code))
      .filter((t): t is (typeof TIEBREAKS)[number] => t != null),
  );

  // The Est. ELO tooltip. Referees meet a same-named column in chess software
  // where it is a decorative performance rating with no effect on the pairings,
  // so the tooltip spells out that this one *is* what pairs the tournament —
  // plus, under "unrated players only", why the rated players' cells are empty.
  const estEloTitle = $derived(
    `${tiebreakTitle("est_elo", $_)}\n${$_("resultsView.estEloWhy")}` +
      (eloRatedPinned ? `\n${$_("resultsView.estEloUnratedOnly")}` : ""),
  );

  // Show the dedicated Est. ELO column — unless it already survived into
  // `tiebreakColumns`, where it appears with its ranking position and this one
  // would duplicate it. (Keyed off the resolved columns, not the raw settings, so
  // a stale save that lists a non-ranking est_elo still gets the dedicated column
  // rather than none at all.)
  const showEstimatedElo = $derived(
    eloPairingMode && !tiebreakColumns.some((c) => c.code === "est_elo"),
  );

  // Estimate-based MacMahon: the starting points were awarded by comparing the
  // thresholds against the live estimate, not the registration rating. Only worth
  // saying for the players the two disagree on — for everyone else the estimate
  // adds nothing the Rating column doesn't already show. Mirrors the server's
  // `macmahon_from_estimate_active`.
  const macmahonFromEstimate = $derived.by(() => {
    const pairing = tournament.settings.pairing;
    return (
      pairing.kind === "swiss" &&
      pairing.macmahon.source.kind === "from_estimate" &&
      pairing.macmahon.thresholds.some((t) => t.criterion.kind === "elo")
    );
  });

  // Player id → medal, from the cup podium (the table order stays pure-Swiss).
  // A place can be null when a double no-show left it undetermined (e.g. both
  // finalists absent), so each medal is awarded only if its winner exists.
  // Cup podium places are tournament numbers, not registration ids.
  const medalOf = $derived.by(() => {
    const m = new Map<number, string>();
    if (cupPodium) {
      if (cupPodium.champion != null) m.set(cupPodium.champion, "🥇");
      if (cupPodium.runner_up != null) m.set(cupPodium.runner_up, "🥈");
      if (cupPodium.third != null) m.set(cupPodium.third, "🥉");
    }
    return m;
  });

  // One column per round that has *started* — every confirmed round, not only
  // the completed ones (a draft isn't a round yet, so `tournament.rounds` is
  // exactly that). The round in progress gets its column as soon as it is
  // paired, filling in cell by cell as results are recorded: pending boards
  // show `3?` rather than a win or a loss.
  //
  // The numbers beside them do *not* follow along. The server scores from the
  // completed rounds alone (`compute_scores`), so wins, points, every tie-break
  // and the row order all stay put until the referee completes the round, at
  // which point the table re-sorts in one step. That is the point of the split:
  // the live column answers "what has come in so far?" without the ranking
  // churning under the referee's eyes mid-round.
  const shownRounds = $derived(tournament.rounds);
  const completedCount = $derived(tournament.rounds.filter((r) => r.completed).length);
  // Every round that isn't completed — normally just the one being played, but
  // an earlier round drops back in here the moment a referee clears one of its
  // results for a correction. Each such column is marked, and the note names
  // them all: their results aren't in the ranking yet, and saying only the first
  // would point at the wrong round in exactly the case that needs explaining.
  const liveRounds = $derived(tournament.rounds.filter((r) => !r.completed));

  // Rows follow the server's ranked order, joined to each player's details.
  const byId = $derived(new Map(tournament.players.map((p) => [p.id, p])));

  // Boards, byes and the cup podium all reference players by their tournament
  // number (`Player.tournament_id`), not the registration id — this maps
  // number → player for those lookups. Rounds only exist once registration is
  // finalized, at which point every player has a number.
  const byTid = $derived(
    new Map(
      tournament.players
        .filter((p): p is Player & { tournament_id: number } => p.tournament_id != null)
        .map((p) => [p.tournament_id, p]),
    ),
  );

  /** A player's tournament number — guaranteed present once rounds exist. */
  function tid(p: Player): number {
    return p.tournament_id!;
  }

  // Standings keyed by player id, so a cell can look up an opponent's metrics
  // when explaining how a tie-break was summed.
  const standingById = $derived(new Map(standings.map((s) => [s.player_id, s])));

  // A player's full name for a tooltip, "Last First"; an em dash if unknown
  // (e.g. an opponent removed after the game).
  const nameOf = (id: string): string => {
    const p = byId.get(id);
    return p ? `${p.last_name} ${p.first_name}`.trim() : "—";
  };

  // Same as `nameOf`, but for a player referenced by tournament number.
  const nameOfTid = (id: number): string => {
    const p = byTid.get(id);
    return p ? `${p.last_name} ${p.first_name}`.trim() : "—";
  };

  /** How a cross-table cell names the opponent it faced: `Doe Jane (1800)`, or
   * the bare name for an unrated player (an empty `()` would read as a missing
   * value rather than an absent rating). The rating rides along because it is
   * the question a referee hovers a cell to answer — was that a win against a
   * strong player? — and the cross-table number alone doesn't say. */
  const opponentLabel = (id: number): string => {
    const p = byTid.get(id);
    if (!p) return "—";
    const name = `${p.last_name} ${p.first_name}`.trim();
    return p.rating != null ? `${name} (${p.rating})` : name;
  };

  // Show a MacMahon column (between Wins and Points) when the starting score
  // matters — either someone has MacMahon starting points, or a manual
  // bonus/penalty adjusts a starting score. Its cell hosts the adjustment
  // underline that used to live on the Points cell.
  // In team mode both are team quantities: the starting points come from the
  // team's average, and adjustments apply to the team.
  const showMacmahon = $derived(
    teamStandings.length > 0
      ? teamStandings.some((t) => t.macmahon !== 0) ||
          (tournament.teams ?? []).some((t) => (t.adjustments ?? []).length > 0)
      : standings.some((s) => s.macmahon !== 0) ||
          tournament.players.some((p) => (p.adjustments ?? []).length > 0),
  );

  // The Wins column, on the same question. `points = macmahon + victories` by
  // construction, so when nobody has a MacMahon start every row holds the same
  // number in both columns and one of them is furniture — which is exactly the
  // condition `showMacmahon` already answers, manual adjustments included,
  // since an adjustment is folded into the MacMahon start.
  //
  // Unless Points is not on show: the referee can drop it from the tie-break
  // list, and then Wins is the only score in the table and has to stay.
  const showVictories = $derived(
    showMacmahon || !tiebreakColumns.some((col) => col.code === "points"),
  );
  const rows = $derived(
    standings
      .map((standing) => ({ standing, player: byId.get(standing.player_id) }))
      .filter(
        (r): r is { standing: Standing; player: Player } => r.player != null,
      ),
  );

  // --- Team mode ------------------------------------------------------------
  //
  // A team tournament ranks teams, and its players are what those teams are
  // made of — so it is one table, not two: each team's row, then its players in
  // board order. What is a *team* quantity (the ranking criteria, the MacMahon
  // start, the float markers, which is the pairing's own) belongs to the team
  // row alone; a player's own identity and results stay on theirs.
  const teamMode = $derived(teamStandings.length > 0);

  /** The matches of each round with a column — the server's own grouping, the
   *  same one the round view shows. */
  const matchesByRound = $derived(
    shownRounds.map((round) => {
      const index = roundIndexByNumber.get(round.number);
      return (index != null && teamMatches[index]) || [];
    }),
  );

  /** Team number → the team, for naming a match's opponent. */
  const teamByNumber = $derived(
    new Map((tournament.teams ?? []).map((t) => [t.tournament_id, t])),
  );

  /** How many columns describe a *player* rather than a result — last and
   *  first name, rating, the estimate when it has a column, nationality and
   *  club. A team row fills that band with its name in the first of them (the
   *  pinned one) and one empty cell for the rest. */
  const identityColumns = $derived(5 + (showEstimatedElo ? 1 : 0));

  type TableRow =
    | { kind: "team"; key: string; team: TeamStanding }
    | { kind: "player"; key: string; standing: Standing; player: Player };

  /** The table's rows: in team mode each team followed by its members, in
   *  ranked order; otherwise simply the ranked players. */
  const tableRows = $derived.by<TableRow[]>(() => {
    if (!teamMode) {
      return rows.map((r) => ({ kind: "player", key: r.player.id, ...r }));
    }
    const out: TableRow[] = [];
    for (const team of teamStandings) {
      out.push({ kind: "team", key: team.team_id, team });
      for (const id of team.members) {
        const player = byId.get(id);
        const standing = standingById.get(id);
        if (player && standing) out.push({ kind: "player", key: id, standing, player });
      }
    }
    return out;
  });

  /**
   * The `TeamStanding` field a criterion reads.
   *
   * `TIEBREAKS.field` names the *player* field, and the two disagree on exactly
   * one criterion: a player's board wins are their own `victories`, while a
   * team's are its members' total (`board_wins`) — its `victories` are *matches*
   * won. Reading the player field here would silently show match wins under the
   * board-wins column, which is the one number the team tie-break turns on.
   */
  function teamValue(team: TeamStanding, code: Tiebreak): string {
    const field = code === "board_wins" ? "board_wins" : (TIEBREAKS.find((t) => t.code === code)?.field ?? "");
    const raw = (team as unknown as Record<string, number>)[field];
    if (raw == null) return "—";
    return HALF_POINT_TIEBREAKS.has(code) ? formatScore(raw) : String(raw);
  }

  type TeamCell =
    | {
        kind: "team-played";
        opponent: string;
        opponentName: string;
        won: boolean;
        level: boolean;
        /** Board wins on each side, in half-wins as they arrive. */
        wins: number;
        losses: number;
        /** Half-points the team had over its opponent at pairing time
         *  (negative = paired up). Every board of the match carries it. */
        pointsDiff: number;
      }
    | { kind: "team-pending"; opponent: string; opponentName: string }
    // The whole team sat the round out — a bye or an absence. Every member has
    // the same entry, and this is the cell that re-scores them together.
    | { kind: "team-sitout"; roundNumber: number; sitout: Sitout }
    | { kind: "not-in-round" };

  function teamCell(team: TeamStanding, i: number): TeamCell {
    const round = shownRounds[i];
    const match = matchesByRound[i].find(
      (m) => m.team1 === team.tournament_id || m.team2 === team.tournament_id,
    );
    if (!match) {
      // No match this round. Every member sits out the same way, so the first
      // one says which it was.
      const first = team.members[0];
      const pid = first ? byId.get(first)?.tournament_id : null;
      const sitout = pid != null ? (round.sitouts ?? []).find((s) => s.player === pid) : undefined;
      if (!sitout) return { kind: "not-in-round" };
      return { kind: "team-sitout", roundNumber: round.number, sitout };
    }
    const isFirst = match.team1 === team.tournament_id;
    const otherNumber = isFirst ? match.team2 : match.team1;
    const other = teamByNumber.get(otherNumber);
    const opponent = String(otherNumber);
    // A team reads as an opponent the way a player does (`opponentLabel`): its
    // name, and the average rating when every member of it has one.
    const elo = teamAverageRating(
      (other?.members ?? []).map((id) => byId.get(id)).filter((p): p is Player => p != null),
    );
    const name = other?.name ?? `#${otherNumber}`;
    const opponentName = elo != null ? `${name} (${elo})` : name;
    if (!match.decided) return { kind: "team-pending", opponent, opponentName };
    const wins = isFirst ? match.wins1 : match.wins2;
    const losses = isFirst ? match.wins2 : match.wins1;
    // points_diff = points(team1) − points(team2) in half-points, frozen at
    // pairing time and stamped on every board of the match; flip it for team 2.
    const diff = round.boards[match.boards[0]]?.points_diff ?? 0;
    return {
      kind: "team-played",
      opponent,
      opponentName,
      won: wins > losses,
      level: wins === losses,
      wins,
      losses,
      pointsDiff: isFirst ? diff : -diff,
    };
  }

  // The team sit-out cell whose picker is open, if any — the team twin of
  // `picking`, keyed by round + team.
  let pickingTeam = $state<{ roundNumber: number; team: string } | null>(null);

  const isPickingTeam = (roundNumber: number, team: string) =>
    pickingTeam?.roundNumber === roundNumber && pickingTeam?.team === team;

  function toggleTeamPicker(roundNumber: number, team: string) {
    pickingTeam = isPickingTeam(roundNumber, team) ? null : { roundNumber, team };
  }

  function chooseTeamValue(
    roundNumber: number,
    team: string,
    current: SitoutValue,
    value: SitoutValue,
  ) {
    pickingTeam = null;
    if (value !== current) onSetTeamSitoutValue?.(roundNumber, team, value);
  }

  /** A team's result cell, e.g. `3+`, `2=`, `4−^`. */
  function teamLabel(cell: TeamCell & { kind: "team-played" }): string {
    const sign = cell.level ? "=" : cell.won ? "+" : "−";
    return `${cell.opponent}${sign}${floatMarkers(cell.pointsDiff)}`;
  }

  /** The team MacMahon cell's tooltip: the breakdown of any manual bonus or
   *  penalty awarded to the team. `undefined` when it has none. */
  function teamAdjustments(team: TeamStanding) {
    return (tournament.teams ?? []).find((t) => t.id === team.team_id)?.adjustments ?? [];
  }

  function teamAdjustmentTitle(team: TeamStanding): string | undefined {
    const adjustments = teamAdjustments(team);
    if (adjustments.length === 0) return undefined;
    return adjustments.map((a) => `${a.delta > 0 ? "+" : ""}${a.delta} — ${a.reason}`).join("\n");
  }

  function teamCellTitle(cell: TeamCell & { kind: "team-played" }): string {
    return $_("resultsView.teamMatchTitle", {
      values: {
        name: cell.opponentName,
        wins: formatScore(cell.wins),
        losses: formatScore(cell.losses),
      },
    });
  }

  // The category leaders: for each category, the highest-ranked player in it
  // (rows are already in the server's ranked order). Keyed by player id → the
  // names of every category that player tops, so one ⭐ carries a tooltip
  // listing them all (a player can lead more than one).
  const categoryLeaders = $derived.by(() => {
    const leaders = new Map<string, string[]>();
    for (const cat of categories) {
      const leader = rows.find(({ player }) => (player.categories ?? []).includes(cat.id));
      if (!leader) continue;
      const led = leaders.get(leader.player.id) ?? [];
      led.push(cat.name);
      leaders.set(leader.player.id, led);
    }
    return leaders;
  });

  // The one category the referee has selected to highlight, or null for none —
  // highlighting two at once isn't useful, so picking a category replaces any
  // previous choice.
  let highlightedCategory = $state<string | null>(null);
  const filtering = $derived(highlightedCategory !== null);

  // Drop the selection if it points at a now-deleted category, so filtering
  // never stays "active" with nothing left to highlight.
  $effect(() => {
    const valid = new Set(categories.map((c) => c.id));
    if (highlightedCategory !== null && !valid.has(highlightedCategory)) {
      highlightedCategory = null;
    }
  });

  function toggleHighlight(id: string) {
    highlightedCategory = highlightedCategory === id ? null : id;
  }

  // Whether the player belongs to the currently-highlighted category.
  function isHighlighted(player: Player): boolean {
    return highlightedCategory !== null && (player.categories ?? []).includes(highlightedCategory);
  }

  type PlayedCell = {
    kind: "played";
    opponent: string;
    /** The opponent, named for the cell tooltip (see `opponentLabel`). */
    opponentName: string;
    /** This player actually won the game — drives the +/− sign and colour. */
    actualWon: boolean;
    /** Counts as a win in the standings (the giver always does). */
    effectiveWon: boolean;
    /** A draw occurred before the decisive game. */
    drawn: boolean;
    /** Present for a handicap game: the code, and whether this player conceded. */
    handicap?: { code: string; gave: boolean };
    /** Half-points this player had over the opponent at pairing time
     *  (negative = played up). */
    pointsDiff: number;
  };

  // A round the player sat out — a bye of some sort, or an absence — showing
  // what it scored (`0+` / `0=` / `0−`). The only editable cell: clicking it
  // re-scores that one round for that one player.
  type SitoutCell = {
    kind: "sitout";
    /** Which round it belongs to, for the re-scoring call. */
    roundNumber: number;
    player: number;
    sitout: Sitout;
  };

  type Cell =
    | SitoutCell
    // A no-show board: this player was the absentee (`0#`) or the one who
    // showed up and was credited the free point, bye-style (`0+`).
    // The player missed this board. `justified` picks the cell: `0-` when they
    // were absent for a reason, `0#` when they simply didn't turn up.
    | { kind: "no-show"; opponentName: string; justified: boolean }
    | { kind: "no-show-win"; opponentName: string }
    | { kind: "pending"; opponent: string; opponentName: string }
    // The inert half of a carried long game, drawn on its own — which only
    // happens if the other half is missing, a save the server will not load.
    // Shows `0−`: this round holds no result, and the cell must not imply one.
    | { kind: "long-pending"; opponentName: string }
    // The player wasn't in this round at all. Reachable only from a file that
    // predates late joiners being marked absent for the rounds they missed.
    | { kind: "not-in-round" }
    | PlayedCell;

  // How this player's row is laid out across the round columns: one cell per
  // round, except for a long game, which is drawn once in a cell straddling the
  // two rounds it was played over. The layout rule is `lib/crossTable.ts` — it
  // is the part that has been wrong twice, and there it can be tested.
  function columnsFor(player: Player): CrossTableColumn[] {
    return crossTableColumns(shownRounds, tid(player));
  }

  // The extra tooltip line on a cell that covers two round columns. The cell
  // shows one result where the row otherwise shows one per round, so it has to
  // say which rounds it accounts for — the reader is counting columns.
  function longSpanNote(column: CrossTableColumn): string {
    if (column.span !== 2) return "";
    return `\n${$_("resultsView.longGameSpanTitle", {
      values: {
        from: shownRounds[column.startRound].number,
        to: shownRounds[column.readRound].number,
      },
    })}`;
  }

  function cellFor(player: Player, round: Round): Cell {
    const pid = tid(player);
    // No board: a bye or an absence, showing whatever it was scored.
    const sitout = (round.sitouts ?? []).find((s) => s.player === pid);
    if (sitout)
      return { kind: "sitout", roundNumber: round.number, player: pid, sitout };
    const boardIdx = round.boards.findIndex((b) => b.player1 === pid || b.player2 === pid);
    if (boardIdx < 0) return { kind: "not-in-round" };
    return cellForBoard(player, round, round.boards[boardIdx], boardIdx);
  }

  /** The cross-table cell for a specific board (played / no-show / pending). */
  function cellForBoard(player: Player, round: Round, board: Board, boardIdx: number): Cell {
    const isP1 = board.player1 === tid(player);
    const side: Winner = isP1 ? "player1" : "player2";
    const opponentTid = isP1 ? board.player2 : board.player1;
    const opponent = String(opponentTid);
    const opponentName = opponentLabel(opponentTid);
    // The inert half of a carried long game, reached on its own. Normally it is
    // never drawn: the straddling cell reads the round holding the live record
    // instead. Getting here means a save with one half of a carried game, which
    // the server refuses to load — so say "no result here" rather than let
    // `outcomeOf` report the record's absent outcome as a game still in play.
    if (board.record?.kind === "long_carried") {
      return { kind: "long-pending", opponentName };
    }
    const forfeit = forfeitOf(board);
    if (forfeit) {
      // This side missed the board for a single absence on their side, or when
      // both players did; otherwise they are the one who showed up.
      const kind = absenceKind(forfeit, side);
      return kind
        ? { kind: "no-show", opponentName, justified: kind === "justified" }
        : { kind: "no-show-win", opponentName };
    }
    if (!winnerOf(board)) return { kind: "pending", opponent, opponentName };

    const { actualWon, gave } = boardOutcome(board, side);
    // Server-computed, Wiel-rule-aware winner for this board (see
    // `TournamentResponse.effective_winners`).
    const roundIdx = roundIndexByNumber.get(round.number);
    const effectiveWon =
      roundIdx != null ? effectiveWinners[roundIdx]?.[boardIdx] === side : actualWon;
    const handicap: PlayedCell["handicap"] = board.handicap
      ? { code: board.handicap.handicap, gave }
      : undefined;
    // points_diff = points(player1) − points(player2) in half-points, frozen at
    // pairing time. From this player's own perspective: a positive diff means
    // player1 outranked player2, i.e. player1 played down (▼); the sign flips
    // for player2.
    const diff = board.points_diff ?? 0;
    const pointsDiff = isP1 ? diff : -diff;

    return {
      kind: "played",
      opponent,
      opponentName,
      actualWon,
      effectiveWon,
      drawn: drawnOf(board),
      handicap,
      pointsDiff,
    };
  }

  /** The three values a sit-out can take, in the order the picker lists them. */
  const SITOUT_VALUES: SitoutValue[] = ["full", "half", "zero"];

  /** The cross-table token for a sit-out value: `0+`, `0=` or `0−`. */
  function sitoutLabel(value: SitoutValue): string {
    return { full: "0+", half: "0=", zero: "0−" }[value];
  }

  /** Whether a sit-out is a bye of some kind (as opposed to an absence) — it
   *  colours the cell and names it in the tooltip. */
  function isBye(sitout: Sitout): boolean {
    return sitout.kind !== "absent";
  }

  /** The tooltip for a sit-out cell: why the side sat out, what it scored, and
   *  (when editable) that it can be clicked. Shared by the player cells and a
   *  team's own, which sits out for the same reasons. */
  function sitoutTitle(sitout: Sitout, editable: boolean): string {
    const kind = sitout.kind;
    const reason =
      kind === "absent"
        ? $_("resultsView.sitoutAbsent")
        : kind === "forced_bye"
          ? $_("resultsView.sitoutForcedBye")
          : kind === "bye"
            ? $_("resultsView.sitoutBye")
            : $_("resultsView.sitoutCupBye");
    const worth = $_(`resultsView.sitoutWorth.${sitout.value}`);
    return editable ? `${reason}\n${worth}\n${$_("resultsView.sitoutEditHint")}` : `${reason}\n${worth}`;
  }

  // The sit-out cell whose picker is open, if any. Keyed by round + player,
  // since that pair identifies a cell uniquely.
  let picking = $state<{ roundNumber: number; player: number } | null>(null);

  /** Whether the per-player sit-out cells are clickable. Never in team mode:
   *  every member of a team sits out with the same value, and the team's score
   *  is read from entries that must agree — the team row's cell edits them
   *  together. */
  const editablePlayerSitouts = $derived(onSetSitoutValue != null && !teamMode);

  const isPicking = (cell: SitoutCell) =>
    picking?.roundNumber === cell.roundNumber && picking?.player === cell.player;

  function togglePicker(cell: SitoutCell) {
    picking = isPicking(cell)
      ? null
      : { roundNumber: cell.roundNumber, player: cell.player };
  }

  function chooseValue(cell: SitoutCell, value: SitoutValue) {
    picking = null;
    if (value !== cell.sitout.value) {
      onSetSitoutValue?.(cell.roundNumber, cell.player, value);
    }
  }

  /**
   * Ascending/descending floater markers, e.g. `^`, `vv` — one per point of gap.
   *
   * `points_diff` is in **half-points** (the unit scores are kept in), so it is
   * halved here: counting it raw gave every ordinary one-point float two
   * markers. Rounded up, so a half-point gap — which a `0=` bye or a
   * MacMahon half can produce — still shows as the float it is.
   */
  function floatMarkers(pointsDiff: number): string {
    if (pointsDiff === 0) return "";
    const marker = pointsDiff < 0 ? "^" : "v";
    return marker.repeat(Math.ceil(Math.abs(pointsDiff) / 2));
  }

  /** The results-cell label, e.g. `3+`, `4=−`, `3+(+4p)`, `4=+(−2p)^`. */
  function playedLabel(cell: PlayedCell): string {
    const draw = cell.drawn ? "=" : "";
    const sign = cell.actualWon ? "+" : "−";
    const hc = cell.handicap
      ? `(${cell.handicap.gave ? "−" : "+"}${cell.handicap.code})`
      : "";
    // In team mode a board's float is the *team's* — the engine paired teams —
    // so it belongs to the team's row and nowhere else.
    const floats = teamMode ? "" : floatMarkers(cell.pointsDiff);
    return `${cell.opponent}${draw}${sign}${hc}${floats}`;
  }

  /** Tooltip for the MacMahon cell: which estimated ELO these starting points
   * were awarded from, when that isn't simply the player's registration rating,
   * and the breakdown of any manual bonus/penalty adjustments. `undefined` when
   * neither applies (the column header already says what the plain value is). */
  function macmahonTitle(player: Player, standing: Standing): string | undefined {
    const lines: string[] = [];
    // Under estimate-based MacMahon the thresholds were compared against the live
    // estimate. Say so only where it differs from the registration rating — an
    // unrated player (who has none), or a rated one the estimate has moved off it.
    // `estimated_elo` is null for the players the server didn't estimate at all.
    const estimate = standing.estimated_elo;
    if (macmahonFromEstimate && estimate != null && estimate !== player.rating) {
      lines.push($_("resultsView.macmahonFromEstimate", { values: { elo: estimate } }));
    }
    for (const a of player.adjustments ?? []) {
      lines.push(`${a.delta > 0 ? "+" : ""}${a.delta} — ${a.reason}`);
    }
    return lines.length > 0 ? lines.join("\n") : undefined;
  }

  /** A tie-break value, and the player who contributed it — `1½ (Doe Jane)`.
   * Every quantity reaching here — points, wins, and the sums over them — is in
   * ×2 units, so they all render through {@link formatScore}. The `count`
   * multiplies the contribution, so a two-round (long) game — which records the
   * opponent twice — shows once with the doubled score. */
  function opponentTerm(id: string, field: keyof Standing, count = 1): string {
    const value = ((standingById.get(id)?.[field] as number | undefined) ?? 0) * count;
    return `${formatScore(value)} (${nameOf(id)})`;
  }

  /** Group repeated opponent ids (a long game records the same opponent twice)
   * into one entry per opponent, keeping first-occurrence order. */
  function groupIds(ids: string[]): { id: string; count: number }[] {
    const out: { id: string; count: number }[] = [];
    for (const id of ids) {
      const existing = out.find((e) => e.id === id);
      if (existing) existing.count += 1;
      else out.push({ id, count: 1 });
    }
    return out;
  }

  /** Join the opponents' contributions to an opponent-sum tie-break, e.g.
   * `3 (Doe Jane) + 2 (Roe Max)`; a placeholder when there are none yet. A long
   * game's opponent appears once with its score doubled. */
  function sumTerms(ids: string[], field: keyof Standing): string {
    if (ids.length === 0) return $_("resultsView.tiebreakNoOpponents");
    return groupIds(ids)
      .map(({ id, count }) => opponentTerm(id, field, count))
      .join(" + ");
  }

  /** Like [`sumTerms`] but for the Buchholz-cut metrics: sort the opponents by
   * their contribution, drop the `drop` lowest (noting who), and sum the rest —
   * mirroring the server's `sum_dropping_lowest`. */
  function droppedTerms(ids: string[], field: keyof Standing, drop: number): string {
    if (ids.length === 0) return $_("resultsView.tiebreakNoOpponents");
    const terms = ids.map((id) => ({
      term: opponentTerm(id, field),
      value: (standingById.get(id)?.[field] as number | undefined) ?? 0,
    }));
    const { kept, dropped } = partitionDropped(terms, (t) => t.value, drop);
    const keptStr = kept.length > 0 ? kept.map((t) => t.term).join(" + ") : "0";
    if (dropped.length === 0) return keptStr;
    return `${keptStr} ${$_("resultsView.tiebreakDropped", { values: { dropped: dropped.map((t) => t.term).join(", ") } })}`;
  }

  /** The per-cell explanation of how a tie-break was computed, appended under
   * the metric's description in the tooltip. `undefined` where a breakdown
   * doesn't apply (Points, which has its own columns; the ELO estimate). */
  function tiebreakBreakdown(code: Tiebreak, standing: Standing): string | undefined {
    switch (code) {
      case "sos_m":
        return sumTerms(standing.opponents, "points");
      case "sos_w":
        return sumTerms(standing.opponents, "victories");
      case "sodos_m":
        return sumTerms(standing.defeated, "points");
      case "sodos_w":
        return sumTerms(standing.defeated, "victories");
      case "sosos_m":
        return sumTerms(standing.opponents, "sosm");
      case "sosos_w":
        return sumTerms(standing.opponents, "sosw");
      case "sos_m1":
        return droppedTerms(standing.opponents, "points", 1);
      case "sos_m2":
        return droppedTerms(standing.opponents, "points", 2);
      case "sos_w1":
        return droppedTerms(standing.opponents, "victories", 1);
      case "sos_w2":
        return droppedTerms(standing.opponents, "victories", 2);
      case "cuss_m":
        return standing.running_points.length > 0
          ? standing.running_points.map((p) => formatScore(p)).join(" + ")
          : undefined;
      case "cuss_w":
        return standing.running_wins.length > 0
          ? standing.running_wins.map((w) => formatScore(w)).join(" + ")
          : undefined;
      case "points":
      case "est_elo":
        return undefined;
    }
  }

  /** A tie-break column's header tooltip. Estimated ELO carries the extra "why
   * is this here?" explanation; every other metric is just its description. */
  function columnTitle(code: Tiebreak): string {
    return code === "est_elo" ? estEloTitle : tiebreakTitle(code, $_);
  }

  /** The full tooltip for a tie-break cell: the metric's description, plus its
   * per-player breakdown when one applies. */
  function tiebreakCellTitle(code: Tiebreak, standing: Standing): string {
    const title = columnTitle(code);
    const breakdown = tiebreakBreakdown(code, standing);
    return breakdown ? `${title}\n${breakdown}` : title;
  }

  // Native `title` tooltips only surface after a long browser delay. Drive our
  // own instead: one hover handler on the table reads the `data-tip` of the cell
  // under the cursor and shows a floating label immediately, following the mouse.
  let tip = $state<{ text: string; x: number; y: number; below: boolean } | null>(null);

  function trackTip(event: MouseEvent) {
    const el = (event.target as HTMLElement | null)?.closest?.(
      "[data-tip]",
    ) as HTMLElement | null;
    const text = el?.dataset.tip;
    if (!text) {
      tip = null;
      return;
    }
    // Clamp horizontally so a wide label stays on-screen; flip above the cursor
    // near the bottom edge so a tall breakdown isn't clipped.
    tip = {
      text,
      x: Math.max(8, Math.min(event.clientX + 14, window.innerWidth - 348)),
      y: event.clientY,
      below: event.clientY < window.innerHeight - 220,
    };
  }

  function clearTip() {
    tip = null;
  }
</script>

{#if tournament.players.length === 0}
  <p class="muted">{$_("resultsView.noPlayers")}</p>
{:else}
  <div class="results">
  {#if !staticPage}
    <div class="results-toolbar print-hide">
      <button type="button" class="ghost" onclick={() => printPage(true)}>🖨 {$_("roundView.print")}</button>
    </div>
  {/if}
  {#if cupPodium && (cupPodium.champion || cupPodium.runner_up || cupPodium.third)}
    <div class="podium">
      <span class="cup-title">{$_("resultsView.cup")}</span>
      {#if cupPodium.champion}
        <span>🥇 <strong>{nameOfTid(cupPodium.champion)}</strong></span>
      {/if}
      {#if cupPodium.runner_up}
        <span>🥈 {nameOfTid(cupPodium.runner_up)}</span>
      {/if}
      {#if cupPodium.third}
        <span>🥉 {nameOfTid(cupPodium.third)}</span>
      {/if}
    </div>
  {/if}
  {#if categories.length > 0 && !staticPage}
    <div class="category-filter print-hide">
      <span class="filter-label">{$_("resultsView.highlightCategory")}</span>
      {#each categories as cat (cat.id)}
        <button
          type="button"
          class="cat-chip"
          class:active={highlightedCategory === cat.id}
          onclick={() => toggleHighlight(cat.id)}
        >{cat.name}</button>
      {/each}
      {#if filtering}
        <button
          type="button"
          class="cat-chip clear"
          onclick={() => (highlightedCategory = null)}
        >{$_("resultsView.clearHighlight")}</button>
      {/if}
    </div>
  {/if}
  <!-- The live columns show results the ranking hasn't taken in yet, so say so
       rather than leaving the referee to wonder why a recorded win moved
       nothing — above the table, where it is read before the numbers it
       qualifies rather than after. It supersedes the "nothing completed yet"
       note, which says less and would only ever appear alongside it (a round is
       in progress in both cases); that note survives for the completed-but-empty
       table. -->
  {#if liveRounds.length === 1}
    <p class="muted note">
      {$_("resultsView.roundInProgressNote", { values: { number: liveRounds[0].number } })}
    </p>
  {:else if liveRounds.length > 1}
    <p class="muted note">
      {$_("resultsView.roundsInProgressNote", {
        values: { numbers: liveRounds.map((r) => r.number).join(", ") },
      })}
    </p>
  {:else if completedCount === 0}
    <p class="muted note">
      {$_("resultsView.noRoundsCompleted")}
    </p>
  {/if}

  <table class:teamed={teamMode} onmousemove={trackTip} onmouseleave={clearTip}>
    <thead>
      <tr>
        <th class="num pin-id">{$_("resultsView.id")}</th>
        <th class="pin-name">{$_("resultsView.lastName")}</th>
        <th>{$_("resultsView.firstName")}</th>
        <th class="num">{$_("resultsView.rating")}</th>
        {#if showEstimatedElo}
          <th class="num" data-tip={estEloTitle}>{tiebreakLabel("est_elo", $_)}</th>
        {/if}
        <th>{$_("resultsView.nationality")}</th>
        <th>{$_("resultsView.club")}</th>
        {#each shownRounds as round (round.number)}
          {#if round.completed}
            <th class="num mid">{$_("resultsView.roundColumn", { values: { number: round.number } })}</th>
          {:else}
            <th
              class="num mid live-round"
              data-tip={$_("resultsView.roundInProgressTitle", { values: { number: round.number } })}
              >{$_("resultsView.roundColumnLive", { values: { number: round.number } })}</th
            >
          {/if}
        {/each}
        {#if showVictories}
          <th class="num mid">{$_("resultsView.victories")}</th>
        {/if}
        {#if showMacmahon}
          <th class="num mid" data-tip={$_("resultsView.macmahonTitle")}
            >{$_("resultsView.macmahon")}</th
          >
        {/if}
        {#each tiebreakColumns as col (col.code)}
          <th class="num mid" data-tip={columnTitle(col.code)}>{tiebreakLabel(col.code, $_)}</th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each tableRows as row (row.key)}
        {#if row.kind === "team"}
          {@const team = row.team}
          <tr class="team-row">
            <td class="num pin-id">{team.tournament_id}</td>
            <!-- The team's name stands where its players' names are. It sits in
                 the *name* column rather than straddling the whole identity
                 band, so it can be pinned by exactly the rule that pins a
                 player's surname: a sticky element cannot leave its own cell, so
                 a name in a cell six columns wide gets carried off-screen with
                 it. The rest of the band is one empty cell, which keeps the row
                 looking the same. -->
            <td class="team-name pin-name">{team.name}</td>
            {#if identityColumns > 1}
              <td colspan={identityColumns - 1}></td>
            {/if}
            {#each shownRounds as round, i (round.number)}
              {@const cell = teamCell(team, i)}
              <td class="num result">
                {#if cell.kind === "team-played"}
                  <span
                    class={cell.level ? "pending" : cell.won ? "win" : "loss"}
                    data-tip={teamCellTitle(cell)}>{teamLabel(cell)}</span
                  >
                {:else if cell.kind === "team-pending"}
                  <span class="pending" data-tip={cell.opponentName}>{cell.opponent}?</span>
                {:else if cell.kind === "team-sitout"}
                  {@const tone = isBye(cell.sitout) && cell.sitout.value === "full" ? "win" : "absent"}
                  {#if onSetTeamSitoutValue}
                    <span class="sitout-cell">
                      <button
                        type="button"
                        class="sitout {tone}"
                        data-testid="team-sitout-{cell.roundNumber}-{team.tournament_id}"
                        aria-haspopup="true"
                        aria-expanded={isPickingTeam(cell.roundNumber, team.team_id)}
                        data-tip={sitoutTitle(cell.sitout, true)}
                        onclick={() => toggleTeamPicker(cell.roundNumber, team.team_id)}
                        >{sitoutLabel(cell.sitout.value)}</button
                      >
                      {#if isPickingTeam(cell.roundNumber, team.team_id)}
                        <span class="sitout-picker print-hide" role="menu">
                          {#each SITOUT_VALUES as value (value)}
                            <button
                              type="button"
                              role="menuitem"
                              class:current={value === cell.sitout.value}
                              data-tip={$_(`resultsView.sitoutWorth.${value}`)}
                              onclick={() =>
                                chooseTeamValue(
                                  cell.roundNumber,
                                  team.team_id,
                                  cell.sitout.value,
                                  value,
                                )}>{sitoutLabel(value)}</button
                            >
                          {/each}
                        </span>
                      {/if}
                    </span>
                  {:else}
                    <span class={tone} data-tip={sitoutTitle(cell.sitout, false)}
                      >{sitoutLabel(cell.sitout.value)}</span
                    >
                  {/if}
                {:else}
                  <span class="absent" data-tip={$_("resultsView.notInRoundTitle")}>0−</span>
                {/if}
              </td>
            {/each}
            <!-- Matches won. A team's *games* won are its board-wins tie-break,
                 which has its own column when the referee ranks on it. -->
            {#if showVictories}
              <td class="num victories">{formatScore(team.victories)}</td>
            {/if}
            {#if showMacmahon}
              <td
                class="num macmahon"
                class:adjusted={teamAdjustments(team).length > 0}
                data-tip={teamAdjustmentTitle(team)}>{formatScore(team.macmahon)}</td
              >
            {/if}
            {#each tiebreakColumns as col (col.code)}
              <td
                class="num"
                class:points={col.code === "points"}
                class:tiebreak={col.code !== "points"}
                data-tip={columnTitle(col.code)}>{teamValue(team, col.code)}</td
              >
            {/each}
          </tr>
        {:else}
          {@const standing = row.standing}
          {@const player = row.player}
          <tr
            class:member-row={teamMode}
            class:cat-highlight={filtering && isHighlighted(player)}
            class:cat-dim={filtering && !isHighlighted(player)}
          >
          <td class="num pin-id">{player.tournament_id ?? "—"}</td>
          <td class="last-name pin-name">{#if player.tournament_id != null && medalOf.has(player.tournament_id)}<span class="medal">{medalOf.get(player.tournament_id)}</span> {/if}{#if categoryLeaders.has(player.id)}<span class="cat-star" title={$_("resultsView.categoryLeader", { values: { categories: categoryLeaders.get(player.id)?.join(", ") } })}>⭐</span> {/if}{player.last_name}</td>
          <td>{player.first_name || "—"}</td>
          <td class="num">{player.rating ?? "—"}</td>
          {#if showEstimatedElo}
            <td class="num est-elo" data-tip={estEloTitle}>{standing.estimated_elo ?? "—"}</td>
          {/if}
          <td>{player.nationality ?? "—"}</td>
          <td>{player.club ?? "—"}</td>
          {#each columnsFor(player) as column, i (shownRounds[i].number)}
            {#if column.span !== 0}
              {@const cell = cellFor(player, shownRounds[column.readRound])}
              {@const straddles = column.span === 2}
              <td class="num result" class:long-span={straddles} colspan={column.span}>
              {#if cell.kind === "sitout"}
                {@const tone = isBye(cell.sitout) && cell.sitout.value === "full" ? "win" : "absent"}
                {#if editablePlayerSitouts}
                  <span class="sitout-cell">
                    <button
                      type="button"
                      class="sitout {tone}"
                      data-testid="sitout-{cell.roundNumber}-{cell.player}"
                      aria-haspopup="true"
                      aria-expanded={isPicking(cell)}
                      data-tip={sitoutTitle(cell.sitout, true)}
                      onclick={() => togglePicker(cell)}>{sitoutLabel(cell.sitout.value)}</button
                    >
                    {#if isPicking(cell)}
                      <span class="sitout-picker print-hide" role="menu">
                        {#each SITOUT_VALUES as value (value)}
                          <button
                            type="button"
                            role="menuitem"
                            class:current={value === cell.sitout.value}
                            data-testid="sitout-option-{value}"
                            data-tip={$_(`resultsView.sitoutWorth.${value}`)}
                            onclick={() => chooseValue(cell, value)}>{sitoutLabel(value)}</button
                          >
                        {/each}
                      </span>
                    {/if}
                  </span>
                {:else}
                  <span class={tone} data-tip={sitoutTitle(cell.sitout, false)}
                    >{sitoutLabel(cell.sitout.value)}</span
                  >
                {/if}
              {:else if cell.kind === "not-in-round"}
                <span class="absent" data-tip={$_("resultsView.notInRoundTitle")}>0−</span>
              {:else if cell.kind === "long-pending"}
                <span
                  class="pending"
                  data-tip={$_("resultsView.longPendingTitle", {
                    values: { name: cell.opponentName },
                  })}>0−</span
                >
              {:else if cell.kind === "no-show"}
                <span
                  class="absent"
                  data-tip={$_(
                    cell.justified ? "resultsView.justifiedTitle" : "resultsView.noShowTitle",
                    { values: { name: cell.opponentName } },
                  ) + longSpanNote(column)}>{cell.justified ? "0−" : "0#"}</span
                >
              {:else if cell.kind === "no-show-win"}
                <span
                  class="win"
                  data-tip={$_("resultsView.noShowWinTitle", {
                    values: { name: cell.opponentName },
                  }) + longSpanNote(column)}>0+</span
                >
              {:else if cell.kind === "pending"}
                <!-- The opponent alone, unprefixed: the cell already says this
                     is a game against them, so a "vs" in front of the name is
                     a word the referee reads past on every single hover. -->
                <span class="pending" data-tip={cell.opponentName + longSpanNote(column)}
                  >{cell.opponent}?</span
                >
              {:else}
                <span
                  class={cell.actualWon ? "win" : "loss"}
                  data-tip={(cell.handicap && cell.effectiveWon !== cell.actualWon
                    ? `${cell.opponentName}\n${$_(
                        cell.effectiveWon
                          ? "resultsView.handicapGameWin"
                          : "resultsView.handicapGameLoss",
                      )}`
                    : cell.opponentName) + longSpanNote(column)}>{playedLabel(cell)}</span
                >
                {/if}
              </td>
            {/if}
          {/each}
          {#if teamMode}
            <!-- Wins counts *matches*, and the ranking criteria rank teams: a
                 player's own figures are not their team's, and lining them up
                 under those columns would read as if they were. One blank per
                 column the team rows carry, so the two stay in step. -->
            {#if showVictories}<td></td>{/if}
            {#if showMacmahon}<td></td>{/if}
            {#each tiebreakColumns as col (col.code)}
              <td></td>
            {/each}
          {:else}
            {#if showVictories}
              <td class="num victories">{formatScore(standing.victories)}</td>
            {/if}
            {#if showMacmahon}
              <td
                class="num macmahon"
                class:adjusted={(player.adjustments ?? []).length > 0}
                data-tip={macmahonTitle(player, standing)}>{formatScore(standing.macmahon)}</td
              >
            {/if}
            {#each tiebreakColumns as col (col.code)}
              {#if col.code === "points"}
                <td class="num points" data-tip={$_("resultsView.victoriesPlusMacmahon")}
                  >{formatScore(standing.points)}</td
                >
              {:else}
                <td class="num tiebreak" data-tip={tiebreakCellTitle(col.code, standing)}
                  >{HALF_POINT_TIEBREAKS.has(col.code)
                    ? formatScore(standing[col.field] as number)
                    : standing[col.field]}</td
                >
              {/if}
            {/each}
          {/if}
        </tr>
        {/if}
      {/each}
    </tbody>
  </table>

  </div>
{/if}

{#if tip}
  <div
    class="cell-tip print-hide"
    style="left: {tip.x}px; top: {tip.y}px; transform: translateY({tip.below
      ? '18px'
      : 'calc(-100% - 14px)'});"
  >
    {tip.text}
  </div>
{/if}

<style>
  .results-toolbar {
    display: flex;
    justify-content: flex-end;
    margin-bottom: 0.5rem;
  }
  .results {
    /* The width of the pinned ID column, shared by the two rules that need to
       agree on it: the column itself, and the offset the name column pins at.
       It is a *border-box* width (see `.pin-id`), so the padding comes out of
       it — leave room for four digits and the "ID" header, or the cell grows
       past it and the two stop agreeing again. The numbers are right-aligned,
       so any slack here shows up as a gap down the left edge of the table;
       `.pin-id` trims its left padding to keep the budget without it. */
    --id-col: 3.25rem;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.9rem;
  }
  th,
  td {
    padding: 0.3rem 0.55rem;
    border-bottom: 1px solid var(--border-divider);
    text-align: left;
    white-space: nowrap;
  }
  th {
    color: var(--text-secondary);
    font-weight: 600;
    font-size: 0.8rem;
  }

  /* --- Pinned header row and identity columns -----------------------------
   *
   * `position: sticky` resolves against the nearest *scrolling* ancestor, and
   * here that is deliberately the document itself: nothing between this table
   * and <html> establishes a scrollport. Wrapping the table in the obvious
   * `overflow-x: auto` would pin the header to that wrapper instead of to the
   * window, which is the opposite of what this is for — so the table is left to
   * overflow, and `.card.wide-table` in App.svelte grows to cover it.
   *
   * With `border-collapse: collapse` a cell's border belongs to the *table*, so
   * it stays behind while the sticky cell travels. The rules that separate a
   * pinned cell from what scrolls under it are therefore inset shadows, which
   * are painted by the cell and travel with it.
   */
  thead th {
    position: sticky;
    top: 0;
    z-index: 2;
    background: var(--bg-surface);
    box-shadow: inset 0 -1px 0 var(--border-divider);
  }
  .pin-id,
  .pin-name {
    position: sticky;
    z-index: 1;
    /* From the row, so a pinned cell keeps its stripe, its team-row tint or its
       category highlight instead of punching a plain hole through them. */
    background: inherit;
  }
  .pin-id {
    left: 0;
    /* Fixed, because the name column's offset has to be a length the stylesheet
       knows — the table lays itself out from its content, and `left: auto` is
       not a thing for a sticky cell. Wide enough for a four-digit number.

       `border-box` is what makes the two agree. This app sets no global
       box-sizing reset, so a cell is `content-box` by default and `width` would
       exclude the 0.55rem of padding on each side: the column would be
       `--id-col` *plus* 1.1rem, while the name column still pinned at
       `--id-col` — landing on top of the last 1.1rem of the numbers. */
    box-sizing: border-box;
    width: var(--id-col);
    /* The table starts at the card's edge, so this cell's left padding is
       stacked on top of the card's own — and the numbers are right-aligned, so
       all of it reads as a gap before the table begins. */
    padding-left: 0.25rem;
  }
  .pin-name {
    left: var(--id-col);
    box-shadow: inset -1px 0 0 var(--border-divider);
  }
  /* The corner cells are pinned on both axes, so they must sit above both the
     header row and the identity columns — and be opaque, which they do not get
     from the rule above: `.pin-id` is a class and `thead th` is two element
     selectors, so `background: inherit` wins there, and a header `tr` has no
     background to inherit. The result is a transparent corner with the whole
     table sliding visibly through it. */
  thead .pin-id,
  thead .pin-name {
    z-index: 3;
    background: var(--bg-surface);
  }
  thead .pin-name {
    box-shadow:
      inset 0 -1px 0 var(--border-divider),
      inset -1px 0 0 var(--border-divider);
  }
  /* Below this the pinned columns would eat the screen rather than help: two
     frozen columns leave a phone under ~384px with nothing to scroll. */
  @media (max-width: 24rem) {
    .pin-id,
    .pin-name {
      position: static;
    }
    .pin-name {
      box-shadow: none;
    }
  }
  /* Every row carries an explicit background, including the un-striped ones.
     The pinned identity columns take theirs with `background: inherit`, so a
     row left transparent would let the scrolling columns show through the
     pinned ones. */
  tbody tr {
    background: var(--bg-surface);
  }
  tbody tr:nth-child(even) {
    background: var(--bg-stripe);
  }
  /* In team mode the rows come in blocks — a team, then its players — so
     striping every other row would cut across them. The team row is the
     separator instead. */
  table.teamed tbody tr:nth-child(even) {
    background: var(--bg-surface);
  }
  .team-row {
    background: var(--bg-stripe);
  }
  .team-row td {
    border-top: 2px solid var(--border);
  }
  .team-name {
    font-weight: 700;
  }
  .member-row .last-name {
    padding-left: 1.4rem;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .result {
    font-variant-numeric: tabular-nums;
  }
  /* Everything scored — the per-round results and every number derived from
     them — is centred in its column; what merely *describes* a player (their
     number, rating, ELO estimate) stays right-aligned, where a column of
     figures lines up digit under digit and is easy to compare down the page.
     Wins and the MacMahon start are centred with the tie-breaks rather than
     against them: they sit between the rounds and the tie-break columns, and
     one right-aligned column in the middle of centred ones reads as a mistake.
     Placed after `.num`, whose `text-align: right` it has to beat on equal
     specificity. */
  .result,
  .victories,
  .macmahon,
  .points,
  .tiebreak,
  th.mid {
    text-align: center;
  }
  /* A long game is one game played over two rounds, so it gets one cell over
     two columns. Without a mark it just looks like a result that has drifted
     off the column grid, which is exactly the misreading the old layout caused
     for real — so the cell is bracketed by its own side rules, saying "these
     two rounds, one game". Kept to borders and a faint tint rather than a
     background block: this table is read as a grid, and a filled cell in the
     middle of it pulls the eye away from the rankings it exists to show. */
  .long-span {
    border-left: 1px solid var(--color-border);
    border-right: 1px solid var(--color-border);
    background: color-mix(in srgb, var(--color-text) 4%, transparent);
  }
  .win {
    color: var(--color-success);
  }
  .loss {
    color: var(--color-danger);
  }
  /* A sit-out cell is a button, but reads as the plain token until hovered. */
  .sitout-cell {
    position: relative;
    display: inline-block;
  }
  button.sitout {
    padding: 0 0.15rem;
    border: 1px solid transparent;
    border-radius: 0.25rem;
    background: none;
    font: inherit;
    font-variant-numeric: tabular-nums;
    color: inherit;
    cursor: pointer;
  }
  button.sitout:hover,
  button.sitout[aria-expanded="true"] {
    border-color: var(--border-divider);
    background: var(--bg-stripe);
  }
  .sitout-picker {
    position: absolute;
    z-index: 900;
    top: calc(100% + 0.2rem);
    right: 0;
    display: flex;
    gap: 0.15rem;
    padding: 0.2rem;
    border: 1px solid var(--border-divider);
    border-radius: 0.35rem;
    background: var(--bg-surface);
    box-shadow: 0 4px 14px var(--shadow-dropdown);
  }
  .sitout-picker button {
    padding: 0.1rem 0.35rem;
    border: 1px solid transparent;
    border-radius: 0.25rem;
    background: none;
    font: inherit;
    font-variant-numeric: tabular-nums;
    color: var(--text);
    cursor: pointer;
  }
  .sitout-picker button:hover {
    background: var(--bg-stripe);
  }
  .sitout-picker button.current {
    border-color: var(--color-accent-strong);
    font-weight: 700;
  }
  .absent {
    color: var(--text-tertiary);
  }
  .pending {
    color: var(--color-warning);
  }
  /* The in-progress round's header, in the same tone as the pending cells
     below it: the column is live, and none of it counts yet. */
  th.live-round {
    color: var(--color-warning);
  }
  .victories {
    font-weight: 600;
  }
  .points {
    font-weight: 700;
    color: var(--color-purple);
  }
  .macmahon {
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
  }
  .macmahon.adjusted {
    text-decoration: underline dotted;
    text-underline-offset: 0.2rem;
  }
  .tiebreak {
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
  }
  .est-elo {
    color: var(--color-accent-strong);
    font-variant-numeric: tabular-nums;
  }
  .podium {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.9rem;
    padding: 0.5rem 0.75rem;
    border: 1px solid var(--border-warning);
    background: var(--bg-warning);
    border-radius: 0.5rem;
    font-size: 0.9rem;
  }
  .cup-title {
    font-weight: 700;
    color: var(--color-warning-strong);
    text-transform: uppercase;
    font-size: 0.75rem;
    letter-spacing: 0.05em;
  }
  .medal {
    font-size: 0.85rem;
  }
  .cat-star {
    font-size: 0.85rem;
    cursor: help;
  }
  .category-filter {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
    margin-bottom: 0.6rem;
  }
  .filter-label {
    font-size: 0.85rem;
    color: var(--text-secondary);
  }
  .cat-chip {
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border-soft);
    border-radius: 999px;
    background: var(--bg-inset);
    color: inherit;
    font: inherit;
    font-size: 0.82rem;
    cursor: pointer;
  }
  .cat-chip:hover {
    background: var(--bg-hover);
  }
  .cat-chip.active {
    background: var(--bg-accent);
    border-color: var(--border-accent-strong);
    color: var(--text-on-accent);
  }
  .cat-chip.clear {
    border-style: dashed;
  }
  tr.cat-dim {
    opacity: 0.35;
  }
  /* Override the zebra stripe (tbody tr:nth-child(even)) so every highlighted
     row is the same color regardless of its odd/even position. */
  tbody tr.cat-highlight {
    background: var(--bg-hover-strong);
  }
  .cell-tip {
    position: fixed;
    z-index: 1000;
    max-width: 340px;
    padding: 0.35rem 0.55rem;
    border-radius: 0.4rem;
    background: var(--text);
    color: var(--bg-surface);
    font-size: 0.78rem;
    line-height: 1.4;
    white-space: pre-line;
    pointer-events: none;
    box-shadow: 0 4px 14px var(--shadow-dropdown);
  }
  .muted {
    color: var(--text-secondary);
  }
  /* Sits above the table, so the gap goes below it. */
  .note {
    font-size: 0.85rem;
    margin: 0.25rem 0 0.75rem;
    text-align: center;
  }

  /* A named page so only this tab's print job goes landscape — the table is
     wide (one column per round plus the tie-breaks), unlike the pairings view. */
  @page results-landscape {
    size: landscape;
  }

  @media print {
    .print-hide {
      display: none;
    }
    .results {
      page: results-landscape;
    }
    table,
    th,
    td,
    .win,
    .loss,
    .absent,
    .pending,
    .points,
    .macmahon,
    .tiebreak,
    .est-elo,
    .muted,
    .note,
    .podium,
    .cup-title {
      color: #000 !important;
      background: transparent !important;
      border-color: #000 !important;
    }
    tbody tr:nth-child(even) {
      background: transparent !important;
    }
    /* Paper does not scroll, so nothing has anything to stick to — and a
       sticky cell on a page break lands on top of the rows below it. */
    thead th,
    .pin-id,
    .pin-name {
      position: static;
      box-shadow: none !important;
    }
    /* The sit-out token is a <button> carrying its tone class (.win/.absent),
       both of which force border-color: #000 above. Keep its border invisible
       in print so no square is drawn around the 0+ / 0= / 0− tokens. */
    button.sitout {
      border-color: transparent !important;
    }
    /* The medals (🥇🥈🥉) and category-leader star (⭐) are colour emoji, whose
       glyph colour ignores `color: #000`. Desaturate them so the printout stays
       black & white while still marking podium places and category leaders. */
    .medal,
    .cat-star,
    .podium {
      filter: grayscale(1);
    }
  }
</style>
