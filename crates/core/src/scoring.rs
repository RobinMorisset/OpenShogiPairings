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
    /// Total score in **half-point units** (×2): MacMahon starting points plus
    /// victories (the effective winner of a board, and a bye, each a win worth
    /// `2`), plus `1` for each half-point absence (a `0=`). Kept in halves so a
    /// half-point stays an exact integer; divide by 2 for display.
    pub points: u32,
    /// Games won (effective winner; a bye counts as a win). A whole game count,
    /// **not** in half-point units — a half-point absence is not a victory.
    pub victories: u32,
    /// MacMahon starting points, in **half-point units** (×2).
    pub macmahon: u32,
    /// Opponents faced, one entry per game (so a rematch would count twice, e.g.
    /// in SOS).
    pub opponents: Vec<Uuid>,
    /// Opponents defeated (effective winner), one entry per game.
    pub defeated: Vec<Uuid>,
    /// Cumulative sum of the player's running points total, added up after each
    /// completed round (the "cumulative" tie-break, MacMahon-inclusive). In
    /// **half-point units**, since it sums [`points`](Self::points).
    pub cuss_m: u32,
    /// Cumulative sum of the player's running win total, added up after each
    /// completed round (the "cumulative" tie-break, wins only). A whole count.
    pub cuss_w: u32,
    /// The player's running points total as it stood after each completed round
    /// (one entry per round) — the sequence CUSSM sums, kept for the breakdown.
    /// In **half-point units**.
    pub running_points: Vec<u32>,
    /// The player's running win total after each completed round — the sequence
    /// CUSSW sums.
    pub running_wins: Vec<u32>,
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
    // When MacMahon is awarded from the live estimate, compute it once here and
    // feed the rounded estimate in place of each player's registration rating
    // for the ELO-based thresholds. Grade thresholds read the player's grade and
    // so are unaffected. Skipped (and left `None`) whenever the toggle is off or
    // there is no ELO threshold to compare against.
    let estimated_elos = settings
        .macmahon_from_estimate_active()
        .then(|| crate::elo::estimate_elos(players, settings, rounds));
    let mut by_id: HashMap<Uuid, PlayerScore> = players
        .iter()
        .map(|p| {
            // The rating the ELO thresholds see: the rounded live estimate when
            // estimate-based MacMahon is active, else the static registration
            // rating. A player with no counted games sits at their prior mean, so
            // every player has an estimate.
            let mm_rating = match &estimated_elos {
                Some(est) => Some(
                    est.get(&p.id)
                        .copied()
                        .unwrap_or(crate::elo::UNRATED_PRIOR_MEAN)
                        .round() as u32,
                ),
                None => p.rating,
            };
            // All point-like quantities are kept in half-point units (×2) so a
            // later half-point absence adds an exact `1`.
            let macmahon = settings.macmahon_points_at(mm_rating, p.grade, rounds_played) * 2;
            // Manual bonuses/maluses are folded in alongside MacMahon starting
            // points, before any round is replayed, so they shape both the
            // standings and the score-gap pairing weight from here on. They are
            // whole-point deltas, so doubled into half-point units. The effective
            // score can't go below zero.
            let adjustment: i32 = p.adjustments.iter().map(|a| a.delta).sum();
            let points = (macmahon as i32 + adjustment * 2).max(0) as u32;
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
                    running_points: Vec::new(),
                    running_wins: Vec::new(),
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
            // A no-show is not a real game: the player who showed up is scored
            // exactly like a bye and the absentee like an absence (both handled
            // below), so neither records the other as an opponent, and there is
            // no float to read off this board.
            if board.no_show.is_some() {
                continue;
            }
            let (a, b) = (board.player1, board.player2);
            if !by_id.contains_key(&a) || !by_id.contains_key(&b) {
                continue;
            }
            // A long board (two rounds, two points) counts as *two* games against
            // the same opponent for the opponent-based tie-breaks (SOS, SODOS,
            // SOSOS, Buchholz), so it is recorded twice. It still feeds ELO as a
            // single game (that reads the boards directly, not these lists).
            let reps = if board.long { 2 } else { 1 };
            for _ in 0..reps {
                by_id.get_mut(&a).unwrap().opponents.push(b);
                by_id.get_mut(&b).unwrap().opponents.push(a);
            }

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
        // A no-show has the same effect on the player who showed up as a bye: a
        // downfloat they can't be given twice.
        for board in &round.boards {
            if let Some(present) = board.no_show_opponent() {
                if let Some(s) = by_id.get_mut(&present) {
                    s.had_bye = true;
                    s.last_descended = Some(round.number);
                }
            }
        }
        // A cup bye (an unopposed bracket advance) is a bye all the same.
        for &player in &round.cup_byes {
            if let Some(s) = by_id.get_mut(&player) {
                s.had_bye = true;
                s.last_descended = Some(round.number);
            }
        }

        // Apply this round's results (effective winner scores).
        for board in &round.boards {
            let (winner, loser) = match board.effective_winner(settings.handicap_wiel_rule) {
                Some(Winner::Player1) => (board.player1, board.player2),
                Some(Winner::Player2) => (board.player2, board.player1),
                None => continue,
            };
            // A long board (double time control) is worth two points and counts
            // as two games against the same opponent for the point/victory totals
            // and the opponent-based tie-breaks; it stays a single game for ELO.
            let reps = if board.long { 2 } else { 1 };
            if let Some(s) = by_id.get_mut(&winner) {
                s.points += 2 * reps; // a win is 2 half-points (×2 for a long game)
                s.victories += reps;
                for _ in 0..reps {
                    s.defeated.push(loser);
                }
            }
        }
        if let Some(bye) = round.bye {
            if let Some(s) = by_id.get_mut(&bye) {
                s.points += 2; // a win is 2 half-points
                s.victories += 1;
            }
        }
        // The player who showed up on a no-show board scores the free point, as
        // for a bye. The absentee scores nothing (like an absence).
        for board in &round.boards {
            if let Some(present) = board.no_show_opponent() {
                if let Some(s) = by_id.get_mut(&present) {
                    // A long board resolved by forfeit still scores its long
                    // weight (two points), unless the referee demoted it.
                    let reps = if board.long { 2 } else { 1 };
                    s.points += 2 * reps; // a win is 2 half-points (×2 for a long game)
                    s.victories += reps;
                }
            }
        }
        // A cup bye scores the free point too — the player advanced unopposed.
        for &player in &round.cup_byes {
            if let Some(s) = by_id.get_mut(&player) {
                s.points += 2; // a win is 2 half-points
                s.victories += 1;
            }
        }
        // A deliberate absence scores half a point when the referee enabled it
        // (a `0=` in the cross-table): +1 half-point, but not a win. `round.absent`
        // is the sat-out set (no board this round); a no-show forfeit is on a
        // board and stays at 0.
        if settings.half_point_absences {
            for id in &round.absent {
                if let Some(s) = by_id.get_mut(id) {
                    s.points += 1;
                }
            }
        }

        // Cumulative tie-break: after this round is scored, add every player's
        // running total to their running sum. A round the player sat out still
        // contributes their (unchanged) total, matching the classic "cumulative"
        // definition of a sum over the sequence of rounds.
        for s in by_id.values_mut() {
            s.cuss_m += s.points;
            s.cuss_w += s.victories;
            s.running_points.push(s.points);
            s.running_wins.push(s.victories);
        }
    }

    Scores { by_id }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::{Board, CupStage, NoShow};
    use crate::settings::MacMahonThreshold;

    fn player(tid: u32, rating: Option<u32>) -> Player {
        Player {
            id: Uuid::new_v4(),
            tournament_id: Some(tid),
            last_name: format!("P{tid}"),
            first_name: String::new(),
            rating,
            grade: None,
            fesa_games: None,
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
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        };
        let scores = compute_scores(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &[round],
        );
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
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        };
        let settings = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)],
            ..Default::default()
        };
        let scores = compute_scores(&[a.clone(), b.clone()], &settings, &[round]);
        assert_eq!(scores.get(&a.id).last_descended, Some(1));
        assert_eq!(scores.get(&b.id).last_ascended, Some(1));
    }

    #[test]
    fn no_show_scores_like_a_bye_for_the_present_player_and_an_absence_for_the_absentee() {
        // A and B are paired but B doesn't show up. A is credited the point (as
        // for a bye: +1 point/win, marked as having "had a bye", a downfloat),
        // while B scores nothing. Neither records the other as an opponent.
        let a = player(1, None);
        let b = player(2, None);
        let round = Round {
            number: 1,
            boards: vec![Board {
                no_show: Some(NoShow::Player2), // player2 (B) is absent
                ..Board::pending(a.id, b.id, Some(0), PairingSource::Swiss)
            }],
            bye: None,
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        };
        let scores = compute_scores(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &[round],
        );
        assert_eq!(scores.get(&a.id).points, 2); // 1 point = 2 half-points
        assert_eq!(scores.get(&a.id).victories, 1);
        assert!(scores.get(&a.id).had_bye);
        assert_eq!(scores.get(&a.id).last_descended, Some(1));
        assert!(scores.get(&a.id).opponents.is_empty());
        // The absentee is untouched — no point, no opponent, no float.
        assert_eq!(scores.get(&b.id).points, 0);
        assert_eq!(scores.get(&b.id).victories, 0);
        assert!(!scores.get(&b.id).had_bye);
        assert!(scores.get(&b.id).opponents.is_empty());
    }

    #[test]
    fn double_no_show_scores_nothing_for_either_player() {
        // Neither player showed up: both take a zero loss, nobody is credited.
        let a = player(1, None);
        let b = player(2, None);
        let round = Round {
            number: 1,
            boards: vec![Board {
                no_show: Some(NoShow::Both),
                ..Board::pending(a.id, b.id, Some(0), PairingSource::Swiss)
            }],
            bye: None,
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        };
        let scores = compute_scores(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &[round],
        );
        for id in [a.id, b.id] {
            assert_eq!(scores.get(&id).points, 0);
            assert_eq!(scores.get(&id).victories, 0);
            assert!(!scores.get(&id).had_bye);
            assert!(scores.get(&id).opponents.is_empty());
        }
    }

    #[test]
    fn cup_bye_scores_a_point_like_a_bye() {
        let a = player(1, None);
        let round = Round {
            number: 1,
            boards: Vec::new(),
            bye: None,
            cup_byes: vec![a.id],
            absent: Vec::new(),
            completed: true,
        };
        let scores = compute_scores(
            std::slice::from_ref(&a),
            &TournamentSettings::default(),
            &[round],
        );
        assert_eq!(scores.get(&a.id).points, 2); // 1 point = 2 half-points
        assert_eq!(scores.get(&a.id).victories, 1);
        assert!(scores.get(&a.id).had_bye);
    }

    #[test]
    fn an_absence_scores_half_a_point_only_when_the_setting_is_on() {
        // A is marked absent for round 1 (no board), while B beats C. With the
        // setting off A scores 0; with it on A scores half a point (1 half-unit)
        // — and it is not a victory. The real win still scores a full point.
        let a = player(1, None);
        let b = player(2, None);
        let c = player(3, None);
        let round = Round {
            number: 1,
            boards: vec![Board {
                result: Some(Winner::Player1),
                ..Board::pending(b.id, c.id, Some(0), PairingSource::Swiss)
            }],
            bye: None,
            cup_byes: Vec::new(),
            absent: vec![a.id],
            completed: true,
        };
        let players = [a.clone(), b.clone(), c.clone()];

        let off = compute_scores(
            &players,
            &TournamentSettings::default(),
            std::slice::from_ref(&round),
        );
        assert_eq!(off.get(&a.id).points, 0); // no half-point without the setting

        let on_settings = TournamentSettings {
            half_point_absences: true,
            ..Default::default()
        };
        let on = compute_scores(&players, &on_settings, std::slice::from_ref(&round));
        assert_eq!(on.get(&a.id).points, 1); // half a point = 1 half-unit
        assert_eq!(on.get(&a.id).victories, 0); // but not a win
        assert_eq!(on.get(&b.id).points, 2); // the real win is a full point
    }

    #[test]
    fn long_board_scores_two_points_two_victories_and_counts_twice_for_tiebreaks() {
        // A beats B on a long (two-round) board: A scores 2 points (4 halves) and
        // 2 victories, and the game counts as two games versus B for the
        // opponent/defeated lists (so SOS/SODOS weight B double).
        let a = player(1, None);
        let b = player(2, None);
        let round = Round {
            number: 1,
            boards: vec![Board {
                result: Some(Winner::Player1),
                long: true,
                ..Board::pending(a.id, b.id, Some(0), PairingSource::Swiss)
            }],
            bye: None,
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        };
        let scores = compute_scores(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &[round],
        );
        assert_eq!(scores.get(&a.id).points, 4); // 2 points = 4 half-points
        assert_eq!(scores.get(&a.id).victories, 2);
        assert_eq!(scores.get(&a.id).defeated, vec![b.id, b.id]); // twice
        assert_eq!(scores.get(&a.id).opponents, vec![b.id, b.id]);
        assert_eq!(scores.get(&b.id).opponents, vec![a.id, a.id]);
        assert_eq!(scores.get(&b.id).points, 0);
        assert_eq!(scores.get(&b.id).victories, 0);
    }

    #[test]
    fn pending_long_board_records_the_opponent_but_awards_no_points() {
        // A long board with no result yet: the opponent is already recorded
        // (twice), but nobody has scored — the "padded" standings state.
        let a = player(1, None);
        let b = player(2, None);
        let round = Round {
            number: 1,
            boards: vec![Board {
                long: true,
                ..Board::pending(a.id, b.id, Some(0), PairingSource::Swiss)
            }],
            bye: None,
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true, // completed even with the long board pending
        };
        let scores = compute_scores(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &[round],
        );
        assert_eq!(scores.get(&a.id).points, 0);
        assert_eq!(scores.get(&a.id).victories, 0);
        assert_eq!(scores.get(&a.id).opponents, vec![b.id, b.id]);
        assert!(scores.get(&a.id).defeated.is_empty());
    }

    #[test]
    fn long_board_resolved_by_forfeit_scores_the_long_weight() {
        // A long board where B is a no-show: A takes the free point at the long
        // weight (2 points / 2 victories), like a doubled bye.
        let a = player(1, None);
        let b = player(2, None);
        let round = Round {
            number: 1,
            boards: vec![Board {
                no_show: Some(NoShow::Player2),
                long: true,
                ..Board::pending(a.id, b.id, Some(0), PairingSource::Swiss)
            }],
            bye: None,
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        };
        let scores = compute_scores(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &[round],
        );
        assert_eq!(scores.get(&a.id).points, 4); // 2 points
        assert_eq!(scores.get(&a.id).victories, 2);
        assert_eq!(scores.get(&b.id).points, 0);
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
        assert_eq!(scores.points(&a.id), 4); // +2 whole = +4 half-points
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
                ..Board::pending(
                    a.id,
                    b.id,
                    Some(5),
                    PairingSource::Cup {
                        stage: CupStage::Final,
                    },
                )
            }],
            bye: None,
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        };
        let scores = compute_scores(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &[round],
        );
        // The win still scores, and both are recorded as opponents faced (so a
        // later Swiss round won't re-pair them).
        assert_eq!(scores.get(&a.id).points, 2); // 1 point = 2 half-points
        assert_eq!(scores.get(&a.id).opponents, vec![b.id]);
        assert_eq!(scores.get(&b.id).opponents, vec![a.id]);
        // ...but the cup game left no float history.
        assert_eq!(scores.get(&a.id).last_descended, None);
        assert_eq!(scores.get(&b.id).last_ascended, None);
    }

    #[test]
    fn macmahon_can_be_awarded_from_the_live_estimate() {
        // A registers at 1400 — below the single 1450 ELO threshold, so 0
        // MacMahon points on paper — but beats three ~2000 opponents, so their
        // estimated strength climbs well above the threshold. Estimate-based
        // MacMahon then earns them the point; the static rating does not.
        let a = player(1, Some(1400));
        let opps: Vec<Player> = (0..3).map(|i| player(10 + i, Some(2000))).collect();
        let mut all = vec![a.clone()];
        all.extend(opps.iter().cloned());
        let rounds: Vec<Round> = opps
            .iter()
            .enumerate()
            .map(|(i, o)| Round {
                number: i as u32 + 1,
                boards: vec![Board {
                    result: Some(Winner::Player1), // A wins every game
                    ..Board::pending(a.id, o.id, Some(0), PairingSource::Swiss)
                }],
                bye: None,
                cup_byes: Vec::new(),
                absent: Vec::new(),
                completed: true,
            })
            .collect();

        let base = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1450)],
            ..Default::default()
        };
        // Static registration rating: A is below 1450, so no MacMahon points.
        let off = compute_scores(&all, &base, &rounds);
        assert_eq!(off.get(&a.id).macmahon, 0);

        // Estimate-based: A's estimate has climbed past 1450, earning the point
        // (and lifting total points to 1 MacMahon + 3 wins = 4, i.e. 8 halves).
        let on = TournamentSettings {
            macmahon_from_estimated_elo: true,
            ..base
        };
        let on = compute_scores(&all, &on, &rounds);
        assert_eq!(on.get(&a.id).macmahon, 2); // 1 MacMahon point = 2 half-points
        assert_eq!(on.get(&a.id).points, 8); // (1 + 3) points = 8 half-points
    }

    #[test]
    fn estimate_based_macmahon_is_inert_without_an_elo_threshold() {
        // The toggle is on, but the only threshold is grade-based, so the
        // estimate has nothing to compare against and scoring is unchanged.
        use crate::player::Grade;
        let mut a = player(1, Some(1400));
        a.grade = Some(Grade::dan(1));
        let settings = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::grade(Grade::dan(1))],
            macmahon_from_estimated_elo: true,
            ..Default::default()
        };
        let scores = compute_scores(std::slice::from_ref(&a), &settings, &[]);
        // Meets the 1-dan grade threshold on grade, exactly as with the toggle off.
        assert_eq!(scores.get(&a.id).macmahon, 2); // 1 MacMahon point = 2 half-points
    }
}
