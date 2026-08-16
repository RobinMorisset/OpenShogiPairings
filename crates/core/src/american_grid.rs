//! American Grid (cross-table) export.
//!
//! The text format the European federation ingests for ELO updates. A header of
//! one to three bracketed lines, then one row per player ordered by final rank,
//! each row documenting who the player faced and the result in every completed
//! round:
//!
//! ```text
//! [Paris Open, Paris, France, 2026-07-04 to 2026-07-05]
//! [2026-07-05]
//! [Time control: 30min + 30sec]
//! Nr Name          Nat Elo  1   2    Pts
//! 1  [Goix] [Théo] FR  1775 [2+ 3+]  2
//! ```
//!
//! The header follows the FESA tournament-system guide: the tournament name, its
//! city, its country and its date range on one line, then the tournament's last
//! day on its own line, then the time control. Everything but the name comes from
//! the settings ([`city`], [`country`], [`dates`], [`time_control`]) and is only
//! written when the referee filled it in — with none of them, the header is the
//! bare `[name]` line this export used to emit.
//!
//! [`city`]: crate::TournamentSettings::city
//! [`country`]: crate::TournamentSettings::country
//! [`dates`]: crate::TournamentSettings::dates
//! [`time_control`]: crate::TournamentSettings::time_control
//!
//! Columns: final rank, a "new player" flag (`N` when the player has no rating),
//! `[last] [first]`, nationality, rating, one cell per round, and total points.
//! The whole round block is wrapped in one literal bracket pair (the `[` glues to
//! the first round cell, the `]` to the last). Each column is padded to the width
//! of its widest content (header included) and separated by a single space, with
//! trailing whitespace stripped per line.
//!
//! This is a port of the reference `american_grid.py` / `formatting.py`, with one
//! deliberate addition: a drawn game is written `=` (regardless of who won the
//! replay), because OSP records draws and the ELO computation only needs to know
//! that a draw occurred. Opponents are referenced by final rank, resolved through
//! the server-computed [`Standing`] ordering.

use std::collections::HashMap;

use uuid::Uuid;

use crate::player::Player;
use crate::round::{Board, GameRecord, Round, Winner};
use crate::standings::Standing;
use crate::tournament::{Tournament, TournamentError};
use crate::units::{HalfPoints, TournamentId};

/// Columns are separated by a single space (widths carry the rest of the layout).
const COLUMN_SEP: &str = " ";

/// Render the tournament as an American Grid document.
///
/// `standings` must be the tournament's own [`Tournament::standings`], whose
/// order defines each player's final rank (and thus the opponent numbering).
///
/// # Errors
///
/// Refuses while any long game is still unresolved
/// ([`UnresolvedLongGame`](TournamentError::UnresolvedLongGame)). A long board
/// carries its result into the *next* round's column, so an unfinished one has
/// no result to put there — and since `Outcome::Pending` is neither a draw nor a
/// win, it would render as `-` for **both** players: a double loss, submitted to
/// a rating body, for a game nobody has played. There is no honest grid to emit
/// until the game is entered, so this refuses rather than guessing.
///
/// The check lives here rather than in the caller because the document is what
/// is wrong, and every route to it — the API, the desktop app, a future export —
/// must be refused alike.
pub fn to_grid(tournament: &Tournament, standings: &[Standing]) -> Result<String, TournamentError> {
    if let Some(blocked) = tournament.grid_export_blocker() {
        return Err(blocked);
    }
    let players_by_id: HashMap<Uuid, &Player> =
        tournament.players.iter().map(|p| (p.id, p)).collect();
    // Boards reference players by tournament number, so key the rank (1-based
    // standings position, used for the `Nr` column and to renumber opponents in
    // the round cells) by that number too.
    let tid_of: HashMap<Uuid, TournamentId> = tournament
        .players
        .iter()
        .filter_map(|p| p.tournament_id.map(|t| (p.id, t)))
        .collect();
    let rank_of: HashMap<TournamentId, u32> = standings
        .iter()
        .enumerate()
        .filter_map(|(i, s)| tid_of.get(&s.player_id).map(|&t| (t, i as u32 + 1)))
        .collect();
    // Every round, not just the finished ones. `grid_export_blocker` above has
    // already refused the document if any round is unfinished, so there is
    // nothing to filter — and filtering was how an unfinished round used to
    // vanish from the export instead of stopping it.
    debug_assert!(
        tournament.rounds.iter().all(|r| r.completed),
        "the blocker admits only a tournament whose rounds are all finished"
    );
    let rounds: Vec<&Round> = tournament.rounds.iter().collect();

    // Header row, then one row per player in final-rank order.
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(standings.len() + 1);
    let mut header: Vec<String> = vec![
        "Nr".into(),
        String::new(), // the "new player" flag column has a blank header
        "Name".into(),
        "Nat".into(),
        "Elo".into(),
    ];
    header.extend(rounds.iter().map(|r| r.number.to_string()));
    header.push("Pts".into());
    rows.push(header);

    for standing in standings {
        if let Some(player) = players_by_id.get(&standing.player_id) {
            rows.push(row_for(player, standing, &rounds, &rank_of));
        }
    }

    // Pad every column to its widest cell (measured in characters, matching the
    // reference), join with a single space, and strip each line's trailing space.
    let columns = rows[0].len();
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .map(|row| row[c].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();

    let mut out = header_lines(tournament);
    for row in &rows {
        let line = row
            .iter()
            .enumerate()
            .map(|(c, cell)| pad(cell, widths[c]))
            .collect::<Vec<_>>()
            .join(COLUMN_SEP);
        out.push_str(line.trim_end());
        out.push('\n');
    }
    Ok(out)
}

/// The bracketed header lines that open the document (see the module docs): the
/// `[name, city, country, dates]` line, then — each only when the referee entered
/// it — the closing date and the time control. Whatever was left empty is simply
/// absent from that first line, commas included, and a one-day event writes its
/// single date rather than a `X to X` range.
fn header_lines(tournament: &Tournament) -> String {
    let settings = &tournament.settings;
    let mut parts = vec![tournament.name.clone()];
    parts.extend(settings.city.clone());
    parts.extend(settings.country.clone());
    if let Some(dates) = &settings.dates {
        parts.push(if dates.single_day() {
            dates.first.to_string()
        } else {
            format!("{} to {}", dates.first, dates.last)
        });
    }

    let mut out = format!("[{}]\n", parts.join(", "));
    if let Some(dates) = &settings.dates {
        out.push_str(&format!("[{}]\n", dates.last));
    }
    if let Some(time_control) = &settings.time_control {
        out.push_str(&format!("[Time control: {time_control}]\n"));
    }
    out
}

/// One player's row: rank, new-player flag, name, nationality, rating, the
/// bracketed round block, and total points.
fn row_for(
    player: &Player,
    standing: &Standing,
    rounds: &[&Round],
    rank_of: &HashMap<TournamentId, u32>,
) -> Vec<String> {
    // The export runs on a finalized tournament, where every player carries a
    // number and appears in the standings this rank table was built from. Both
    // fallbacks below (number 0, which is no player, and a `?` rank) would ship a
    // malformed federation document rather than refuse it, so state the invariant
    // here — the numbering and the standings are ours, not the file's.
    debug_assert!(
        player.tournament_id.is_some(),
        "an exported player has no tournament number"
    );
    let tid = player.tournament_id.unwrap_or(TournamentId(0));
    debug_assert!(
        rank_of.contains_key(&tid),
        "exported player {tid} is missing from the standings"
    );
    let mut round_cells: Vec<String> = rounds
        .iter()
        .map(|round| round_cell(tid, round, rank_of))
        .collect();
    // Wrap the whole block in a single literal bracket pair. Done in place so a
    // one-round tournament yields `[cell]` (both edits land on the same cell).
    if !round_cells.is_empty() {
        let last = round_cells.len() - 1;
        round_cells[0] = format!("[{}", round_cells[0]);
        round_cells[last] = format!("{}]", round_cells[last]);
    }

    let mut row = vec![
        rank_of
            .get(&tid)
            .map_or_else(|| "?".into(), |r| r.to_string()),
        if player.rating.is_none() {
            "N".into()
        } else {
            String::new()
        },
        format!("[{}] [{}]", player.last_name, player.first_name),
        player.nationality.clone().unwrap_or_default(),
        player.rating.map(|r| r.to_string()).unwrap_or_default(),
    ];
    row.extend(round_cells);
    row.push(format_half_points(standing.points));
    row
}

/// Render a score held in **half-point units** for the `Pts` column: a whole
/// number when even (`6` for 6 points), or the FESA `n/2` form when odd (`5/2`
/// for 2½). Matches how FESA itself writes fractional point totals.
fn format_half_points(points: HalfPoints) -> String {
    let half = points.halves();
    if half.is_multiple_of(2) {
        (half / 2).to_string()
    } else {
        format!("{half}/2")
    }
}

/// The single round cell for a player: whatever their sit-out scored (`0+` for
/// the usual bye, `0-`/`0=` for an absence) when they played no board, `0#` /
/// `0+` for a no-show, or `<opponent><marker><handicap?>` for a game. The
/// opponent is their final rank.
fn round_cell(
    player_id: TournamentId,
    round: &Round,
    rank_of: &HashMap<TournamentId, u32>,
) -> String {
    // No board: a bye or an absence, rendered as whatever it was worth.
    if let Some(sitout) = round.sitout(player_id) {
        return sitout.value.cell().into();
    }
    let Some(board) = round
        .boards
        .iter()
        .find(|b| b.player1 == player_id || b.player2 == player_id)
    else {
        // Not in this round at all (e.g. registered later): nothing scored.
        return "0-".into();
    };
    match board.record {
        // The inert record a carried long game leaves behind: no game was settled
        // in this round and the result belongs to the `LongEnd` column, so this
        // one reads as a non-game, which is what `0-` says.
        GameRecord::LongCarried => "0-".into(),
        // Everything else is a board whose result belongs to *this* column: an
        // ordinary game, a long game finished in the round it was carried into,
        // and a long game the tournament ended on before it could be carried.
        _ => {
            // Every one of them holds a settled result, because `to_grid` refuses
            // the whole document while any long game is unresolved. Asserted
            // rather than handled: rendering an unfinished game here is precisely
            // the bug that guard exists to prevent — `Outcome::Pending` is neither
            // a draw nor a win, so `result_marker` would write `-` for *both*
            // players. A `0-` fallback would paper over a regression in the guard
            // by quietly reporting a played game as no game at all.
            debug_assert!(
                !board.long_pending(),
                "to_grid refuses while a long game is unresolved, so round \
                 {} must not still hold one",
                round.number
            );
            game_cell(board, player_id, rank_of)
        }
    }
}

/// The cell for a player's actual board: `0#`/`0+` for a no-show, otherwise
/// `<opponent-rank><marker><handicap?>`. Shared by the ordinary round rendering
/// and by the second-round column that carries a long game's result.
fn game_cell(
    board: &Board,
    player_id: TournamentId,
    rank_of: &HashMap<TournamentId, u32>,
) -> String {
    let is_player1 = board.player1 == player_id;
    let side = if is_player1 {
        Winner::Player1
    } else {
        Winner::Player2
    };

    // A forfeited board renders distinctly: the missing side's own cell says why
    // it missed — `0#` for an unjustified no-show, `0-` for an absence with a
    // reason — and the side that turned up gets `0+`, the bye-equivalent free
    // point. Under a double forfeit both cells are the absentees'.
    if let Some(forfeit) = board.outcome().forfeit() {
        return match forfeit.kind(side) {
            Some(kind) => kind.cell().into(),
            None => "0+".into(),
        };
    }

    let opponent_id = if is_player1 {
        board.player2
    } else {
        board.player1
    };
    // An opponent is a player of this tournament, so it is ranked like any other;
    // a `?` here is a malformed cell in a document a rating body ingests.
    debug_assert!(
        rank_of.contains_key(&opponent_id),
        "opponent {opponent_id} is missing from the standings"
    );
    let opponent = rank_of
        .get(&opponent_id)
        .map_or_else(|| "?".into(), |r| r.to_string());

    let mut cell = format!("{opponent}{}", result_marker(board, is_player1));
    if let Some(game) = &board.handicap {
        // `-` when this player conceded the odds (the frozen giver), `+` when
        // they received them.
        let gave = matches!(
            (game.giver, is_player1),
            (Winner::Player1, true) | (Winner::Player2, false)
        );
        let sign = if gave { '-' } else { '+' };
        cell.push_str(&format!("({sign}{})", game.handicap.code()));
    }
    cell
}

/// The result marker for a played board from `player`'s perspective: `=` if a
/// draw occurred (the ELO computation only needs that fact), otherwise `+` for a
/// win and `-` for a loss, following the *actual* game result.
fn result_marker(board: &Board, is_player1: bool) -> &'static str {
    if board.outcome().drawn() {
        return "=";
    }
    let won = matches!(
        (board.outcome().winner(), is_player1),
        (Some(Winner::Player1), true) | (Some(Winner::Player2), false)
    );
    if won {
        "+"
    } else {
        "-"
    }
}

/// Left-justify `s` to `width` characters (character count, like the reference's
/// `str.ljust`), padding with spaces. Never truncates.
fn pad(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::RoundExplanation;
    use crate::round::GameRecord;
    use crate::round::{
        AbsenceKind, CupStage, Forfeit, Handicap, HandicapGame, Outcome, PairingSource, Sitout,
        SitoutKind, SitoutValue,
    };

    /// The tournament number assigned to a registered player (only valid after
    /// `finalize_registration`).
    fn tid(t: &Tournament, id: Uuid) -> TournamentId {
        t.players
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .tournament_id
            .unwrap()
    }

    /// Build a finalized two-player tournament (P1 rated 2000, P2 1000) plus one
    /// completed round whose single board is configured by `configure`.
    fn one_board(configure: impl FnOnce(&mut Board)) -> (Tournament, TournamentId, TournamentId) {
        let mut t = Tournament::new("Test Cup").unwrap();
        let p1 = t
            .add_player(crate::player::NewPlayer {
                last_name: "Alpha".into(),
                first_name: Some("Ann".into()),
                rating: Some(2000),
                nationality: Some("fr".into()),
                ..Default::default()
            })
            .unwrap()
            .id;
        let p2 = t
            .add_player(crate::player::NewPlayer {
                last_name: "Beta".into(),
                first_name: Some("Bo".into()),
                rating: Some(1000),
                nationality: Some("jp".into()),
                ..Default::default()
            })
            .unwrap()
            .id;
        t.finalize_registration().unwrap();
        let p1 = tid(&t, p1);
        let p2 = tid(&t, p2);

        let mut board = Board {
            record: GameRecord::Short(Outcome::won(Winner::Player1)),
            ..Board::pending(p1, p2, 0, PairingSource::Swiss)
        };
        configure(&mut board);
        t.rounds.push(Round {
            number: 1,
            explanation: RoundExplanation::empty(1),
            boards: vec![board],
            sitouts: Vec::new(),
            completed: true,
        });
        (t, p1, p2)
    }

    /// Split the grid into its lines, dropping the `[name]` header and the column
    /// header so tests can look at data rows directly.
    fn data_rows(grid: &str) -> Vec<String> {
        grid.lines().skip(2).map(str::to_string).collect()
    }

    #[test]
    fn header_line_and_columns() {
        let (t, ..) = one_board(|_| {});
        let grid = to_grid(&t, &t.standings()).unwrap();
        let mut lines = grid.lines();
        assert_eq!(lines.next().unwrap(), "[Test Cup]");
        // Column header carries the round number and the totals column.
        let header = lines.next().unwrap();
        assert!(header.starts_with("Nr"));
        assert!(header.contains(" 1 ") || header.ends_with(" 1  Pts") || header.contains('1'));
        assert!(header.trim_end().ends_with("Pts"));
    }

    #[test]
    fn header_carries_the_place_dates_and_time_control_when_set() {
        use crate::settings::{IsoDate, TournamentDates};
        let day = |s: &str| IsoDate::parse(s).unwrap();

        let (mut t, ..) = one_board(|_| {});
        t.settings.city = Some("Ludwigshafen".into());
        t.settings.country = Some("Germany".into());
        t.settings.dates =
            Some(TournamentDates::new(day("2026-07-04"), day("2026-07-05")).unwrap());
        t.settings.time_control = Some("30min + 30sec".into());
        let grid = to_grid(&t, &t.standings()).unwrap();
        let mut lines = grid.lines();
        assert_eq!(
            lines.next().unwrap(),
            "[Test Cup, Ludwigshafen, Germany, 2026-07-04 to 2026-07-05]"
        );
        assert_eq!(lines.next().unwrap(), "[2026-07-05]"); // the last day, alone
        assert_eq!(lines.next().unwrap(), "[Time control: 30min + 30sec]");
        assert!(lines.next().unwrap().starts_with("Nr"));

        // A one-day event writes the day itself, not a `X to X` range.
        t.settings.dates =
            Some(TournamentDates::new(day("2026-07-04"), day("2026-07-04")).unwrap());
        let grid = to_grid(&t, &t.standings()).unwrap();
        let mut lines = grid.lines();
        assert_eq!(
            lines.next().unwrap(),
            "[Test Cup, Ludwigshafen, Germany, 2026-07-04]"
        );
        assert_eq!(lines.next().unwrap(), "[2026-07-04]");
    }

    #[test]
    fn header_omits_what_the_referee_left_empty() {
        use crate::settings::{IsoDate, TournamentDates};
        let day = |s: &str| IsoDate::parse(s).unwrap();

        // Nothing filled in: the bare name line this export has always written.
        let (mut t, ..) = one_board(|_| {});
        assert_eq!(
            to_grid(&t, &t.standings()).unwrap().lines().next().unwrap(),
            "[Test Cup]"
        );

        // Time control only: the name line stays bare, and the time control
        // follows it directly (there is no date line to hold its place).
        t.settings.time_control = Some("40min sudden death".into());
        let grid = to_grid(&t, &t.standings()).unwrap();
        let mut lines = grid.lines();
        assert_eq!(lines.next().unwrap(), "[Test Cup]");
        assert_eq!(lines.next().unwrap(), "[Time control: 40min sudden death]");
        assert!(lines.next().unwrap().starts_with("Nr"));

        // Dates only: no time-control line at all.
        t.settings.time_control = None;
        t.settings.dates =
            Some(TournamentDates::new(day("2026-07-04"), day("2026-07-05")).unwrap());
        let grid = to_grid(&t, &t.standings()).unwrap();
        let mut lines = grid.lines();
        assert_eq!(
            lines.next().unwrap(),
            "[Test Cup, 2026-07-04 to 2026-07-05]"
        );
        assert_eq!(lines.next().unwrap(), "[2026-07-05]");
        assert!(lines.next().unwrap().starts_with("Nr"));

        // A country with no city (or the reverse) leaves no hole behind: only the
        // parts present are joined, with no stray comma for the missing one.
        t.settings.dates = None;
        t.settings.country = Some("Germany".into());
        assert_eq!(
            to_grid(&t, &t.standings()).unwrap().lines().next().unwrap(),
            "[Test Cup, Germany]"
        );
    }

    #[test]
    fn win_and_loss_reference_final_rank() {
        // P1 (rank 1) beat P2 (rank 2). P1's cell reads `2+`, P2's `1-`.
        let (t, ..) = one_board(|_| {});
        let rows = data_rows(&to_grid(&t, &t.standings()).unwrap());
        assert!(rows[0].starts_with("1 "), "winner first: {rows:?}");
        assert!(rows[0].contains("[2+]"));
        assert!(rows[0].contains("[Alpha] [Ann]"));
        assert!(rows[0].contains("FR"));
        assert!(rows[0].trim_end().ends_with(" 1")); // 1 point
        assert!(rows[1].starts_with("2 "));
        assert!(rows[1].contains("[1-]"));
        assert!(rows[1].trim_end().ends_with(" 0")); // 0 points
    }

    #[test]
    fn draw_collapses_to_equals_for_both_sides() {
        // A draw occurred but the game was replayed to a P1 win. Both cells show
        // `=`, dropping the +/- entirely.
        let (t, ..) = one_board(|b| {
            b.set_outcome(Outcome::Won {
                winner: Winner::Player1,
                drawn: true,
            })
        });
        let rows = data_rows(&to_grid(&t, &t.standings()).unwrap());
        assert!(rows.iter().any(|r| r.contains("[2=]")));
        assert!(rows.iter().any(|r| r.contains("[1=]")));
        assert!(!rows.iter().any(|r| r.contains('+') || r.contains('-')));
    }

    #[test]
    fn handicap_sign_marks_giver_and_receiver() {
        // Higher-rated P1 gives a two-piece handicap: `-2p` on the giver's cell,
        // `+2p` on the receiver's. The +/- game result is unaffected.
        let (t, p1, _) = one_board(|b| {
            b.handicap = Some(HandicapGame {
                handicap: Handicap::TwoPiece,
                giver: Winner::Player1,
            });
        });
        let grid = to_grid(&t, &t.standings()).unwrap();
        let _ = p1;
        assert!(grid.contains("[2+(-2p)]"), "giver cell: {grid}");
        assert!(grid.contains("[1-(+2p)]"), "receiver cell: {grid}");
    }

    #[test]
    fn long_board_shows_placeholder_then_result_in_the_next_column() {
        // A beats B on a long (two-round) board started in round 1. The grid shows
        // `0-` in round 1 (game in progress) and the result in round 2 (where it
        // becomes known): A's block reads `[0- 2+]`, B's `[0- 1-]`.
        let mut t = Tournament::new("Long").unwrap();
        let a = t.add_player(named("A")).unwrap().id;
        let b = t.add_player(named("B")).unwrap().id;
        t.finalize_registration().unwrap();
        let a = tid(&t, a);
        let b = tid(&t, b);
        t.rounds.push(Round {
            number: 1,
            explanation: RoundExplanation::empty(1),
            boards: vec![Board {
                record: GameRecord::LongCarried,
                ..Board::pending(a, b, 0, PairingSource::Swiss)
            }],
            sitouts: Vec::new(),
            completed: true,
        });
        // Round 2 holds the live record: the game is finished here, so this is
        // the column its result belongs to.
        t.rounds.push(Round {
            number: 2,
            explanation: RoundExplanation::empty(2),
            boards: vec![Board {
                record: GameRecord::LongEnd(Outcome::won(Winner::Player1)),
                ..Board::pending(a, b, 0, PairingSource::Carried)
            }],
            sitouts: Vec::new(),
            completed: true,
        });
        let rows = data_rows(&to_grid(&t, &t.standings()).unwrap());
        assert!(rows[0].contains("[0- 2+]"), "A row: {rows:?}");
        assert!(rows[1].contains("[0- 1-]"), "B row: {rows:?}");
    }

    /// The same shape as above but with the long game *not* entered. The result
    /// belongs in round 2's column and there is none, and a `Pending` outcome is
    /// neither a draw nor a win — so the renderer would have written `-` for both
    /// sides, submitting a double loss for a game nobody played. There is no
    /// honest document to emit, so it refuses.
    #[test]
    fn an_unresolved_long_game_refuses_to_export_rather_than_inventing_a_result() {
        let mut t = Tournament::new("Long").unwrap();
        let a = t.add_player(named("A")).unwrap().id;
        let b = t.add_player(named("B")).unwrap().id;
        t.finalize_registration().unwrap();
        let a = tid(&t, a);
        let b = tid(&t, b);
        t.rounds.push(Round {
            number: 1,
            explanation: RoundExplanation::empty(1),
            boards: vec![Board {
                record: GameRecord::LongCarried,
                ..Board::pending(a, b, 0, PairingSource::Swiss)
            }],
            sitouts: Vec::new(),
            completed: true,
        });
        t.rounds.push(Round {
            number: 2,
            explanation: RoundExplanation::empty(2),
            boards: vec![Board {
                record: GameRecord::LongEnd(Outcome::PENDING),
                ..Board::pending(a, b, 0, PairingSource::Carried)
            }],
            sitouts: Vec::new(),
            completed: false,
        });

        assert_eq!(
            to_grid(&t, &t.standings()),
            Err(TournamentError::UnresolvedLongGame { round: 2 })
        );

        // Entering the result on the live record unblocks it, and round 2 — the
        // column the game was finished in — carries it.
        t.rounds[1].boards[0].set_outcome(Outcome::won(Winner::Player1));
        t.rounds[1].completed = true;
        let rows = data_rows(&to_grid(&t, &t.standings()).unwrap());
        assert!(rows[0].contains("[0- 2+]"), "A row: {rows:?}");
    }

    /// The grid is the tournament's final record, so a round that is still being
    /// played refuses the whole document.
    ///
    /// It used to be *filtered out* instead: an unfinished round simply did not
    /// appear, so the export succeeded and produced a document missing a round,
    /// with nothing to say so. That is the quiet failure this guards — quieter
    /// than the unfinished-long-game case it sits beside, because there the
    /// starting round still rendered `0-` for both players while the round the
    /// result belonged to was the one dropped.
    #[test]
    fn an_unfinished_round_refuses_the_whole_export() {
        let (mut t, _, _) = one_board(|_| {});
        assert!(to_grid(&t, &t.standings()).is_ok(), "the baseline exports");

        // A second round, paired but not played.
        let (a, b) = (t.rounds[0].boards[0].player1, t.rounds[0].boards[0].player2);
        t.rounds.push(Round {
            number: 2,
            explanation: RoundExplanation::empty(2),
            boards: vec![Board::pending(a, b, 0, PairingSource::Swiss)],
            sitouts: Vec::new(),
            completed: false,
        });
        assert_eq!(
            to_grid(&t, &t.standings()),
            Err(TournamentError::UnfinishedRound { round: 2 }),
            "an unfinished round must stop the export, not vanish from it"
        );
        // The blocker and the export agree, since one is defined in terms of the
        // other — the button says exactly what the export would do.
        assert_eq!(
            t.grid_export_blocker(),
            Some(TournamentError::UnfinishedRound { round: 2 })
        );

        // Recording the result unblocks it, and round 2 is in the document.
        t.rounds[1].boards[0].set_outcome(Outcome::won(Winner::Player1));
        t.rounds[1].completed = true;
        let grid = to_grid(&t, &t.standings()).unwrap();
        assert!(
            data_rows(&grid)[0].contains("[2+ 2+]"),
            "both rounds present: {grid}"
        );
    }

    /// A long game gets the message that names it, rather than the general one
    /// about the round it happens to sit in.
    #[test]
    fn a_long_game_is_diagnosed_before_the_round_it_holds_up() {
        let (mut t, _, _) = one_board(|b| b.record = GameRecord::LongCarried);
        let (a, b) = (t.rounds[0].boards[0].player1, t.rounds[0].boards[0].player2);
        t.rounds.push(Round {
            number: 2,
            explanation: RoundExplanation::empty(2),
            boards: vec![Board {
                record: GameRecord::LongEnd(Outcome::PENDING),
                ..Board::pending(a, b, 0, PairingSource::Carried)
            }],
            sitouts: Vec::new(),
            completed: false,
        });
        // Round 2 is unfinished *and* holds an unresolved long game. Both rules
        // apply; the specific one wins, because "finish the long game" is what
        // the referee can act on.
        assert_eq!(
            to_grid(&t, &t.standings()),
            Err(TournamentError::UnresolvedLongGame { round: 2 })
        );
    }

    #[test]
    fn bye_and_absence_tokens() {
        let mut t = Tournament::new("Byes").unwrap();
        let a = t.add_player(named("A")).unwrap().id;
        let b = t.add_player(named("B")).unwrap().id;
        let c = t.add_player(named("C")).unwrap().id;
        t.finalize_registration().unwrap();
        let a = tid(&t, a);
        let b = tid(&t, b);
        let c = tid(&t, c);
        // A vs B played; C sits out as the bye; nobody absent.
        t.rounds.push(Round {
            number: 1,
            explanation: RoundExplanation::empty(1),
            boards: vec![Board {
                record: GameRecord::Short(Outcome::won(Winner::Player1)),
                ..Board::pending(a, b, 0, PairingSource::Swiss)
            }],
            sitouts: vec![Sitout {
                player: c,
                kind: SitoutKind::Bye,
                value: SitoutValue::Full,
            }],
            completed: true,
        });
        let grid = to_grid(&t, &t.standings()).unwrap();
        assert!(grid.contains("[0+]"), "bye token missing: {grid}");

        // A second round where C is absent (no board, not the bye).
        t.rounds[0].completed = true;
        t.rounds.push(Round {
            number: 2,
            explanation: RoundExplanation::empty(2),
            boards: vec![Board {
                record: GameRecord::Short(Outcome::won(Winner::Player2)),
                ..Board::pending(a, b, 0, PairingSource::Swiss)
            }],
            sitouts: vec![Sitout {
                player: c,
                kind: SitoutKind::Absent,
                value: SitoutValue::Zero,
            }],
            completed: true,
        });
        let grid = to_grid(&t, &t.standings()).unwrap();
        // C's row now ends its round block with an absence in round 2.
        assert!(grid.contains("0-]"), "absence token missing: {grid}");
    }

    #[test]
    fn no_show_tokens() {
        // P1 (rated 2000) beats P2 in a first round so the ranking is settled,
        // then in round 2 P2 doesn't show up against P1: P1's cell is `0+`, P2's
        // `0#`.
        let (mut t, p1, p2) = one_board(|_| {});
        t.rounds.push(Round {
            number: 2,
            explanation: RoundExplanation::empty(2),
            boards: vec![Board {
                record: GameRecord::Short(Outcome::Forfeit {
                    absent: Forfeit::Player2(AbsenceKind::NoShow),
                }), // P2 absent
                ..Board::pending(p1, p2, 0, PairingSource::Swiss)
            }],
            sitouts: Vec::new(),
            completed: true,
        });
        let grid = to_grid(&t, &t.standings()).unwrap();
        // P1 (rank 1) shows the free point `0+`; P2 (rank 2) the no-show `0#`.
        assert!(
            grid.contains("0+]"),
            "no-show opponent token missing: {grid}"
        );
        assert!(grid.contains("0#]"), "no-show token missing: {grid}");
    }

    /// The two absence kinds are what the grid distinguishes: `0#` says the
    /// player simply didn't turn up, `0-` that they were absent for a reason.
    /// The opponent's cell is the same free point either way.
    #[test]
    fn a_justified_absence_reads_as_a_dash_not_a_hash() {
        let (mut t, p1, p2) = one_board(|_| {});
        t.rounds.push(Round {
            number: 2,
            explanation: RoundExplanation::empty(2),
            boards: vec![Board {
                record: GameRecord::Short(Outcome::Forfeit {
                    absent: Forfeit::Player2(AbsenceKind::Justified),
                }),
                ..Board::pending(p1, p2, 0, PairingSource::Swiss)
            }],
            sitouts: Vec::new(),
            completed: true,
        });
        let grid = to_grid(&t, &t.standings()).unwrap();
        assert!(
            grid.contains("0-]"),
            "justified absence token missing: {grid}"
        );
        assert!(
            !grid.contains("0#"),
            "an unjustified token appeared: {grid}"
        );
        assert!(
            grid.contains("0+]"),
            "the opponent still takes the point: {grid}"
        );
    }

    #[test]
    fn double_no_show_marks_both_absent() {
        // Neither player shows up: both cells read `0#`, nobody gets `0+`.
        let (mut t, p1, p2) = one_board(|_| {});
        t.rounds.push(Round {
            number: 2,
            explanation: RoundExplanation::empty(2),
            boards: vec![Board {
                record: GameRecord::Short(Outcome::Forfeit {
                    absent: Forfeit::Both(AbsenceKind::NoShow, AbsenceKind::NoShow),
                }),
                ..Board::pending(p1, p2, 0, PairingSource::Swiss)
            }],
            sitouts: Vec::new(),
            completed: true,
        });
        let grid = to_grid(&t, &t.standings()).unwrap();
        assert_eq!(
            grid.matches("0#").count(),
            2,
            "both cells are no-shows: {grid}"
        );
        assert!(
            !grid.contains("0+"),
            "no free point on a double no-show: {grid}"
        );
    }

    #[test]
    fn cup_bye_reads_as_a_free_point() {
        // A cup bye (an unopposed advance, no board) shows `0+`, like any bye.
        let (mut t, p1, _p2) = one_board(|_| {});
        t.rounds.push(Round {
            number: 2,
            explanation: RoundExplanation::empty(2),
            boards: Vec::new(),
            sitouts: vec![Sitout {
                player: p1,
                kind: SitoutKind::CupBye {
                    stage: CupStage::Semifinal,
                },
                value: SitoutValue::Full,
            }],
            completed: true,
        });
        let grid = to_grid(&t, &t.standings()).unwrap();
        assert!(grid.contains("0+"), "cup bye token missing: {grid}");
    }

    #[test]
    fn a_sitout_renders_whatever_the_referee_scored_it() {
        // The cell follows the sit-out's value, not the reason: a bye the referee
        // knocked down to half a point reads `0=`, not `0+`.
        let (mut t, _p1, p2) = one_board(|_| {});
        t.rounds.push(Round {
            number: 2,
            explanation: RoundExplanation::empty(2),
            boards: Vec::new(),
            sitouts: vec![Sitout {
                player: p2,
                kind: SitoutKind::Bye,
                value: SitoutValue::Half,
            }],
            completed: true,
        });
        let grid = to_grid(&t, &t.standings()).unwrap();
        assert!(
            grid.contains("0=]"),
            "half-scored bye token missing: {grid}"
        );
    }

    #[test]
    fn new_player_flag_and_blank_fields() {
        let mut t = Tournament::new("Mixed").unwrap();
        t.add_player(named("Nobody")).unwrap(); // unrated, no nationality
        t.finalize_registration().unwrap();
        let grid = to_grid(&t, &t.standings()).unwrap();
        // Unrated → `N` flag, and no rounds yet so no bracket block.
        let row = data_rows(&grid).into_iter().next().unwrap();
        assert!(row.contains(" N "), "new-player flag: {row}");
        assert!(row.contains("[Nobody] []"));
    }

    fn named(last: &str) -> crate::player::NewPlayer {
        crate::player::NewPlayer {
            last_name: last.into(),
            ..Default::default()
        }
    }

    #[test]
    fn half_point_totals_render_whole_or_n_over_2() {
        assert_eq!(format_half_points(HalfPoints::from_halves(0)), "0");
        assert_eq!(format_half_points(HalfPoints::from_halves(6)), "3"); // 6 halves = 3 points
        assert_eq!(format_half_points(HalfPoints::from_halves(1)), "1/2"); // ½
        assert_eq!(format_half_points(HalfPoints::from_halves(5)), "5/2"); // 2½
    }
}
