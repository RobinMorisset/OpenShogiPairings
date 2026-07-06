//! Shared per-player scoring, replayed from the completed rounds.
//!
//! Both the pairing engine ([`crate::pairing`]) and the results standings
//! ([`crate::standings`]) need the same underlying figures — each player's
//! points, who they faced and beat, whether they had a bye, and their float
//! history — so that computation lives here once rather than in each consumer.

use std::cmp::Ordering;
use std::collections::HashMap;

use uuid::Uuid;

use crate::player::Player;
use crate::round::{Round, Winner};
use crate::settings::TournamentSettings;

/// One player's accumulated state going into the next round.
pub(crate) struct PlayerScore {
    /// MacMahon starting points plus victories (the effective winner of a board,
    /// and a bye, each count as a win).
    pub points: u32,
    /// Games won (effective winner; a bye counts as a win).
    pub victories: u32,
    /// MacMahon starting points.
    pub macmahon: u32,
    /// Opponents faced, one entry per game (so a rematch would count twice, e.g.
    /// in SOS).
    pub opponents: Vec<Uuid>,
    /// Opponents defeated (effective winner), one entry per game.
    pub defeated: Vec<Uuid>,
    /// Whether the player has taken a bye.
    pub had_bye: bool,
    /// Round number of the most recent round the player floated up / down (a bye
    /// counts as a downfloat). `None` if never.
    pub last_ascended: Option<u32>,
    pub last_descended: Option<u32>,
}

/// Per-player scores keyed by player id.
pub(crate) struct Scores {
    by_id: HashMap<Uuid, PlayerScore>,
}

impl Scores {
    /// The accumulated state for a known player.
    pub fn get(&self, id: &Uuid) -> &PlayerScore {
        &self.by_id[id]
    }

    /// A player's total points, or 0 if they aren't in the tournament (e.g. an
    /// opponent that was later removed).
    pub fn points(&self, id: &Uuid) -> u32 {
        self.by_id.get(id).map_or(0, |s| s.points)
    }
}

/// Replay the **completed** rounds to accumulate each player's points, opponents,
/// bye status and float history.
///
/// A float's direction is read from each board's frozen [`points_diff`] (the
/// float as it was at pairing time), falling back to the live points-going-into-
/// the-round only for boards that predate that field. Freezing is what keeps the
/// float history correct once MacMahon thresholds can change mid-tournament, or
/// when an earlier result is edited: the score table is recomputed live, but who
/// floated in a past round is a fact of how that round was actually paired.
///
/// [`points_diff`]: crate::round::Board::points_diff
pub(crate) fn compute_scores(
    players: &[Player],
    settings: &TournamentSettings,
    rounds: &[Round],
) -> Scores {
    // MacMahon starting points use the thresholds in effect *now* — after all
    // the completed rounds — so degressive MacMahon shrinks the spread as the
    // tournament goes on. Which round each player floated in is read separately
    // from the frozen `points_diff`, so a later removal can't rewrite history.
    let rounds_played = rounds.iter().filter(|r| r.completed).count() as u32;
    let mut by_id: HashMap<Uuid, PlayerScore> = players
        .iter()
        .map(|p| {
            let macmahon = settings.macmahon_points_at(p.rating, rounds_played);
            (
                p.id,
                PlayerScore {
                    points: macmahon,
                    victories: 0,
                    macmahon,
                    opponents: Vec::new(),
                    defeated: Vec::new(),
                    had_bye: false,
                    last_ascended: None,
                    last_descended: None,
                },
            )
        })
        .collect();

    for round in rounds.iter().filter(|r| r.completed) {
        // A float is judged on the points going *into* the round, so record
        // opponents and floats before applying this round's results.
        for board in &round.boards {
            let (a, b) = (board.player1, board.player2);
            if !by_id.contains_key(&a) || !by_id.contains_key(&b) {
                continue;
            }
            by_id.get_mut(&a).unwrap().opponents.push(b);
            by_id.get_mut(&b).unwrap().opponents.push(a);

            // Direction from the frozen float, else from the live difference.
            let diff = board
                .points_diff
                .unwrap_or_else(|| by_id[&a].points as i32 - by_id[&b].points as i32);
            match diff.cmp(&0) {
                Ordering::Greater => {
                    // a had more points → a downfloats, b upfloats.
                    by_id.get_mut(&a).unwrap().last_descended = Some(round.number);
                    by_id.get_mut(&b).unwrap().last_ascended = Some(round.number);
                }
                Ordering::Less => {
                    by_id.get_mut(&a).unwrap().last_ascended = Some(round.number);
                    by_id.get_mut(&b).unwrap().last_descended = Some(round.number);
                }
                Ordering::Equal => {}
            }
        }
        if let Some(bye) = round.bye {
            if let Some(s) = by_id.get_mut(&bye) {
                s.had_bye = true;
                s.last_descended = Some(round.number); // a bye is a downfloat
            }
        }

        // Apply this round's results (effective winner scores).
        for board in &round.boards {
            let (winner, loser) = match board.effective_winner() {
                Some(Winner::Player1) => (board.player1, board.player2),
                Some(Winner::Player2) => (board.player2, board.player1),
                None => continue,
            };
            if let Some(s) = by_id.get_mut(&winner) {
                s.points += 1;
                s.victories += 1;
                s.defeated.push(loser);
            }
        }
        if let Some(bye) = round.bye {
            if let Some(s) = by_id.get_mut(&bye) {
                s.points += 1;
                s.victories += 1;
            }
        }
    }

    Scores { by_id }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::Board;

    fn player(tid: u32, rating: Option<u32>) -> Player {
        Player {
            id: Uuid::new_v4(),
            tournament_id: Some(tid),
            last_name: format!("P{tid}"),
            first_name: String::new(),
            rating,
            nationality: None,
            club: None,
        }
    }

    #[test]
    fn float_direction_follows_the_frozen_diff_not_live_points() {
        // A and B are level on live points (both 0 going in), so the live
        // difference would say "no float". The board's stored points_diff = +2
        // records that A actually floated *down* onto B (who floated up).
        let a = player(1, None);
        let b = player(2, None);
        let round = Round {
            number: 1,
            boards: vec![Board {
                player1: a.id,
                player2: b.id,
                result: Some(Winner::Player1),
                drawn: false,
                handicap: None,
                points_diff: Some(2),
            }],
            bye: None,
            absent: Vec::new(),
            completed: true,
        };
        let scores = compute_scores(&[a.clone(), b.clone()], &TournamentSettings::default(), &[round]);
        assert_eq!(scores.get(&a.id).last_descended, Some(1));
        assert_eq!(scores.get(&a.id).last_ascended, None);
        assert_eq!(scores.get(&b.id).last_ascended, Some(1));
        assert_eq!(scores.get(&b.id).last_descended, None);
    }

    #[test]
    fn missing_diff_falls_back_to_live_points() {
        // No stored diff: A (1 MacMahon point) vs B (0) → A downfloats live.
        let a = player(1, Some(2000));
        let b = player(2, Some(1000));
        let round = Round {
            number: 1,
            boards: vec![Board {
                player1: a.id,
                player2: b.id,
                result: Some(Winner::Player1),
                drawn: false,
                handicap: None,
                points_diff: None,
            }],
            bye: None,
            absent: Vec::new(),
            completed: true,
        };
        let settings = TournamentSettings {
            macmahon_thresholds: vec![1500],
            ..Default::default()
        };
        let scores = compute_scores(&[a.clone(), b.clone()], &settings, &[round]);
        assert_eq!(scores.get(&a.id).last_descended, Some(1));
        assert_eq!(scores.get(&b.id).last_ascended, Some(1));
    }
}
