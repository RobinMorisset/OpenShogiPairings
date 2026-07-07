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
use crate::round::{PairingSource, Round, Winner};
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
    /// Cumulative sum of the player's running points total, added up after each
    /// completed round (the "cumulative" tie-break, MacMahon-inclusive).
    pub cuss_m: u32,
    /// Cumulative sum of the player's running win total, added up after each
    /// completed round (the "cumulative" tie-break, wins only).
    pub cuss_w: u32,
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

    /// A player's win total, or 0 if they aren't in the tournament.
    pub fn victories(&self, id: &Uuid) -> u32 {
        self.by_id.get(id).map_or(0, |s| s.victories)
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
            // Manual bonuses/maluses are folded in alongside MacMahon starting
            // points, before any round is replayed, so they shape both the
            // standings and the score-gap pairing weight from here on. The
            // effective score can't go below zero.
            let adjustment: i32 = p.adjustments.iter().map(|a| a.delta).sum();
            let points = (macmahon as i32 + adjustment).max(0) as u32;
            (
                p.id,
                PlayerScore {
                    points,
                    victories: 0,
                    macmahon,
                    opponents: Vec::new(),
                    defeated: Vec::new(),
                    cuss_m: 0,
                    cuss_w: 0,
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

            // A cup board is a forced bracket pairing, not a Swiss float, so it
            // must not shape the players' float history (though it still counts as
            // a game faced above, so a later Swiss round won't re-pair them).
            if matches!(board.source, PairingSource::Cup { .. }) {
                continue;
            }

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

        // Cumulative tie-break: after this round is scored, add every player's
        // running total to their running sum. A round the player sat out still
        // contributes their (unchanged) total, matching the classic "cumulative"
        // definition of a sum over the sequence of rounds.
        for s in by_id.values_mut() {
            s.cuss_m += s.points;
            s.cuss_w += s.victories;
        }
    }

    Scores { by_id }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::{Board, CupStage};

    fn player(tid: u32, rating: Option<u32>) -> Player {
        Player {
            id: Uuid::new_v4(),
            tournament_id: Some(tid),
            last_name: format!("P{tid}"),
            first_name: String::new(),
            rating,
            nationality: None,
            club: None,
            eligible: false,
            adjustments: Vec::new(),
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
                result: Some(Winner::Player1),
                ..Board::pending(a.id, b.id, Some(2), PairingSource::Swiss)
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
                result: Some(Winner::Player1),
                ..Board::pending(a.id, b.id, None, PairingSource::Swiss)
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

    #[test]
    fn manual_adjustments_feed_into_points_and_floor_at_zero() {
        use crate::player::PointAdjustment;

        let mut a = player(1, None);
        a.adjustments.push(PointAdjustment {
            id: Uuid::new_v4(),
            delta: 2,
            reason: "fair-play bonus".into(),
        });
        let mut b = player(2, None);
        b.adjustments.push(PointAdjustment {
            id: Uuid::new_v4(),
            delta: -5,
            reason: "penalty".into(),
        });

        let scores = compute_scores(&[a.clone(), b.clone()], &TournamentSettings::default(), &[]);
        assert_eq!(scores.points(&a.id), 2);
        assert_eq!(scores.points(&b.id), 0); // floored, not negative
    }

    #[test]
    fn cup_boards_count_points_and_opponents_but_not_floats() {
        // A cup board with a big frozen points_diff would be a heavy downfloat if
        // it were a Swiss game — but the cup bypasses the float rules.
        let a = player(1, None);
        let b = player(2, None);
        let round = Round {
            number: 1,
            boards: vec![Board {
                result: Some(Winner::Player1),
                ..Board::pending(a.id, b.id, Some(5), PairingSource::Cup { stage: CupStage::Final })
            }],
            bye: None,
            absent: Vec::new(),
            completed: true,
        };
        let scores = compute_scores(&[a.clone(), b.clone()], &TournamentSettings::default(), &[round]);
        // The win still scores, and both are recorded as opponents faced (so a
        // later Swiss round won't re-pair them).
        assert_eq!(scores.get(&a.id).points, 1);
        assert_eq!(scores.get(&a.id).opponents, vec![b.id]);
        assert_eq!(scores.get(&b.id).opponents, vec![a.id]);
        // ...but the cup game left no float history.
        assert_eq!(scores.get(&a.id).last_descended, None);
        assert_eq!(scores.get(&b.id).last_ascended, None);
    }
}
