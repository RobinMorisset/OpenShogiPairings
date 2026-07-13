//! American Grid (cross-table) export.
//!
//! The text format the European federation ingests for ELO updates. A single
//! `[tournament name]` header line, then one row per player ordered by final
//! rank, each row documenting who the player faced and the result in every
//! completed round:
//!
//! ```text
//! [Paris Open]
//! Nr Name          Nat Elo  1   2    Pts
//! 1  [Goix] [Théo] FR  1775 [2+ 3+]  2
//! ```
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
use crate::round::{Board, Round, Winner};
use crate::standings::Standing;
use crate::tournament::Tournament;

/// Columns are separated by a single space (widths carry the rest of the layout).
const COLUMN_SEP: &str = " ";

/// Render the tournament as an American Grid document.
///
/// `standings` must be the tournament's own [`Tournament::standings`], whose
/// order defines each player's final rank (and thus the opponent numbering).
pub fn to_grid(tournament: &Tournament, standings: &[Standing]) -> String {
    let players_by_id: HashMap<Uuid, &Player> =
        tournament.players.iter().map(|p| (p.id, p)).collect();
    // Final rank = 1-based position in the standings; used for the `Nr` column
    // and to renumber opponents referenced in the round cells.
    let rank_of: HashMap<Uuid, u32> = standings
        .iter()
        .enumerate()
        .map(|(i, s)| (s.player_id, i as u32 + 1))
        .collect();
    let completed: Vec<&Round> = tournament.rounds.iter().filter(|r| r.completed).collect();

    // Header row, then one row per player in final-rank order.
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(standings.len() + 1);
    let mut header: Vec<String> = vec![
        "Nr".into(),
        String::new(), // the "new player" flag column has a blank header
        "Name".into(),
        "Nat".into(),
        "Elo".into(),
    ];
    header.extend(completed.iter().map(|r| r.number.to_string()));
    header.push("Pts".into());
    rows.push(header);

    for standing in standings {
        if let Some(player) = players_by_id.get(&standing.player_id) {
            rows.push(row_for(player, standing, &completed, &rank_of));
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

    let mut out = format!("[{}]\n", tournament.name);
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
    out
}

/// One player's row: rank, new-player flag, name, nationality, rating, the
/// bracketed round block, and total points.
fn row_for(
    player: &Player,
    standing: &Standing,
    rounds: &[&Round],
    rank_of: &HashMap<Uuid, u32>,
) -> Vec<String> {
    let mut round_cells: Vec<String> = rounds
        .iter()
        .map(|r| round_cell(player.id, r, rank_of))
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
            .get(&player.id)
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
    row.push(standing.points.to_string());
    row
}

/// The single round cell for a player: `0+` for a bye (or a no-show opponent),
/// `0-` for an absence, `0#` for a no-show, or `<opponent><marker><handicap?>`
/// for a game. The opponent is their final rank.
fn round_cell(player_id: Uuid, round: &Round, rank_of: &HashMap<Uuid, u32>) -> String {
    // The Swiss bye and the (rare) cup bye both read as a free point.
    if round.bye == Some(player_id) || round.cup_byes.contains(&player_id) {
        return "0+".into();
    }
    let Some(board) = round
        .boards
        .iter()
        .find(|b| b.player1 == player_id || b.player2 == player_id)
    else {
        // No board and not the bye: absent for the whole round.
        return "0-".into();
    };

    let is_player1 = board.player1 == player_id;
    let side = if is_player1 {
        Winner::Player1
    } else {
        Winner::Player2
    };

    // A no-show board renders distinctly: `0#` for a player who was absent (both,
    // under a double no-show), `0+` (a bye-equivalent free point) for the one who
    // showed up.
    if board.no_show.is_some() {
        return if board.no_show_absent(side) {
            "0#".into()
        } else {
            "0+".into()
        };
    }

    let opponent_id = if is_player1 {
        board.player2
    } else {
        board.player1
    };
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
    if board.drawn {
        return "=";
    }
    let won = matches!(
        (board.result, is_player1),
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
    use crate::round::{Handicap, HandicapGame, NoShow, PairingSource};

    /// Build a finalized two-player tournament (P1 rated 2000, P2 1000) plus one
    /// completed round whose single board is configured by `configure`.
    fn one_board(configure: impl FnOnce(&mut Board)) -> (Tournament, Uuid, Uuid) {
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

        let mut board = Board {
            result: Some(Winner::Player1),
            ..Board::pending(p1, p2, None, PairingSource::Swiss)
        };
        configure(&mut board);
        t.rounds.push(Round {
            number: 1,
            boards: vec![board],
            bye: None,
            cup_byes: Vec::new(),
            absent: Vec::new(),
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
        let grid = to_grid(&t, &t.standings());
        let mut lines = grid.lines();
        assert_eq!(lines.next().unwrap(), "[Test Cup]");
        // Column header carries the round number and the totals column.
        let header = lines.next().unwrap();
        assert!(header.starts_with("Nr"));
        assert!(header.contains(" 1 ") || header.ends_with(" 1  Pts") || header.contains('1'));
        assert!(header.trim_end().ends_with("Pts"));
    }

    #[test]
    fn win_and_loss_reference_final_rank() {
        // P1 (rank 1) beat P2 (rank 2). P1's cell reads `2+`, P2's `1-`.
        let (t, ..) = one_board(|_| {});
        let rows = data_rows(&to_grid(&t, &t.standings()));
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
        let (t, ..) = one_board(|b| b.drawn = true);
        let rows = data_rows(&to_grid(&t, &t.standings()));
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
        let grid = to_grid(&t, &t.standings());
        let _ = p1;
        assert!(grid.contains("[2+(-2p)]"), "giver cell: {grid}");
        assert!(grid.contains("[1-(+2p)]"), "receiver cell: {grid}");
    }

    #[test]
    fn bye_and_absence_tokens() {
        let mut t = Tournament::new("Byes").unwrap();
        let a = t.add_player(named("A")).unwrap().id;
        let b = t.add_player(named("B")).unwrap().id;
        let c = t.add_player(named("C")).unwrap().id;
        t.finalize_registration().unwrap();
        // A vs B played; C sits out as the bye; nobody absent.
        t.rounds.push(Round {
            number: 1,
            boards: vec![Board {
                result: Some(Winner::Player1),
                ..Board::pending(a, b, None, PairingSource::Swiss)
            }],
            bye: Some(c),
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        });
        let grid = to_grid(&t, &t.standings());
        assert!(grid.contains("[0+]"), "bye token missing: {grid}");

        // A second round where C is absent (no board, not the bye).
        t.rounds[0].completed = true;
        t.rounds.push(Round {
            number: 2,
            boards: vec![Board {
                result: Some(Winner::Player2),
                ..Board::pending(a, b, None, PairingSource::Swiss)
            }],
            bye: None,
            cup_byes: Vec::new(),
            absent: vec![c],
            completed: true,
        });
        let grid = to_grid(&t, &t.standings());
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
            boards: vec![Board {
                no_show: Some(NoShow::Player2), // P2 absent
                ..Board::pending(p1, p2, None, PairingSource::Swiss)
            }],
            bye: None,
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        });
        let grid = to_grid(&t, &t.standings());
        // P1 (rank 1) shows the free point `0+`; P2 (rank 2) the no-show `0#`.
        assert!(grid.contains("0+]"), "no-show opponent token missing: {grid}");
        assert!(grid.contains("0#]"), "no-show token missing: {grid}");
    }

    #[test]
    fn double_no_show_marks_both_absent() {
        // Neither player shows up: both cells read `0#`, nobody gets `0+`.
        let (mut t, p1, p2) = one_board(|_| {});
        t.rounds.push(Round {
            number: 2,
            boards: vec![Board {
                no_show: Some(NoShow::Both),
                ..Board::pending(p1, p2, None, PairingSource::Swiss)
            }],
            bye: None,
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        });
        let grid = to_grid(&t, &t.standings());
        assert_eq!(grid.matches("0#").count(), 2, "both cells are no-shows: {grid}");
        assert!(!grid.contains("0+"), "no free point on a double no-show: {grid}");
    }

    #[test]
    fn cup_bye_reads_as_a_free_point() {
        // A cup bye (an unopposed advance, no board) shows `0+`, like any bye.
        let (mut t, p1, _p2) = one_board(|_| {});
        t.rounds.push(Round {
            number: 2,
            boards: Vec::new(),
            bye: None,
            cup_byes: vec![p1],
            absent: Vec::new(),
            completed: true,
        });
        let grid = to_grid(&t, &t.standings());
        assert!(grid.contains("0+"), "cup bye token missing: {grid}");
    }

    #[test]
    fn new_player_flag_and_blank_fields() {
        let mut t = Tournament::new("Mixed").unwrap();
        t.add_player(named("Nobody")).unwrap(); // unrated, no nationality
        t.finalize_registration().unwrap();
        let grid = to_grid(&t, &t.standings());
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
}
