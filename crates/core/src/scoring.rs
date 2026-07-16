//! Shared per-player scoring, replayed from the completed rounds.
//!
//! Both the pairing engine ([`crate::pairing`]) and the results standings
//! ([`crate::standings`]) need the same underlying figures — each player's
//! points, who they faced and beat, whether they had a bye, and their float
//! history — so that computation lives here once rather than in each consumer.

use std::cmp::Ordering;

use typed_index_collections::TiVec;
use uuid::Uuid;

use crate::player::Player;
use crate::round::{Round, Winner};
use crate::settings::TournamentSettings;
use crate::units::{HalfPoints, TournamentId, Wins};

/// One player's accumulated state going into the next round. Defaultable so a
/// [`Scores`] table can leave a gap for any tournament number no current player
/// holds (number 0, and numbers freed by a pre-play removal).
#[derive(Default)]
pub(crate) struct PlayerScore {
    /// Total score: MacMahon starting points plus victories (the effective winner
    /// of a board, and a bye, each a win worth one point), plus half a point for
    /// each half-point absence (a `0=`).
    pub points: HalfPoints,
    /// Games won (effective winner; a bye counts as a win). A half-point absence
    /// is not a victory.
    pub victories: Wins,
    /// MacMahon starting points.
    pub macmahon: HalfPoints,
    /// Opponents faced, one entry per game (so a rematch would count twice, e.g.
    /// in SOS), stored by tournament number — the dense key this table is indexed
    /// by, so the opponent-based tie-breaks walk them without hashing. Only ever
    /// holds numbers of players still in the tournament (a player who has played
    /// can't be removed), so every entry resolves to a real slot.
    pub opponents: Vec<TournamentId>,
    /// Opponents defeated (effective winner), one entry per game, by tournament
    /// number (see [`Self::opponents`]).
    pub defeated: Vec<TournamentId>,
    /// Cumulative sum of the player's running points total, added up after each
    /// completed round (the "cumulative" tie-break, MacMahon-inclusive).
    pub cuss_m: HalfPoints,
    /// Cumulative sum of the player's running win total, added up after each
    /// completed round (the "cumulative" tie-break, wins only).
    pub cuss_w: Wins,
    /// The player's running points total as it stood after each completed round
    /// (one entry per round) — the sequence CUSSM sums, kept for the breakdown.
    pub running_points: Vec<HalfPoints>,
    /// The player's running win total after each completed round — the sequence
    /// CUSSW sums.
    pub running_wins: Vec<Wins>,
    /// Whether the player has taken a bye.
    pub had_bye: bool,
    /// Round number of the most recent round the player floated up / down (a bye
    /// counts as a downfloat). `None` if never.
    pub last_ascended: Option<u32>,
    pub last_descended: Option<u32>,
}

/// Per-player scores, stored in a dense vector indexed by tournament number (the
/// number assigned at finalization, so scoring only runs once every player has
/// one). `ids` is the reverse map, turning a stored number back into the id a
/// caller expects. Numbers no current player holds (0, and any freed by a
/// pre-play removal) are gaps, holding a [`PlayerScore::default`] and a nil id.
pub(crate) struct Scores {
    by_tid: TiVec<TournamentId, PlayerScore>,
    ids: TiVec<TournamentId, Uuid>,
}

impl Scores {
    /// The accumulated state for a known player.
    #[cfg(test)]
    pub fn get(&self, id: &Uuid) -> &PlayerScore {
        &self.by_tid[self.tid_of(id).expect("unknown player")]
    }

    /// The accumulated state at a tournament number — an [`PlayerScore::opponents`]
    /// / [`PlayerScore::defeated`] entry, or a [`Self::tid_of`] result. A gap
    /// number resolves to a default (all-zero) score. The [`TiVec`] is indexed by
    /// the [`TournamentId`] directly — no `as usize`.
    pub fn get_tid(&self, tid: TournamentId) -> &PlayerScore {
        &self.by_tid[tid]
    }

    /// A player's tournament number, or `None` if they aren't in the tournament.
    /// A linear scan of `ids`: the number is already known everywhere on the hot
    /// path (boards and players carry it), so this id-facing lookup is only for
    /// the [`Uuid`] boundary and tests, and isn't worth a parallel hash table.
    pub fn tid_of(&self, id: &Uuid) -> Option<TournamentId> {
        self.ids
            .iter()
            .position(|x| x == id)
            .map(TournamentId::from)
    }

    /// The player id at a tournament number (the inverse of [`Self::tid_of`]).
    pub fn id_of(&self, tid: TournamentId) -> Uuid {
        self.ids[tid]
    }

    /// A player's total points, or zero if they aren't in the tournament (e.g. an
    /// opponent that was later removed).
    #[cfg(test)]
    pub fn points(&self, id: &Uuid) -> HalfPoints {
        self.tid_of(id)
            .map_or(HalfPoints::ZERO, |t| self.by_tid[t].points)
    }

    /// The number of tournament-number slots, so a caller can size a parallel
    /// per-number table. Every [`TournamentId`] `t` with `usize::from(t) <
    /// tid_capacity()` is a valid [`Self::get_tid`] argument.
    pub fn tid_capacity(&self) -> usize {
        self.by_tid.len()
    }
}

/// Replay the **completed** rounds to accumulate each player's points, opponents,
/// bye status and float history.
///
/// A float's direction is read from each board's frozen [`points_diff`] (the
/// float as it was at pairing time). Freezing is what keeps the float history
/// correct once MacMahon thresholds can change mid-tournament, or when an earlier
/// result is edited: the score table is recomputed live, but who floated in a
/// past round is a fact of how that round was actually paired.
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
    // Tournament number of each player — the dense key everything is stored by,
    // and what opponent/defeated lists hold. Assigned at finalization, so present
    // for every player whenever scoring runs (only once at least one round
    // exists). The score records accumulate straight into this number-indexed
    // vector, with no id-keyed intermediate: boards and players already carry the
    // number, so the only reverse (number → id) map kept is `ids`, below.
    let max_tid = players
        .iter()
        .map(|p| p.tournament_id.unwrap())
        .max()
        .unwrap_or(0) as usize;

    // Gaps (number 0, and numbers freed by a pre-play removal) keep a default
    // all-zero score and a nil id — never read, since only present players' numbers
    // appear as opponents. `real_tids` lists the present players' numbers for the
    // per-round cumulative pass, which must visit every real player but not a gap.
    let mut by_tid: TiVec<TournamentId, PlayerScore> = std::iter::repeat_with(PlayerScore::default)
        .take(max_tid + 1)
        .collect();
    let mut ids: TiVec<TournamentId, Uuid> = vec![Uuid::nil(); max_tid + 1].into();
    let mut real_tids: Vec<TournamentId> = Vec::with_capacity(players.len());
    for p in players {
        // The rating the ELO thresholds see: the rounded live estimate when
        // estimate-based MacMahon is active, else the static registration rating.
        // A player with no counted games sits at their prior mean, so every player
        // has an estimate.
        let mm_rating = match &estimated_elos {
            Some(est) => Some(
                est.get(&p.id)
                    .copied()
                    .unwrap_or(crate::elo::UNRATED_PRIOR_MEAN)
                    .round() as u32,
            ),
            None => p.rating,
        };
        let macmahon =
            HalfPoints::from_whole(settings.macmahon_points_at(mm_rating, p.grade, rounds_played));
        // Manual bonuses/maluses are folded in alongside MacMahon starting points,
        // before any round is replayed, so they shape both the standings and the
        // score-gap pairing weight from here on. They are whole-point deltas. The
        // signed floor at zero needs raw units, so drop into halves for this one
        // computation. The effective score can't go below zero.
        let adjustment: i32 = p.adjustments.iter().map(|a| a.delta).sum();
        let points =
            HalfPoints::from_halves((macmahon.halves() as i32 + adjustment * 2).max(0) as u32);
        let t = TournamentId(p.tournament_id.unwrap());
        ids[t] = p.id;
        real_tids.push(t);
        by_tid[t] = PlayerScore {
            points,
            macmahon,
            // A player faces one opponent per completed round (twice on a long
            // board), and can't defeat more than they face. Reserving up front
            // avoids the 1→2→4→8 regrowth as these fill in round by round.
            opponents: Vec::with_capacity(rounds_played as usize),
            defeated: Vec::with_capacity(rounds_played as usize),
            ..PlayerScore::default()
        };
    }

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
            // Boards carry tournament numbers, so the score table is indexed
            // directly — no Uuid→number lookup, and every board player is a current
            // player (a played player can't be removed), so its slot is real.
            let (ta, tb) = (TournamentId(board.player1), TournamentId(board.player2));
            // A long board (two rounds, two points) counts as *two* games against
            // the same opponent for the opponent-based tie-breaks (SOS, SODOS,
            // SOSOS, Buchholz), so it is recorded twice. It still feeds ELO as a
            // single game (that reads the boards directly, not these lists).
            let reps = if board.long { 2 } else { 1 };
            for _ in 0..reps {
                by_tid[ta].opponents.push(tb);
                by_tid[tb].opponents.push(ta);
            }

            // Direction from the frozen float, recorded at pairing time — for
            // every source, cup included: a bracket pairing that crosses score
            // groups floats its players just like a Swiss one, so it should curb
            // re-floating them in later rounds. (The board is already recorded as
            // a game faced above, so a later Swiss round won't re-pair them.)
            match board.points_diff.cmp(&0) {
                Ordering::Greater => {
                    // a had more points → a downfloats, b upfloats.
                    by_tid[ta].last_descended = Some(round.number);
                    by_tid[tb].last_ascended = Some(round.number);
                }
                Ordering::Less => {
                    by_tid[ta].last_ascended = Some(round.number);
                    by_tid[tb].last_descended = Some(round.number);
                }
                Ordering::Equal => {}
            }
        }
        if let Some(bye) = round.bye {
            let s = &mut by_tid[TournamentId(bye)];
            s.had_bye = true;
            s.last_descended = Some(round.number); // a bye is a downfloat
        }
        // A no-show has the same effect on the player who showed up as a bye: a
        // downfloat they can't be given twice.
        for board in &round.boards {
            if let Some(present) = board.no_show_opponent() {
                let s = &mut by_tid[TournamentId(present)];
                s.had_bye = true;
                s.last_descended = Some(round.number);
            }
        }
        // A cup bye (an unopposed bracket advance) is a bye all the same.
        for &player in &round.cup_byes {
            let s = &mut by_tid[TournamentId(player)];
            s.had_bye = true;
            s.last_descended = Some(round.number);
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
            let s = &mut by_tid[TournamentId(winner)];
            s.points += HalfPoints::from_whole(reps); // one point per game (two for a long board)
            s.victories += Wins(reps);
            for _ in 0..reps {
                s.defeated.push(TournamentId(loser));
            }
        }
        if let Some(bye) = round.bye {
            let s = &mut by_tid[TournamentId(bye)];
            s.points += HalfPoints::from_whole(1); // a bye scores one point
            s.victories += Wins(1);
        }
        // The player who showed up on a no-show board scores the free point, as
        // for a bye. The absentee scores nothing (like an absence).
        for board in &round.boards {
            if let Some(present) = board.no_show_opponent() {
                // A long board resolved by forfeit still scores its long weight
                // (two points), unless the referee demoted it.
                let reps = if board.long { 2 } else { 1 };
                let s = &mut by_tid[TournamentId(present)];
                s.points += HalfPoints::from_whole(reps); // one point (two for a long board)
                s.victories += Wins(reps);
            }
        }
        // A cup bye scores the free point too — the player advanced unopposed.
        for &player in &round.cup_byes {
            let s = &mut by_tid[TournamentId(player)];
            s.points += HalfPoints::from_whole(1); // an unopposed advance scores one point
            s.victories += Wins(1);
        }
        // A deliberate absence scores half a point when the referee enabled it
        // (a `0=` in the cross-table): +1 half-point, but not a win. `round.absent`
        // is the sat-out set (no board this round); a no-show forfeit is on a
        // board and stays at 0.
        if settings.half_point_absences {
            for &id in &round.absent {
                by_tid[TournamentId(id)].points += HalfPoints::from_halves(1); // half a point
            }
        }

        // Cumulative tie-break: after this round is scored, add every player's
        // running total to their running sum. A round the player sat out still
        // contributes their (unchanged) total, matching the classic "cumulative"
        // definition of a sum over the sequence of rounds. Visits the present
        // players (by their numbers), never the gap slots.
        for &t in &real_tids {
            let s = &mut by_tid[t];
            s.cuss_m += s.points;
            s.cuss_w += s.victories;
            s.running_points.push(s.points);
            s.running_wins.push(s.victories);
        }
    }

    Scores { by_tid, ids }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::{Board, CupStage, NoShow, PairingSource};
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
                ..Board::pending(
                    a.tournament_id.unwrap(),
                    b.tournament_id.unwrap(),
                    2,
                    PairingSource::Swiss,
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
        assert_eq!(scores.get(&a.id).last_descended, Some(1));
        assert_eq!(scores.get(&a.id).last_ascended, None);
        assert_eq!(scores.get(&b.id).last_ascended, Some(1));
        assert_eq!(scores.get(&b.id).last_descended, None);
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
                ..Board::pending(
                    a.tournament_id.unwrap(),
                    b.tournament_id.unwrap(),
                    0,
                    PairingSource::Swiss,
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
                ..Board::pending(
                    a.tournament_id.unwrap(),
                    b.tournament_id.unwrap(),
                    0,
                    PairingSource::Swiss,
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
            cup_byes: vec![a.tournament_id.unwrap()],
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
                ..Board::pending(
                    b.tournament_id.unwrap(),
                    c.tournament_id.unwrap(),
                    0,
                    PairingSource::Swiss,
                )
            }],
            bye: None,
            cup_byes: Vec::new(),
            absent: vec![a.tournament_id.unwrap()],
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
                ..Board::pending(
                    a.tournament_id.unwrap(),
                    b.tournament_id.unwrap(),
                    0,
                    PairingSource::Swiss,
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
        assert_eq!(scores.get(&a.id).points, 4); // 2 points = 4 half-points
        assert_eq!(scores.get(&a.id).victories, 2);
        let (atid, btid) = (a.tournament_id.unwrap(), b.tournament_id.unwrap());
        assert_eq!(scores.get(&a.id).defeated, vec![btid, btid]); // twice
        assert_eq!(scores.get(&a.id).opponents, vec![btid, btid]);
        assert_eq!(scores.get(&b.id).opponents, vec![atid, atid]);
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
                ..Board::pending(
                    a.tournament_id.unwrap(),
                    b.tournament_id.unwrap(),
                    0,
                    PairingSource::Swiss,
                )
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
        assert_eq!(
            scores.get(&a.id).opponents,
            vec![b.tournament_id.unwrap(), b.tournament_id.unwrap()]
        );
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
                ..Board::pending(
                    a.tournament_id.unwrap(),
                    b.tournament_id.unwrap(),
                    0,
                    PairingSource::Swiss,
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
    fn cup_boards_count_points_opponents_and_floats() {
        // A cup board with a big frozen points_diff (+5) is a heavy downfloat for
        // player1: a bracket pairing across score groups shapes float history just
        // like a Swiss one, so it should curb re-floating these players later.
        let a = player(1, None);
        let b = player(2, None);
        let round = Round {
            number: 1,
            boards: vec![Board {
                result: Some(Winner::Player1),
                ..Board::pending(
                    a.tournament_id.unwrap(),
                    b.tournament_id.unwrap(),
                    5,
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
        assert_eq!(scores.get(&a.id).opponents, vec![b.tournament_id.unwrap()]);
        assert_eq!(scores.get(&b.id).opponents, vec![a.tournament_id.unwrap()]);
        // ...and the +5 float is recorded: a downfloated, b upfloated.
        assert_eq!(scores.get(&a.id).last_descended, Some(1));
        assert_eq!(scores.get(&b.id).last_ascended, Some(1));
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
                    ..Board::pending(
                        a.tournament_id.unwrap(),
                        o.tournament_id.unwrap(),
                        0,
                        PairingSource::Swiss,
                    )
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
