//! Rebuilding a finished [`Tournament`] from parsed cross-table rows.
//!
//! The machinery shared by every cross-table importer — today only the FESA
//! result-table parser ([`crate::fesa_results`]), which produces [`RawRow`]s and
//! hands them here. Rebuilding drives the ordinary round machinery: add the
//! players, finalize registration, then for every round force *all* the pairings
//! (so the pairing engine has nothing left to solve) and record each game's
//! result.
//!
//! This exists to spin a tournament up to a non-trivial state quickly, for tests
//! and simulations; there is no UI for it.
//!
//! # Cells
//!
//! Each round cell is a bye (`0+`), an absence (`0-` / `0#`), a half-point
//! bye/absence (`0=`), or a game `<opp><+/-/=>` with an optional `(<±><code>)`
//! handicap. Opponents are referenced by the `Nr` (final rank) column. A `=`
//! records only that a draw occurred (the win/loss of the replay is not
//! encoded), so an imported draw is given an arbitrary decisive winner.
//!
//! A handicap's sign says whether *this* player conceded the odds (`-`) or
//! received them (`+`), so the two rows of a game must mirror each other. The
//! conceding side is taken from the cell rather than re-derived from the
//! ratings: a cross-table records games where the *lower*-rated player gave the
//! odds (often against a pre-unrated opponent), which a rating-based guess would
//! get backwards.
//!
//! A handicap mark on a *non-playing* cell is a published-table quirk (a player
//! handicapped in every game, the mark carried onto their bye row too): it is
//! validated and then dropped, since no board exists to carry it.
//!
//! Each non-playing cell is imported with the score it states — a `0+` as a bye,
//! a `0=` as a half-scoring absence, a `0-` as an absence worth nothing — so the
//! tournament's `half_point_absences` default never enters into it. A round may
//! carry as many `0+` cells as it likes (a genuine bye plus any number of
//! forfeit wins, which a cross-table renders identically); each becomes a bye.
//!
//! # Accepted limitation
//!
//! The importer reconstructs a *bye/absence* history, not the no-show board
//! machinery: a player who showed up on a no-show board reads as `0+` (a free
//! point) and imports as a plain **bye**, and the absentee's `0#` as an
//! **absence**. Both score the same as the original, which is what the importer
//! is for — seeding non-trivial tournament state for tests and simulations —
//! where reconstructing the boards themselves is out of scope.

use std::collections::HashMap;

use crate::player::NewPlayer;
use crate::round::{Board, Handicap, PairingSource, SitoutValue, Winner};
use crate::tournament::{Tournament, TournamentError};
use crate::units::TournamentId;

/// Why a cross-table could not be imported.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResultImportError {
    /// The text contained no parseable player rows.
    #[error("no player rows found")]
    NoPlayers,
    /// A data row could not be parsed (missing number, no ELO column, …).
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
    /// The two sides of a game disagree about the handicap: different odds, the
    /// same conceding sign on both, or only one side annotated at all.
    #[error("round {round}: {a} and {b} recorded inconsistent handicaps")]
    InconsistentHandicap { round: u32, a: u32, b: u32 },
    /// The tournament machinery rejected the reconstructed state.
    #[error(transparent)]
    Tournament(#[from] TournamentError),
}

/// Register `rows` as players (in Nr order) and replay every round from their
/// cells, returning the finished tournament and the `Nr -> id` map.
pub(crate) fn build_tournament(
    name: Option<&str>,
    rows: Vec<RawRow>,
) -> Result<(Tournament, HashMap<u32, TournamentId>), ResultImportError> {
    if rows.is_empty() {
        return Err(ResultImportError::NoPlayers);
    }

    // Every row must describe the same number of rounds.
    let round_count = rows[0].cells.len();
    for row in &rows {
        if row.cells.len() != round_count {
            return Err(ResultImportError::InconsistentRounds {
                expected: round_count,
                found: row.cells.len(),
            });
        }
    }

    // Register the players (in Nr order) and remember Nr -> id.
    let mut tournament = Tournament::new(name.unwrap_or("Imported tournament"))?;
    let mut uuid_of: HashMap<u32, uuid::Uuid> = HashMap::new();
    let mut ordered = rows.clone();
    ordered.sort_by_key(|r| r.number);
    for row in &ordered {
        if uuid_of.contains_key(&row.number) {
            return Err(ResultImportError::DuplicateNumber(row.number));
        }
        let id = tournament
            .add_player(NewPlayer {
                last_name: row.last_name.clone(),
                first_name: Some(row.first_name.clone()),
                rating: row.rating,
                grade: row.grade,
                fesa_games: None,
                nationality: row.nationality.clone(),
                club: None,
            })?
            .id;
        uuid_of.insert(row.number, id);
    }
    tournament.finalize_registration()?;

    // Rounds are tid-native, so map each table "Nr" to the tournament number
    // finalization assigned (by rating, so generally *not* the table's Nr).
    let tid_by_uuid: HashMap<uuid::Uuid, TournamentId> = tournament
        .players
        .iter()
        .filter_map(|p| p.tournament_id.map(|t| (p.id, t)))
        .collect();
    let id_of: HashMap<u32, TournamentId> = uuid_of
        .iter()
        .map(|(&nr, uid)| (nr, tid_by_uuid[uid]))
        .collect();

    let by_number: HashMap<u32, &RawRow> = rows.iter().map(|r| (r.number, r)).collect();

    for r in 0..round_count {
        rebuild_round(&mut tournament, r, &rows, &by_number, &id_of)?;
    }
    Ok((tournament, id_of))
}

/// Force one round's pairings from the parsed cells, then record every result.
fn rebuild_round(
    tournament: &mut Tournament,
    r: usize,
    rows: &[RawRow],
    by_number: &HashMap<u32, &RawRow>,
    id_of: &HashMap<u32, TournamentId>,
) -> Result<(), ResultImportError> {
    let round_number = r as u32 + 1;
    let mut absent: Vec<TournamentId> = Vec::new();
    let mut forced_byes: Vec<TournamentId> = Vec::new();
    let mut forced_boards: Vec<Board> = Vec::new();
    // What each non-playing cell scored, applied once the round exists — the
    // cell states the score outright, so nothing is left to the tournament's
    // `half_point_absences` default.
    let mut sitout_values: Vec<(TournamentId, SitoutValue)> = Vec::new();
    // (a, b, outcome-from-a's-view, handicap-from-a's-view) per game, once.
    let mut results: Vec<(u32, u32, Outcome, Option<CellHandicap>)> = Vec::new();

    for row in rows {
        let id = id_of[&row.number];
        match &row.cells[r] {
            // A `0+` is a free point without a board: import it as a forced bye,
            // however many the round has (a real bye plus any forfeit wins).
            Cell::Bye => {
                forced_byes.push(id);
                sitout_values.push((id, SitoutValue::Full));
            }
            // A half-point bye reconstructs as an absence scoring its ½.
            Cell::HalfBye => {
                absent.push(id);
                sitout_values.push((id, SitoutValue::Half));
            }
            Cell::Absent => {
                absent.push(id);
                sitout_values.push((id, SitoutValue::Zero));
            }
            Cell::Game {
                opponent,
                outcome,
                handicap,
            } => {
                let opp = *by_number
                    .get(opponent)
                    .ok_or(ResultImportError::UnknownOpponent {
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
                        handicap: their_handicap,
                    } = &opp.cells[r]
                    else {
                        return Err(ResultImportError::Asymmetric {
                            round: round_number,
                            a: row.number,
                            b: *opponent,
                        });
                    };
                    if *back != row.number {
                        return Err(ResultImportError::Asymmetric {
                            round: round_number,
                            a: row.number,
                            b: *opponent,
                        });
                    }
                    if !outcome.is_consistent_with(their) {
                        return Err(ResultImportError::InconsistentResult {
                            round: round_number,
                            a: row.number,
                            b: *opponent,
                        });
                    }
                    // Both rows describe the same game, so either both annotate
                    // the handicap — same odds, opposite conceding signs — or
                    // neither does.
                    let mirrored = match (handicap, their_handicap) {
                        (None, None) => true,
                        (Some(mine), Some(theirs)) => {
                            mine.handicap == theirs.handicap && mine.gave != theirs.gave
                        }
                        _ => false,
                    };
                    if !mirrored {
                        return Err(ResultImportError::InconsistentHandicap {
                            round: round_number,
                            a: row.number,
                            b: *opponent,
                        });
                    }
                    forced_boards.push(Board::pending(
                        id,
                        id_of[opponent],
                        0,
                        PairingSource::Forced,
                    ));
                    results.push((row.number, *opponent, *outcome, *handicap));
                }
            }
        }
    }

    // Force every pairing, so the engine has an empty graph to solve.
    tournament.prepare_round()?;
    tournament.update_draft(absent, forced_boards, forced_byes)?;
    tournament.confirm_round()?;

    // Score each non-playing cell exactly as the table wrote it, rather than
    // leaving an absence on the tournament default.
    for (player, value) in sitout_values {
        tournament.set_sitout_value(round_number, player, value)?;
    }

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
            // The cell says which side conceded, so set the giver from it rather
            // than letting the ratings decide.
            let giver = match (h.gave, a_is_player1) {
                (true, true) | (false, false) => Winner::Player1,
                (true, false) | (false, true) => Winner::Player2,
            };
            tournament.set_board_handicap_from_source(round_number, idx, h.handicap, giver)?;
        }
        let winner = outcome.winner(a_is_player1);
        tournament.toggle_board_winner(round_number, idx, winner)?;
        if outcome == Outcome::Draw {
            tournament.set_board_drawn(round_number, idx, true)?;
        }
    }

    // Every board was forced from `results` and just resolved, so recording the
    // last one has already completed the round.
    Ok(())
}

/// A parsed player row: its number and fields, plus one cell per round. Produced
/// by a cross-table parser and consumed by [`build_tournament`].
#[derive(Debug, Clone)]
pub(crate) struct RawRow {
    pub(crate) number: u32,
    pub(crate) last_name: String,
    pub(crate) first_name: String,
    pub(crate) nationality: Option<String>,
    pub(crate) rating: Option<u32>,
    /// The player's dan/kyu grade, if the source carries one.
    pub(crate) grade: Option<crate::player::Grade>,
    pub(crate) cells: Vec<Cell>,
}

/// A parsed round cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cell {
    Bye,
    Absent,
    /// A half-point bye/absence (`0=`): the player scored ½ without playing.
    /// Reconstructed as an absence whose sit-out is worth [`SitoutValue::Half`].
    HalfBye,
    Game {
        opponent: u32,
        outcome: Outcome,
        handicap: Option<CellHandicap>,
    },
}

/// The handicap read off one cell: the odds, and whether *this* player conceded
/// them (`-` in the cell) rather than received them (`+`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CellHandicap {
    pub(crate) handicap: Handicap,
    pub(crate) gave: bool,
}

/// The result of a game from one player's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
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

/// Parse a single round cell: `0+` bye, `0-`/`0#` absence, `0=` half-point
/// bye/absence, or `<opp><+/-/=>` with an optional `(<±><code>)` handicap. A
/// trailing floater mark is tolerated; anything else after the sign is rejected.
pub(crate) fn parse_cell(cell: &str, line_no: usize) -> Result<Cell, ResultImportError> {
    let bad = || ResultImportError::BadCell {
        line: line_no,
        cell: cell.to_string(),
    };
    // Floaters are dropped from the table, but tolerate a stray trailing ^/v.
    let cell = cell.trim_end_matches(['^', 'v']);

    let digits_end = cell.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
    let (digits, rest) = cell.split_at(digits_end);
    let opponent: u32 = digits.parse().map_err(|_| bad())?;

    let mut chars = rest.chars();
    let sign = chars.next().ok_or_else(bad)?;
    let tail = chars.as_str();

    if opponent == 0 {
        // A bye/absence has no board, so odds cannot have been given — but a few
        // published tables annotate one anyway (a player handicapped in every
        // game, the mark carried onto their bye row too). The suffix is still
        // validated, then dropped: there is nothing to attach it to. Such files
        // are listed in the corpus anomaly report, see `scripts/fesa_anomalies.py`.
        if !tail.is_empty() {
            parse_handicap(tail, line_no, cell)?;
        }
        return match sign {
            '+' => Ok(Cell::Bye),
            '-' | '#' => Ok(Cell::Absent),
            '=' => Ok(Cell::HalfBye), // `0=`: a half-point bye/absence
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

/// Parse a `(<±><code>)` handicap suffix: the sign says whether this player
/// conceded the odds (`-`) or received them (`+`).
///
/// Codes are matched case-insensitively — the same table may write `(-R)` and
/// `(-r)` — and `m` is accepted as an older spelling of the sente handicap.
fn parse_handicap(
    suffix: &str,
    line_no: usize,
    cell: &str,
) -> Result<CellHandicap, ResultImportError> {
    let bad = || ResultImportError::BadCell {
        line: line_no,
        cell: cell.to_string(),
    };
    let inner = suffix
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(bad)?;
    let gave = match inner.chars().next() {
        Some('-') => true,
        Some('+') => false,
        _ => return Err(bad()),
    };
    let code = inner[1..].to_ascii_lowercase();
    let code = if code == "m" { "s".to_string() } else { code };
    let handicap = Handicap::from_code(&code).ok_or_else(bad)?;
    Ok(CellHandicap { handicap, gave })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row with the given number, name and cells.
    fn row(number: u32, last: &str, rating: u32, cells: &[Cell]) -> RawRow {
        RawRow {
            number,
            last_name: last.to_string(),
            first_name: last.to_lowercase(),
            nationality: Some("FR".to_string()),
            rating: Some(rating),
            grade: None,
            cells: cells.to_vec(),
        }
    }

    fn game(opponent: u32, outcome: Outcome) -> Cell {
        Cell::Game {
            opponent,
            outcome,
            handicap: None,
        }
    }

    /// A game cell carrying a handicap this player gave (`gave`) or received.
    fn handicap_game(opponent: u32, outcome: Outcome, handicap: Handicap, gave: bool) -> Cell {
        Cell::Game {
            opponent,
            outcome,
            handicap: Some(CellHandicap { handicap, gave }),
        }
    }

    /// The tournament number of the player with this last name.
    fn tid(t: &Tournament, last: &str) -> TournamentId {
        t.players
            .iter()
            .find(|p| p.last_name == last)
            .expect("player exists")
            .tournament_id
            .unwrap()
    }

    fn points(t: &Tournament, last: &str) -> crate::HalfPoints {
        let id = t
            .players
            .iter()
            .find(|p| p.last_name == last)
            .expect("player exists")
            .id;
        t.standings()
            .into_iter()
            .find(|s| s.player_id == id)
            .unwrap()
            .points
    }

    #[test]
    fn parses_every_cell_shape() {
        assert_eq!(parse_cell("0+", 1).unwrap(), Cell::Bye);
        assert_eq!(parse_cell("0-", 1).unwrap(), Cell::Absent);
        assert_eq!(parse_cell("0#", 1).unwrap(), Cell::Absent);
        assert_eq!(parse_cell("0=", 1).unwrap(), Cell::HalfBye);
        assert_eq!(parse_cell("12+", 1).unwrap(), game(12, Outcome::Win));
        assert_eq!(parse_cell("7-", 1).unwrap(), game(7, Outcome::Loss));
        assert_eq!(parse_cell("3=", 1).unwrap(), game(3, Outcome::Draw));
        // A floater mark is tolerated.
        assert_eq!(parse_cell("5+^", 1).unwrap(), game(5, Outcome::Win));
    }

    #[test]
    fn parses_handicap_annotations() {
        // `-` concedes the odds, `+` receives them.
        assert_eq!(
            parse_cell("9+(-5p)", 1).unwrap(),
            handicap_game(9, Outcome::Win, Handicap::FivePiece, true)
        );
        assert_eq!(
            parse_cell("1-(+5p)", 1).unwrap(),
            handicap_game(1, Outcome::Loss, Handicap::FivePiece, false)
        );
        // Codes are case-insensitive: the same corpus writes both.
        assert_eq!(
            parse_cell("4+(-R)", 1).unwrap(),
            handicap_game(4, Outcome::Win, Handicap::Rook, true)
        );
        assert_eq!(
            parse_cell("4+(-rl)", 1).unwrap(),
            handicap_game(4, Outcome::Win, Handicap::RookLance, true)
        );
        // `m` is an older spelling of the sente handicap.
        assert_eq!(
            parse_cell("3+(+m)", 1).unwrap(),
            handicap_game(3, Outcome::Win, Handicap::Sente, false)
        );
        // A bye annotated with odds (some tables do) keeps the bye and drops the
        // mark — no board was played, so nothing can carry it.
        assert_eq!(parse_cell("0+(+6p)", 1).unwrap(), Cell::Bye);
        assert_eq!(
            parse_cell("3+(+s)", 1).unwrap(),
            handicap_game(3, Outcome::Win, Handicap::Sente, false)
        );
    }

    #[test]
    fn rejects_malformed_cells() {
        // No digits before the sign.
        assert!(matches!(
            parse_cell("x+", 4),
            Err(ResultImportError::BadCell { cell, line: 4 }) if cell == "x+"
        ));
        // A handicap suffix must be a parenthesised signed code.
        assert!(matches!(
            parse_cell("2+-r", 1),
            Err(ResultImportError::BadCell { cell, .. }) if cell == "2+-r"
        ));
        assert!(matches!(
            parse_cell("2+(r)", 1),
            Err(ResultImportError::BadCell { cell, .. }) if cell == "2+(r)"
        ));
        // An unknown code is rejected rather than dropped.
        assert!(matches!(
            parse_cell("2+(-z)", 1),
            Err(ResultImportError::BadCell { cell, .. }) if cell == "2+(-z)"
        ));
        // A non-game cell's annotation is validated even though it is dropped.
        assert!(matches!(
            parse_cell("0+(-z)", 1),
            Err(ResultImportError::BadCell { cell, .. }) if cell == "0+(-z)"
        ));
        assert!(matches!(
            parse_cell("0+x", 1),
            Err(ResultImportError::BadCell { cell, .. }) if cell == "0+x"
        ));
        // Unknown signs, for a non-game and a game cell.
        assert!(matches!(
            parse_cell("0*", 1),
            Err(ResultImportError::BadCell { cell, .. }) if cell == "0*"
        ));
        assert!(matches!(
            parse_cell("2*", 1),
            Err(ResultImportError::BadCell { cell, .. }) if cell == "2*"
        ));
        // No sign at all.
        assert!(matches!(
            parse_cell("12", 1),
            Err(ResultImportError::BadCell { .. })
        ));
    }

    #[test]
    fn builds_games_byes_and_absences() {
        // R1: Alpha beats Beta, Gamma takes a bye, Delta is absent.
        // R2: Alpha beats Gamma, Beta draws Delta.
        let rows = vec![
            row(
                1,
                "Alpha",
                2000,
                &[game(2, Outcome::Win), game(3, Outcome::Win)],
            ),
            row(
                2,
                "Beta",
                1500,
                &[game(1, Outcome::Loss), game(4, Outcome::Draw)],
            ),
            row(3, "Gamma", 1000, &[Cell::Bye, game(1, Outcome::Loss)]),
            row(4, "Delta", 900, &[Cell::Absent, game(2, Outcome::Draw)]),
        ];
        let (t, id_of) = build_tournament(Some("Mini Open"), rows).unwrap();

        assert_eq!(t.name, "Mini Open");
        assert_eq!(t.players.len(), 4);
        assert_eq!(t.rounds.len(), 2);
        assert!(t.rounds.iter().all(|r| r.completed));
        // The map is keyed by the table's Nr, valued by the assigned number.
        assert_eq!(id_of[&1], tid(&t, "Alpha"));

        // The bye and the absence landed on the right players in round 1.
        assert_eq!(
            t.rounds[0].byes().collect::<Vec<_>>(),
            vec![tid(&t, "Gamma")]
        );
        assert_eq!(
            t.rounds[0].absentees().collect::<Vec<_>>(),
            vec![tid(&t, "Delta")]
        );

        // Alpha won both games; the drawn board is flagged as a draw.
        assert_eq!(points(&t, "Alpha"), 4); // 2 wins = 4 half-points
        let drawn = t.rounds[1]
            .boards
            .iter()
            .find(|b| b.outcome.drawn())
            .expect("the Beta–Delta draw");
        assert!(drawn.outcome.winner().is_some()); // a decisive winner is still recorded
    }

    #[test]
    fn a_loss_recorded_by_the_lower_numbered_player_still_resolves() {
        // The processing side (lower Nr) is the loser — the `Outcome::Loss` arm.
        let rows = vec![
            row(1, "Alpha", 2000, &[game(2, Outcome::Loss)]),
            row(2, "Beta", 1000, &[game(1, Outcome::Win)]),
        ];
        let (t, _) = build_tournament(None, rows).unwrap();
        assert_eq!(t.name, "Imported tournament"); // the unnamed default
        assert_eq!(points(&t, "Beta"), 2);
        assert_eq!(points(&t, "Alpha"), 0);
    }

    #[test]
    fn a_half_point_bye_scores_half_without_touching_the_setting() {
        let rows = vec![
            row(1, "Alpha", 2000, &[game(2, Outcome::Win)]),
            row(2, "Beta", 1000, &[game(1, Outcome::Loss)]),
            row(3, "Gamma", 1500, &[Cell::HalfBye]),
        ];
        let (t, _) = build_tournament(None, rows).unwrap();
        assert!(!t.settings.half_point_absences, "the setting is untouched");
        let gamma = tid(&t, "Gamma");
        assert_eq!(t.rounds[0].absentees().collect::<Vec<_>>(), vec![gamma]);
        assert_eq!(t.rounds[0].sitout(gamma).unwrap().value, SitoutValue::Half);
        assert_eq!(points(&t, "Gamma"), 1); // half a point = 1 half-unit
    }

    #[test]
    fn several_byes_in_one_round_all_keep_their_point() {
        // A genuine bye plus a forfeit win, which a cross-table renders alike.
        let rows = vec![
            row(1, "Alpha", 2000, &[Cell::Bye]),
            row(2, "Beta", 1900, &[Cell::Bye]),
            row(3, "Gamma", 1800, &[game(4, Outcome::Win)]),
            row(4, "Delta", 1700, &[game(3, Outcome::Loss)]),
        ];
        let (t, _) = build_tournament(None, rows).unwrap();
        assert_eq!(
            t.rounds[0].byes().collect::<Vec<_>>(),
            vec![tid(&t, "Alpha"), tid(&t, "Beta")]
        );
        assert_eq!(t.rounds[0].absentees().next(), None);
        assert_eq!(points(&t, "Alpha"), 2);
        assert_eq!(points(&t, "Beta"), 2);
    }

    #[test]
    fn rejects_no_rows() {
        assert_eq!(
            build_tournament(None, Vec::new()).unwrap_err(),
            ResultImportError::NoPlayers
        );
    }

    #[test]
    fn rejects_rows_with_inconsistent_round_counts() {
        let rows = vec![
            row(1, "Alpha", 2000, &[game(2, Outcome::Win), Cell::Bye]),
            row(2, "Beta", 1000, &[game(1, Outcome::Loss)]),
        ];
        assert!(matches!(
            build_tournament(None, rows),
            Err(ResultImportError::InconsistentRounds {
                expected: 2,
                found: 1
            })
        ));
    }

    #[test]
    fn rejects_duplicate_player_numbers() {
        let rows = vec![
            row(1, "Alpha", 2000, &[game(2, Outcome::Win)]),
            row(1, "Beta", 1000, &[game(1, Outcome::Loss)]),
        ];
        assert_eq!(
            build_tournament(None, rows).unwrap_err(),
            ResultImportError::DuplicateNumber(1)
        );
    }

    #[test]
    fn rejects_an_unknown_opponent() {
        let rows = vec![row(1, "Alpha", 2000, &[game(9, Outcome::Win)])];
        assert!(matches!(
            build_tournament(None, rows),
            Err(ResultImportError::UnknownOpponent {
                round: 1,
                by: 1,
                opponent: 9
            })
        ));
    }

    #[test]
    fn rejects_inconsistent_results() {
        // Both players claim a win over each other.
        let rows = vec![
            row(1, "Alpha", 2000, &[game(2, Outcome::Win)]),
            row(2, "Beta", 1000, &[game(1, Outcome::Win)]),
        ];
        assert!(matches!(
            build_tournament(None, rows),
            Err(ResultImportError::InconsistentResult {
                round: 1,
                a: 1,
                b: 2
            })
        ));
    }

    #[test]
    fn rejects_an_asymmetric_game() {
        // Alpha claims a game vs Beta, but Beta's cell is a bye.
        let rows = vec![
            row(1, "Alpha", 2000, &[game(2, Outcome::Win)]),
            row(2, "Beta", 1000, &[Cell::Bye]),
        ];
        assert!(matches!(
            build_tournament(None, rows),
            Err(ResultImportError::Asymmetric {
                round: 1,
                a: 1,
                b: 2
            })
        ));

        // …and one where Beta names a third player instead of Alpha.
        let rows = vec![
            row(1, "Alpha", 2000, &[game(2, Outcome::Win)]),
            row(2, "Beta", 1000, &[game(3, Outcome::Loss)]),
            row(3, "Gamma", 1500, &[Cell::Bye]),
        ];
        assert!(matches!(
            build_tournament(None, rows),
            Err(ResultImportError::Asymmetric {
                round: 1,
                a: 1,
                b: 2
            })
        ));
    }

    #[test]
    fn a_handicap_takes_its_giver_from_the_cell_not_the_ratings() {
        // The *lower*-rated Beta concedes the odds — what the cells say, and the
        // opposite of what a rating-derived giver would pick.
        let rows = vec![
            row(
                1,
                "Alpha",
                2000,
                &[handicap_game(2, Outcome::Win, Handicap::Rook, false)],
            ),
            row(
                2,
                "Beta",
                1000,
                &[handicap_game(1, Outcome::Loss, Handicap::Rook, true)],
            ),
        ];
        let (t, _) = build_tournament(None, rows).unwrap();
        let board = &t.rounds[0].boards[0];
        let game = board.handicap.expect("handicap set");
        assert_eq!(game.handicap, Handicap::Rook);
        let beta_side = if board.player1 == tid(&t, "Beta") {
            Winner::Player1
        } else {
            Winner::Player2
        };
        assert_eq!(game.giver, beta_side);
    }

    #[test]
    fn rejects_handicaps_the_two_sides_disagree_on() {
        // Only one side annotated.
        let one_sided = vec![
            row(
                1,
                "Alpha",
                2000,
                &[handicap_game(2, Outcome::Win, Handicap::Rook, true)],
            ),
            row(2, "Beta", 1000, &[game(1, Outcome::Loss)]),
        ];
        assert!(matches!(
            build_tournament(None, one_sided),
            Err(ResultImportError::InconsistentHandicap {
                round: 1,
                a: 1,
                b: 2
            })
        ));

        // Both claim to have conceded.
        let both_gave = vec![
            row(
                1,
                "Alpha",
                2000,
                &[handicap_game(2, Outcome::Win, Handicap::Rook, true)],
            ),
            row(
                2,
                "Beta",
                1000,
                &[handicap_game(1, Outcome::Loss, Handicap::Rook, true)],
            ),
        ];
        assert!(matches!(
            build_tournament(None, both_gave),
            Err(ResultImportError::InconsistentHandicap {
                round: 1,
                a: 1,
                b: 2
            })
        ));

        // Different odds on each side.
        let mismatched = vec![
            row(
                1,
                "Alpha",
                2000,
                &[handicap_game(2, Outcome::Win, Handicap::Rook, true)],
            ),
            row(
                2,
                "Beta",
                1000,
                &[handicap_game(1, Outcome::Loss, Handicap::Bishop, false)],
            ),
        ];
        assert!(matches!(
            build_tournament(None, mismatched),
            Err(ResultImportError::InconsistentHandicap {
                round: 1,
                a: 1,
                b: 2
            })
        ));
    }
}
