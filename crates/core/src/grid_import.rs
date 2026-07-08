//! American Grid (cross-table) import.
//!
//! Parses the text format produced by [`crate::american_grid`] — or a federation
//! export such as `ShogiAmericanGrid/AmericanGrid.txt` — and rebuilds it into a
//! live [`Tournament`], driving the ordinary round machinery: add the players,
//! finalize registration, then for every round force *all* the pairings (so the
//! pairing engine has nothing left to solve) and record each game's result.
//!
//! This exists to spin a tournament up to a non-trivial state quickly, for tests
//! and simulations; there is no UI for it.
//!
//! # Format recap
//!
//! Zero or more `[metadata]` lines (the first is taken as the tournament name), a
//! `Nr … Pts` column header, then one row per player ordered by final rank:
//!
//! ```text
//! 1  N [Last] [First] FR 1775 [30+ 15+(-r) 0- 0+] 7
//! ```
//!
//! A data row has exactly three bracketed groups — `[last]`, `[first]`, and the
//! round block — which is what lets names contain spaces. Each round cell is a
//! bye (`0+`), an absence (`0-` / `0#`), or a game `<opp><+/-/=>` with an optional
//! `(<±><code>)` handicap. Opponents are referenced by the `Nr` (final rank)
//! column. A `=` records only that a draw occurred (the win/loss of the replay is
//! not encoded), so an imported draw is given an arbitrary decisive winner.

use std::collections::HashMap;

use crate::player::NewPlayer;
use crate::round::{Board, Handicap, PairingSource, Winner};
use crate::tournament::{Tournament, TournamentError};

/// Why an American Grid could not be imported.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GridImportError {
    /// The text contained no parseable player rows.
    #[error("no player rows found in the grid")]
    NoPlayers,
    /// A data row could not be parsed (wrong bracket count, missing number, …).
    #[error("line {line}: could not parse player row ({reason})")]
    BadRow { line: usize, reason: String },
    /// A round cell did not match any known shape.
    #[error("line {line}: malformed round cell {cell:?}")]
    BadCell { line: usize, cell: String },
    /// Two rows share the same `Nr`.
    #[error("duplicate player number {0}")]
    DuplicateNumber(u32),
    /// Rows disagree on how many round columns there are.
    #[error("rows disagree on the number of rounds ({expected} vs {found})")]
    InconsistentRounds { expected: usize, found: usize },
    /// A cell references a player number that has no row.
    #[error("round {round}: player {by} references unknown opponent {opponent}")]
    UnknownOpponent { round: u32, by: u32, opponent: u32 },
    /// A game recorded by one player is not mirrored by the opponent.
    #[error("round {round}: {a} recorded a game vs {b}, but {b} did not reciprocate")]
    Asymmetric { round: u32, a: u32, b: u32 },
    /// Both sides of a game recorded the same win/loss (or disagreed on a draw).
    #[error("round {round}: {a} and {b} recorded inconsistent results")]
    InconsistentResult { round: u32, a: u32, b: u32 },
    /// More than one player took the bye in a single round.
    #[error("round {round}: more than one player took the bye")]
    MultipleByes { round: u32 },
    /// The tournament machinery rejected the reconstructed state.
    #[error(transparent)]
    Tournament(#[from] TournamentError),
}

/// Parse an American Grid document and rebuild it into a finished [`Tournament`]
/// with every round completed.
pub fn import_american_grid(text: &str) -> Result<Tournament, GridImportError> {
    let (name, rows) = parse(text)?;
    if rows.is_empty() {
        return Err(GridImportError::NoPlayers);
    }

    // Every row must describe the same number of rounds.
    let round_count = rows[0].cells.len();
    for row in &rows {
        if row.cells.len() != round_count {
            return Err(GridImportError::InconsistentRounds {
                expected: round_count,
                found: row.cells.len(),
            });
        }
    }

    // Register the players (in Nr order) and remember Nr -> id.
    let mut tournament = Tournament::new(name.as_deref().unwrap_or("Imported tournament"))?;
    let mut id_of: HashMap<u32, uuid::Uuid> = HashMap::new();
    let mut ordered = rows.clone();
    ordered.sort_by_key(|r| r.number);
    for row in &ordered {
        if id_of.contains_key(&row.number) {
            return Err(GridImportError::DuplicateNumber(row.number));
        }
        let id = tournament
            .add_player(NewPlayer {
                last_name: row.last_name.clone(),
                first_name: Some(row.first_name.clone()),
                rating: row.rating,
                fesa_games: None,
                nationality: row.nationality.clone(),
                club: None,
            })?
            .id;
        id_of.insert(row.number, id);
    }
    tournament.finalize_registration()?;

    let by_number: HashMap<u32, &RawRow> = rows.iter().map(|r| (r.number, r)).collect();

    for r in 0..round_count {
        rebuild_round(&mut tournament, r, &rows, &by_number, &id_of)?;
    }
    Ok(tournament)
}

/// Force one round's pairings from the parsed cells, then record every result.
fn rebuild_round(
    tournament: &mut Tournament,
    r: usize,
    rows: &[RawRow],
    by_number: &HashMap<u32, &RawRow>,
    id_of: &HashMap<u32, uuid::Uuid>,
) -> Result<(), GridImportError> {
    let round_number = r as u32 + 1;
    let mut absent: Vec<uuid::Uuid> = Vec::new();
    let mut byes: Vec<uuid::Uuid> = Vec::new();
    let mut forced_boards: Vec<Board> = Vec::new();
    // (a, b, outcome-from-a's-view, handicap) for each game, collected once.
    let mut results: Vec<(u32, u32, Outcome, Option<Handicap>)> = Vec::new();

    for row in rows {
        match &row.cells[r] {
            Cell::Bye => byes.push(id_of[&row.number]),
            Cell::Absent => absent.push(id_of[&row.number]),
            Cell::Game {
                opponent,
                outcome,
                handicap,
            } => {
                let opp = *by_number
                    .get(opponent)
                    .ok_or(GridImportError::UnknownOpponent {
                        round: round_number,
                        by: row.number,
                        opponent: *opponent,
                    })?;
                // Handle each pair once, from the lower-numbered side, checking
                // the opponent mirrors it consistently.
                if row.number < *opponent {
                    let Cell::Game {
                        opponent: back,
                        outcome: their,
                        ..
                    } = &opp.cells[r]
                    else {
                        return Err(GridImportError::Asymmetric {
                            round: round_number,
                            a: row.number,
                            b: *opponent,
                        });
                    };
                    if *back != row.number {
                        return Err(GridImportError::Asymmetric {
                            round: round_number,
                            a: row.number,
                            b: *opponent,
                        });
                    }
                    if !outcome.is_consistent_with(their) {
                        return Err(GridImportError::InconsistentResult {
                            round: round_number,
                            a: row.number,
                            b: *opponent,
                        });
                    }
                    forced_boards.push(Board::pending(
                        id_of[&row.number],
                        id_of[opponent],
                        None,
                        PairingSource::Forced,
                    ));
                    results.push((row.number, *opponent, *outcome, *handicap));
                }
            }
        }
    }

    if byes.len() > 1 {
        return Err(GridImportError::MultipleByes {
            round: round_number,
        });
    }
    let forced_bye = byes.into_iter().next();

    // Force every pairing, so the engine has an empty graph to solve.
    tournament.prepare_round()?;
    tournament.update_draft(absent, forced_boards, forced_bye)?;
    tournament.confirm_round()?;

    for (a_num, b_num, outcome, handicap) in results {
        let a = id_of[&a_num];
        let b = id_of[&b_num];
        // Locate the freshly created board (orientation preserved from forcing).
        let round = tournament.rounds.last().expect("round just confirmed");
        let idx = round
            .boards
            .iter()
            .position(|bd| {
                (bd.player1 == a && bd.player2 == b) || (bd.player1 == b && bd.player2 == a)
            })
            .expect("forced board present");
        let a_is_player1 = round.boards[idx].player1 == a;

        if let Some(h) = handicap {
            tournament.set_board_handicap(round_number, idx, Some(h))?;
        }
        let winner = outcome.winner(a_is_player1);
        tournament.toggle_board_winner(round_number, idx, winner)?;
        if outcome == Outcome::Draw {
            tournament.set_board_drawn(round_number, idx, true)?;
        }
    }

    tournament.complete_current_round()?;
    Ok(())
}

// --- Parsing --------------------------------------------------------------

/// A parsed player row: its number and fields, plus one cell per round.
#[derive(Debug, Clone)]
struct RawRow {
    number: u32,
    last_name: String,
    first_name: String,
    nationality: Option<String>,
    rating: Option<u32>,
    cells: Vec<Cell>,
}

/// A parsed round cell.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Cell {
    Bye,
    Absent,
    Game {
        opponent: u32,
        outcome: Outcome,
        handicap: Option<Handicap>,
    },
}

/// The result of a game from one player's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Win,
    Loss,
    Draw,
}

impl Outcome {
    /// Whether the opponent's recorded outcome is the complement of this one.
    fn is_consistent_with(self, other: &Outcome) -> bool {
        matches!(
            (self, other),
            (Outcome::Win, Outcome::Loss)
                | (Outcome::Loss, Outcome::Win)
                | (Outcome::Draw, Outcome::Draw)
        )
    }

    /// Which side won, given whether the perspective player is player 1. A draw
    /// has no encoded winner, so it is arbitrarily assigned to player 1 (the draw
    /// flag is set separately).
    fn winner(self, is_player1: bool) -> Winner {
        let perspective_won = match self {
            Outcome::Win => true,
            Outcome::Loss => false,
            Outcome::Draw => true, // arbitrary decisive winner for a lost-info draw
        };
        match (perspective_won, is_player1) {
            (true, true) | (false, false) => Winner::Player1,
            (true, false) | (false, true) => Winner::Player2,
        }
    }
}

/// Parse the whole document into an optional tournament name and the player rows.
fn parse(text: &str) -> Result<(Option<String>, Vec<RawRow>), GridImportError> {
    let mut name: Option<String> = None;
    let mut rows: Vec<RawRow> = Vec::new();

    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        let groups = bracket_groups(line);
        match groups.len() {
            // A metadata line (`[…]`); the first one names the tournament.
            1 if name.is_none() && line.trim_start().starts_with('[') => {
                let (open, close) = groups[0];
                name = Some(line[open + 1..close].trim().to_string());
            }
            // A data row has exactly the three groups: [last] [first] [rounds].
            3 => rows.push(parse_row(line, &groups, i + 1)?),
            // Anything else (the column header, extra metadata) is skipped.
            _ => {}
        }
    }
    Ok((name, rows))
}

/// Byte offsets `(open, close)` of every top-level `[...]` group on a line. The
/// grid never nests brackets, so a simple scan suffices.
fn bracket_groups(line: &str) -> Vec<(usize, usize)> {
    let mut groups = Vec::new();
    let mut open: Option<usize> = None;
    for (i, c) in line.char_indices() {
        match c {
            '[' if open.is_none() => open = Some(i),
            ']' => {
                if let Some(o) = open.take() {
                    groups.push((o, i));
                }
            }
            _ => {}
        }
    }
    groups
}

/// Parse one data row given its three bracket groups.
fn parse_row(
    line: &str,
    groups: &[(usize, usize)],
    line_no: usize,
) -> Result<RawRow, GridImportError> {
    let bad = |reason: &str| GridImportError::BadRow {
        line: line_no,
        reason: reason.to_string(),
    };

    let (l_open, l_close) = groups[0];
    let (f_open, f_close) = groups[1];
    let (r_open, r_close) = groups[2];

    let before = &line[..l_open];
    let last_name = line[l_open + 1..l_close].trim().to_string();
    let first_name = line[f_open + 1..f_close].trim().to_string();
    let between = &line[f_close + 1..r_open];
    let round_block = &line[r_open + 1..r_close];

    // The leading number is the player's Nr; an optional `N` flag is ignored
    // (a blank Elo already marks an unrated player).
    let number: u32 = before
        .split_whitespace()
        .next()
        .and_then(|t| t.parse().ok())
        .ok_or_else(|| bad("missing player number"))?;

    // Between the name and the round block sit the nationality and/or rating, in
    // either presence: a bare number is the rating, anything else the country.
    let mut nationality = None;
    let mut rating = None;
    for token in between.split_whitespace() {
        if let Ok(elo) = token.parse::<u32>() {
            rating = Some(elo);
        } else {
            nationality = Some(token.to_string());
        }
    }

    let cells = round_block
        .split_whitespace()
        .map(|c| parse_cell(c, line_no))
        .collect::<Result<Vec<_>, _>>()?;
    if cells.is_empty() {
        return Err(bad("no round cells"));
    }

    Ok(RawRow {
        number,
        last_name,
        first_name,
        nationality,
        rating,
        cells,
    })
}

/// Parse a single round cell (`0+`, `0-`, `0#`, or `<opp><+/-/=>` plus an
/// optional `(<±><code>)` handicap). A trailing floater mark is tolerated.
fn parse_cell(cell: &str, line_no: usize) -> Result<Cell, GridImportError> {
    let bad = || GridImportError::BadCell {
        line: line_no,
        cell: cell.to_string(),
    };
    // Floaters are dropped from the grid, but tolerate a stray trailing ^/v.
    let cell = cell.trim_end_matches(['^', 'v']);

    let digits_end = cell.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
    let (digits, rest) = cell.split_at(digits_end);
    let opponent: u32 = digits.parse().map_err(|_| bad())?;

    let mut chars = rest.chars();
    let sign = chars.next().ok_or_else(bad)?;
    let tail = chars.as_str();

    if opponent == 0 {
        // A non-game token carries no suffix.
        if !tail.is_empty() {
            return Err(bad());
        }
        return match sign {
            '+' => Ok(Cell::Bye),
            '-' | '#' => Ok(Cell::Absent),
            _ => Err(bad()),
        };
    }

    let outcome = match sign {
        '+' => Outcome::Win,
        '-' => Outcome::Loss,
        '=' => Outcome::Draw,
        _ => return Err(bad()),
    };
    let handicap = if tail.is_empty() {
        None
    } else {
        Some(parse_handicap(tail, line_no, cell)?)
    };
    Ok(Cell::Game {
        opponent,
        outcome,
        handicap,
    })
}

/// Parse a `(<±><code>)` handicap suffix. The sign (who conceded the odds) is not
/// needed — the giver is re-derived from ratings when the board is rebuilt — so
/// only the code is read.
fn parse_handicap(suffix: &str, line_no: usize, cell: &str) -> Result<Handicap, GridImportError> {
    let bad = || GridImportError::BadCell {
        line: line_no,
        cell: cell.to_string(),
    };
    let inner = suffix
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(bad)?;
    let code = inner.strip_prefix(['+', '-']).ok_or_else(bad)?;
    Handicap::from_code(code).ok_or_else(bad)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::Winner;

    /// Find a player by last name.
    fn player<'a>(t: &'a Tournament, last: &str) -> &'a crate::player::Player {
        t.players
            .iter()
            .find(|p| p.last_name == last)
            .expect("player exists")
    }

    /// The board in a round that pairs the two named players.
    fn board<'a>(t: &'a Tournament, round: usize, a: &str, b: &str) -> &'a Board {
        let (ai, bi) = (player(t, a).id, player(t, b).id);
        t.rounds[round]
            .boards
            .iter()
            .find(|bd| {
                (bd.player1 == ai && bd.player2 == bi) || (bd.player1 == bi && bd.player2 == ai)
            })
            .expect("board exists")
    }

    #[test]
    fn imports_players_rounds_byes_and_absences() {
        // 3 players, 2 rounds. R1: Alpha>Beta, Gamma bye. R2: Alpha>Gamma, Beta
        // bye. Gamma is rated but has no nationality (blank between-segment token).
        let grid = "\
[Mini Open]
Nr Name          Nat Elo  1   2   Pts
1  [Alpha] [Ann] FR  2000 [2+ 3+] 2
2  [Beta] [Bo]   JP  1500 [1- 0+] 1
3  [Gamma] []        1000 [0+ 1-] 1
";
        let t = import_american_grid(grid).unwrap();
        assert_eq!(t.name, "Mini Open");
        assert_eq!(t.players.len(), 3);
        assert_eq!(t.rounds.len(), 2);
        assert!(t.rounds.iter().all(|r| r.completed));

        // Fields parsed correctly, including a blank nationality with a rating.
        assert_eq!(player(&t, "Gamma").rating, Some(1000));
        assert_eq!(player(&t, "Gamma").nationality, None);
        assert_eq!(player(&t, "Alpha").first_name, "Ann");
        assert_eq!(player(&t, "Beta").nationality.as_deref(), Some("JP"));

        // Byes landed on the right players.
        assert_eq!(t.rounds[0].bye, Some(player(&t, "Gamma").id));
        assert_eq!(t.rounds[1].bye, Some(player(&t, "Beta").id));

        // Alpha beat Beta in round 1.
        let b = board(&t, 0, "Alpha", "Beta");
        let alpha_is_p1 = b.player1 == player(&t, "Alpha").id;
        assert_eq!(
            b.effective_winner(),
            Some(if alpha_is_p1 {
                Winner::Player1
            } else {
                Winner::Player2
            })
        );

        // Standings: Alpha clear first with 2 points.
        let standings = t.standings();
        assert_eq!(standings[0].player_id, player(&t, "Alpha").id);
        assert_eq!(standings[0].points, 2);
    }

    #[test]
    fn imports_multi_word_names_and_new_players() {
        // A two-word surname and an unrated ("N" flag) player with no rating.
        let grid = "\
[Names]
Nr   Name                      Nat Elo  1   Pts
1  N [Le Roux] [Jean Pierre]   FR       [2+] 1
2    [Posada Zuluaga] [Victor] CO  1600 [1-] 0
";
        let t = import_american_grid(grid).unwrap();
        assert_eq!(player(&t, "Le Roux").first_name, "Jean Pierre");
        assert_eq!(player(&t, "Le Roux").rating, None); // unrated
        assert_eq!(player(&t, "Posada Zuluaga").first_name, "Victor");
        assert_eq!(player(&t, "Posada Zuluaga").rating, Some(1600));
    }

    #[test]
    fn imports_handicap_giver_from_ratings() {
        // High (2000) gives a rook to Low (1000) and wins the game outright.
        let grid = "\
[HC]
Nr Name       Nat Elo  1        Pts
1  [High] [H] FR  2000 [2+(-r)] 1
2  [Low] [L]  FR  1000 [1-(+r)] 0
";
        let t = import_american_grid(grid).unwrap();
        let b = board(&t, 0, "High", "Low");
        let game = b.handicap.expect("handicap set");
        assert_eq!(game.handicap, Handicap::Rook);
        // The giver is the higher-rated player, High.
        let high_side = if b.player1 == player(&t, "High").id {
            Winner::Player1
        } else {
            Winner::Player2
        };
        assert_eq!(game.giver, high_side);
        assert_eq!(b.effective_winner(), Some(high_side));
    }

    #[test]
    fn imports_a_draw_as_drawn_with_a_decisive_winner() {
        let grid = "\
[Draw]
Nr Name    Nat Elo  1   Pts
1  [X] [x] FR  1500 [2=] 1
2  [Y] [y] FR  1400 [1=] 0
";
        let t = import_american_grid(grid).unwrap();
        let b = board(&t, 0, "X", "Y");
        assert!(b.drawn);
        assert!(b.result.is_some()); // a decisive winner is still recorded
    }

    #[test]
    fn rejects_inconsistent_results() {
        // Both players claim a win over each other.
        let grid = "\
[Bad]
Nr Name    Nat Elo  1   Pts
1  [A] [a] FR  2000 [2+] 1
2  [B] [b] FR  1000 [1+] 1
";
        assert!(matches!(
            import_american_grid(grid),
            Err(GridImportError::InconsistentResult { round: 1, .. })
        ));
    }

    #[test]
    fn rejects_unknown_opponent() {
        let grid = "\
[Bad]
Nr Name    Nat Elo  1   Pts
1  [A] [a] FR  2000 [9+] 1
";
        assert!(matches!(
            import_american_grid(grid),
            Err(GridImportError::UnknownOpponent { opponent: 9, .. })
        ));
    }

    #[test]
    fn round_trips_through_export_to_a_fixpoint() {
        // Distinct ratings (so tie-breaks are fully determined) with a bye and a
        // handicap. One export/import pass normalizes it; further passes are
        // stable, so grid1 == grid2.
        let base = "\
[Cup]
Nr Name        Nat Elo  1        2   Pts
1  [One] [a]   FR  2000 [3+(-l)  2+] 2
2  [Two] [b]   FR  1500 [0+      1-] 1
3  [Three] [c] FR  1000 [1-      0+] 1
";
        let t1 = import_american_grid(base).unwrap();
        let g1 = crate::american_grid(&t1, &t1.standings());
        let t2 = import_american_grid(&g1).unwrap();
        let g2 = crate::american_grid(&t2, &t2.standings());
        assert_eq!(
            g1, g2,
            "export is not a fixpoint:\n---g1---\n{g1}\n---g2---\n{g2}"
        );
    }

    #[test]
    fn imports_an_absent_player() {
        // C sits out round 1 (absent, not a bye); A and B still play each other.
        let grid = "\
[Abs]
Nr Name    Nat Elo  1   Pts
1  [A] [a] FR  2000 [2+] 1
2  [B] [b] FR  1000 [1-] 0
3  [C] [c] FR  1500 [0-] 0
";
        let t = import_american_grid(grid).unwrap();
        assert_eq!(t.rounds[0].absent, vec![player(&t, "C").id]);
        assert_eq!(t.rounds[0].bye, None);
    }

    #[test]
    fn imports_a_loss_recorded_by_the_lower_numbered_player() {
        // The processing side (lower Nr) is the one who lost, not the winner —
        // exercises the `Outcome::Loss` arm of `Outcome::winner`.
        let grid = "\
[Loss]
Nr Name    Nat Elo  1   Pts
1  [A] [a] FR  2000 [2-] 0
2  [B] [b] FR  1000 [1+] 1
";
        let t = import_american_grid(grid).unwrap();
        let b = board(&t, 0, "A", "B");
        assert_eq!(b.effective_winner(), Some(Winner::Player2));
    }

    #[test]
    fn rejects_a_grid_with_no_player_rows() {
        let grid = "\
[Empty]
Nr Name Nat Elo 1 Pts
";
        assert_eq!(import_american_grid(grid), Err(GridImportError::NoPlayers));
    }

    #[test]
    fn rejects_rows_with_inconsistent_round_counts() {
        let grid = "\
[Bad]
Nr Name    Nat Elo  1   2   Pts
1  [A] [a] FR  2000 [2+ 0+] 2
2  [B] [b] FR  1000 [1-] 0
";
        assert!(matches!(
            import_american_grid(grid),
            Err(GridImportError::InconsistentRounds {
                expected: 2,
                found: 1
            })
        ));
    }

    #[test]
    fn rejects_duplicate_player_numbers() {
        let grid = "\
[Bad]
Nr Name    Nat Elo  1   Pts
1  [A] [a] FR  2000 [2+] 1
1  [B] [b] FR  1000 [1-] 0
";
        assert_eq!(
            import_american_grid(grid),
            Err(GridImportError::DuplicateNumber(1))
        );
    }

    #[test]
    fn rejects_multiple_byes_in_one_round() {
        let grid = "\
[Bad]
Nr Name    Nat Elo  1   Pts
1  [A] [a] FR  2000 [0+] 0
2  [B] [b] FR  1000 [0+] 0
";
        assert!(matches!(
            import_american_grid(grid),
            Err(GridImportError::MultipleByes { round: 1 })
        ));
    }

    #[test]
    fn rejects_bad_row_missing_player_number() {
        let grid = "\
[Bad]
Nr Name    Nat Elo  1   Pts
X  [A] [a] FR  2000 [2+] 1
2  [B] [b] FR  1000 [1-] 0
";
        assert!(matches!(
            import_american_grid(grid),
            Err(GridImportError::BadRow { reason, .. }) if reason == "missing player number"
        ));
    }

    #[test]
    fn rejects_bad_row_with_no_round_cells() {
        let grid = "\
[Bad]
Nr Name    Nat Elo  1  Pts
1  [A] [a] FR  2000 [] 1
";
        assert!(matches!(
            import_american_grid(grid),
            Err(GridImportError::BadRow { reason, .. }) if reason == "no round cells"
        ));
    }

    #[test]
    fn rejects_malformed_round_cells() {
        // No digits at all before the sign.
        assert!(matches!(
            import_american_grid(
                "[Bad]\nNr Name    Nat Elo  1   Pts\n1  [A] [a] FR  2000 [x+] 1\n"
            ),
            Err(GridImportError::BadCell { cell, .. }) if cell == "x+"
        ));
        // A non-game (`0`) cell must not carry a trailing suffix.
        assert!(matches!(
            import_american_grid(
                "[Bad]\nNr Name    Nat Elo  1    Pts\n1  [A] [a] FR  2000 [0+x] 1\n"
            ),
            Err(GridImportError::BadCell { cell, .. }) if cell == "0+x"
        ));
        // A non-game cell's sign must be `+`/`-`/`#`.
        assert!(matches!(
            import_american_grid(
                "[Bad]\nNr Name    Nat Elo  1   Pts\n1  [A] [a] FR  2000 [0*] 1\n"
            ),
            Err(GridImportError::BadCell { cell, .. }) if cell == "0*"
        ));
        // A game cell's sign must be `+`/`-`/`=`.
        assert!(matches!(
            import_american_grid(
                "[Bad]\nNr Name    Nat Elo  1   Pts\n1  [A] [a] FR  2000 [2*] 1\n"
            ),
            Err(GridImportError::BadCell { cell, .. }) if cell == "2*"
        ));
    }

    #[test]
    fn rejects_bad_handicap_suffix() {
        let grid = "\
[Bad]
Nr Name    Nat Elo  1        Pts
1  [A] [a] FR  2000 [2+(-z)] 1
2  [B] [b] FR  1000 [1-]     0
";
        assert!(matches!(
            import_american_grid(grid),
            Err(GridImportError::BadCell { cell, .. }) if cell == "2+(-z)"
        ));
    }

    #[test]
    fn rejects_asymmetric_game_when_opponent_does_not_reciprocate() {
        // A claims a game vs B, but B's cell for the round is a bye, not a game.
        let grid = "\
[Bad]
Nr Name    Nat Elo  1   Pts
1  [A] [a] FR  2000 [2+] 1
2  [B] [b] FR  1000 [0+] 0
";
        assert!(matches!(
            import_american_grid(grid),
            Err(GridImportError::Asymmetric {
                round: 1,
                a: 1,
                b: 2
            })
        ));
    }

    #[test]
    fn rejects_asymmetric_game_when_opponent_references_someone_else() {
        // A claims a game vs B, but B's cell references C instead of A.
        let grid = "\
[Bad]
Nr Name    Nat Elo  1   Pts
1  [A] [a] FR  2000 [2+] 1
2  [B] [b] FR  1000 [3-] 0
3  [C] [c] FR  1500 [0+] 0
";
        assert!(matches!(
            import_american_grid(grid),
            Err(GridImportError::Asymmetric {
                round: 1,
                a: 1,
                b: 2
            })
        ));
    }
}
