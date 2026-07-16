//! Standings and tie-breaks.
//!
//! The ranked table shown on the Results tab — and, later, the ordering the
//! American grid (cross-table) is built from — is computed here so the server is
//! the single source of truth. It is derived from the **completed** rounds only.
//!
//! Each player's score is their **points** (MacMahon starting points plus
//! victories, counting the *effective* winner of a board — the handicap giver
//! always scores — and a bye as a win). The table is ranked by the
//! referee-chosen criteria (see [`TournamentSettings::tiebreaks`]) in order —
//! points is one of them, normally first — and finally by tournament number so
//! the order is always deterministic.
//!
//! Points plus every one of the twelve tie-break metrics is computed for every
//! player, so the Results tab can show whichever the referee selected without
//! recomputing. Each opponent-sum metric comes in a MacMahon-inclusive (`…M`)
//! and a wins-only (`…W`) flavour, according to how an opponent's "score" is
//! measured.
//!
//! # Relationship to [`crate::scoring`]
//!
//! A [`Standing`] is a **derived view**, not a second copy of the data. The raw
//! per-player accumulation — points, MacMahon start, victories, opponents faced
//! and beaten — is computed once by [`compute_scores`] (into the internal
//! `Scores` / `PlayerScore` substrate), and both this module *and* the pairing
//! engine build on that one result. [`compute_standings`] calls [`compute_scores`]
//! and then layers the twelve ranking tie-breaks and the live ELO estimate on
//! top; the pairing engine ([`crate::pairing`]) reads the same substrate directly.
//!
//! The substrate and this view are kept as separate types on purpose, rather
//! than folded into one:
//!
//! - **Disjoint extra fields.** `PlayerScore` carries pairing-only float history
//!   (`last_ascended` / `last_descended` / `had_bye`) that drives the pairing
//!   rules but is meaningless on a results page; [`Standing`] carries the twelve
//!   tie-break metrics and `estimated_elo`, which the engine never needs. Neither
//!   type should pay for the other's fields.
//! - **Different representation.** The substrate is indexed by dense tournament
//!   *number* and stores opponents as numbers, so the opponent-sum tie-breaks are
//!   plain array walks with no hashing — it runs repeatedly inside the matching
//!   solver. A [`Standing`] instead faces callers by [`Uuid`] and is a flat,
//!   ranked list, the shape a client consumes.
//! - **Dependency direction.** Pairing is the engine, standings is presentation.
//!   Having both depend on the lower-level `scoring` module keeps `pairing` from
//!   depending on `standings` (and from pulling in ranking/ELO logic it never
//!   uses).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::elo::estimate_elos;
use crate::player::Player;
use crate::round::Round;
use crate::scoring::compute_scores;
use crate::settings::{Tiebreak, TournamentSettings};
use crate::units::{HalfPoints, Wins};

/// One player's standing: score and every tie-break metric. The position in the
/// returned [`compute_standings`] vector is the player's rank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Standing {
    pub player_id: Uuid,
    /// Games won (effective winner; a bye counts as a win).
    pub victories: Wins,
    /// MacMahon starting points.
    pub macmahon: HalfPoints,
    /// Total score: `macmahon + 2·victories`, plus `1` per half-point absence.
    pub points: HalfPoints,
    /// Sum of opponents' points.
    pub sosm: HalfPoints,
    /// Sum of opponents' wins.
    pub sosw: Wins,
    /// Sum of defeated opponents' points.
    pub sodosm: HalfPoints,
    /// Sum of defeated opponents' wins.
    pub sodosw: Wins,
    /// Sum of opponents' SOSM.
    pub sososm: HalfPoints,
    /// Sum of opponents' SOSW.
    pub sososw: Wins,
    /// SOSM dropping the single lowest-scoring opponent.
    pub sosm1: HalfPoints,
    /// SOSM dropping the two lowest-scoring opponents.
    pub sosm2: HalfPoints,
    /// SOSW dropping the single lowest-scoring opponent.
    pub sosw1: Wins,
    /// SOSW dropping the two lowest-scoring opponents.
    pub sosw2: Wins,
    /// Cumulative sum of the running points total after each round.
    pub cussm: HalfPoints,
    /// Cumulative sum of the running win total after each round.
    pub cussw: Wins,
    /// Direct confrontation: this player's wins against the other players they
    /// were still tied with once every earlier configured criterion ran out
    /// (0 if that tied group never played a complete round-robin among
    /// themselves, or if `Dc` isn't configured, or wasn't reached).
    pub dc: Wins,
    /// Opponents faced, one entry per game, in round order (a rematch appears
    /// twice). Lets the Results tab show how the opponent-sum tie-breaks were
    /// built without re-deriving who played whom.
    pub opponents: Vec<Uuid>,
    /// Opponents defeated (effective winner), one entry per game — the subset of
    /// `opponents` the SODOS tie-breaks sum over.
    pub defeated: Vec<Uuid>,
    /// Running points total after each completed round (the sequence CUSSM sums).
    pub running_points: Vec<HalfPoints>,
    /// Running win total after each completed round (the sequence CUSSW sums).
    pub running_wins: Vec<Wins>,
    /// The player's current estimated ELO (rounded), from the Bayesian estimate
    /// that drives the experimental ELO pairing mode (see [`crate::estimate_elos`]).
    /// `None` outside ELO mode: the estimate is only shown (and only ranks) when
    /// [`TournamentSettings::elo_estimate_needed`], so in Swiss mode it isn't
    /// computed at all. When present, a player with no counted games sits at their
    /// prior mean (their registration rating, or 600 if unrated).
    pub estimated_elo: Option<i32>,
}

impl Standing {
    /// The value of a given ranking criterion for this player, as a plain
    /// ordinal `u32`. Ranking only ever compares a metric against the *same*
    /// metric, so the typed half-point / win distinction that the fields carry
    /// collapses to a bare "bigger is better" integer here (half-points in raw
    /// ×2 units, wins as whole counts — never compared across the two).
    pub fn tiebreak(&self, tb: Tiebreak) -> u32 {
        match tb {
            Tiebreak::Points => self.points.halves(),
            Tiebreak::SosM => self.sosm.halves(),
            Tiebreak::SosW => self.sosw.get(),
            Tiebreak::SodosM => self.sodosm.halves(),
            Tiebreak::SodosW => self.sodosw.get(),
            Tiebreak::SososM => self.sososm.halves(),
            Tiebreak::SososW => self.sososw.get(),
            Tiebreak::SosM1 => self.sosm1.halves(),
            Tiebreak::SosM2 => self.sosm2.halves(),
            Tiebreak::SosW1 => self.sosw1.get(),
            Tiebreak::SosW2 => self.sosw2.get(),
            Tiebreak::CussM => self.cussm.halves(),
            Tiebreak::CussW => self.cussw.get(),
            Tiebreak::Dc => self.dc.get(),
            // Ranking metrics are u32; a (non-negative) estimated ELO fits, and a
            // stray negative estimate floors at 0 for ordering. Only reached when
            // the estimate is maintained (see the `EstElo` skip in ranking), so it
            // is `Some`; a missing one sorts as 0.
            Tiebreak::EstElo => self.estimated_elo.unwrap_or(0).max(0) as u32,
        }
    }
}

/// Sum a list of opponent scores after dropping the `drop` lowest — the
/// Buchholz-cut family (`drop` = 0 is the plain sum). Generic over the score
/// unit so it serves both the half-point (`…M`) and win-count (`…W`) flavours.
fn sum_dropping_lowest<T: Ord + Copy + std::iter::Sum>(mut scores: Vec<T>, drop: usize) -> T {
    scores.sort_unstable();
    scores.into_iter().skip(drop).sum()
}

/// Compute the ranked standings from the completed rounds.
///
/// Byes and absences contribute no opponent to the tie-breaks. Rounds that are
/// not yet completed are ignored.
pub fn compute_standings(
    players: &[Player],
    settings: &TournamentSettings,
    rounds: &[Round],
) -> Vec<Standing> {
    // Points, opponents, wins and cumulative sums come from the shared scorer; the
    // opponent-based tie-breaks are then sums over each player's opponents, in two
    // flavours: `…M` scores an opponent by their points, `…W` by their wins.
    let scores = compute_scores(players, settings, rounds);
    // Opponent/defeated lists hold tournament numbers, so scoring an opponent is a
    // direct table lookup — `…M` by their points, `…W` by their wins.
    let score_m = |o: &u32| scores.get_tid(*o).points;
    let score_w = |o: &u32| scores.get_tid(*o).victories;

    // The live Bayesian ELO estimate — only shown, and only ranks, in ELO mode,
    // so skip the whole (quadrature-heavy) computation in Swiss mode.
    let elos = settings
        .elo_estimate_needed()
        .then(|| estimate_elos(players, settings, rounds));

    // SOSM and SOSW first — each needed on its own for the SOSOS metrics. Indexed
    // by tournament number so the SOSOS pass (a sum over opponents, which are
    // numbers) is a plain array walk.
    let cap = scores.tid_capacity();
    let mut sosm = vec![HalfPoints::ZERO; cap];
    let mut sosw = vec![Wins::ZERO; cap];
    for p in players {
        let t = scores
            .tid_of(&p.id)
            .expect("player has a number when scoring runs") as usize;
        let s = scores.get_tid(t as u32);
        sosm[t] = s.opponents.iter().map(score_m).sum();
        sosw[t] = s.opponents.iter().map(score_w).sum();
    }

    let mut standings: Vec<Standing> = players
        .iter()
        .map(|p| {
            let t = p.tournament_id.expect("player has a number") as usize;
            let s = scores.get_tid(t as u32);
            let opp_m: Vec<HalfPoints> = s.opponents.iter().map(&score_m).collect();
            let opp_w: Vec<Wins> = s.opponents.iter().map(&score_w).collect();
            Standing {
                player_id: p.id,
                victories: s.victories,
                macmahon: s.macmahon,
                points: s.points,
                sosm: sosm[t],
                sosw: sosw[t],
                sodosm: s.defeated.iter().map(&score_m).sum(),
                sodosw: s.defeated.iter().map(&score_w).sum(),
                sososm: s.opponents.iter().map(|&o| sosm[o as usize]).sum(),
                sososw: s.opponents.iter().map(|&o| sosw[o as usize]).sum(),
                sosm1: sum_dropping_lowest(opp_m.clone(), 1),
                sosm2: sum_dropping_lowest(opp_m, 2),
                sosw1: sum_dropping_lowest(opp_w.clone(), 1),
                sosw2: sum_dropping_lowest(opp_w, 2),
                cussm: s.cuss_m,
                cussw: s.cuss_w,
                // Filled in below, while the ranking order is computed, since it
                // depends on which players end up tied on the earlier criteria.
                dc: Wins::ZERO,
                // The output faces callers by id, so translate the numbers back.
                opponents: s.opponents.iter().map(|&o| scores.id_of(o)).collect(),
                defeated: s.defeated.iter().map(|&o| scores.id_of(o)).collect(),
                running_points: s.running_points.clone(),
                running_wins: s.running_wins.clone(),
                estimated_elo: elos
                    .as_ref()
                    .map(|e| e.get(&p.id).copied().unwrap_or(0.0).round() as i32),
            }
        })
        .collect();

    // Rank by each configured criterion in order (points is one of them, normally
    // first); the tournament number breaks any remaining tie so the order is
    // deterministic (unnumbered players last).
    //
    // Most criteria are a plain per-player number, so ranking by them is just a
    // lexicographic sort. Direct confrontation isn't: its value for a player
    // depends on which other players they're still tied with once every earlier
    // criterion has run out, so ranking proceeds by repeatedly splitting the
    // still-tied groups, criterion by criterion, rather than a single sort.
    let tnum: HashMap<Uuid, u32> = players
        .iter()
        .map(|p| (p.id, p.tournament_id.unwrap_or(u32::MAX)))
        .collect();

    let mut start: Vec<usize> = (0..standings.len()).collect();
    start.sort_by_key(|&i| tnum[&standings[i].player_id]);
    let mut groups: Vec<Vec<usize>> = vec![start];

    for &tb in &settings.tiebreaks {
        // Estimated ELO only ranks when a live estimate is maintained (either ELO
        // mode); otherwise the estimate is just the registration rating, so skip
        // it (defends against a loaded save that predates normalization dropping
        // it).
        if tb == Tiebreak::EstElo && !settings.elo_estimate_needed() {
            continue;
        }
        let mut next_groups: Vec<Vec<usize>> = Vec::new();
        for group in groups {
            if group.len() < 2 {
                next_groups.push(group);
                continue;
            }
            if tb == Tiebreak::Dc {
                match direct_confrontation(&group, &standings) {
                    Some(dc) => {
                        for &i in &group {
                            standings[i].dc = Wins(dc[&standings[i].player_id]);
                        }
                        next_groups.extend(split_by_key(group, |i| standings[i].dc.get()));
                    }
                    // Not a complete subgraph: DC stays 0 for everyone here, and
                    // the group stays tied going into the next criterion.
                    None => next_groups.push(group),
                }
                continue;
            }
            next_groups.extend(split_by_key(group, |i| standings[i].tiebreak(tb)));
        }
        groups = next_groups;
    }

    groups
        .into_iter()
        .flatten()
        .map(|i| standings[i].clone())
        .collect()
}

/// Stable-split a tied group into subgroups sharing the same (descending) key —
/// each subgroup is still internally ordered as it was in `group` (so an outer
/// tie-break already applied, like tournament number, survives untouched).
fn split_by_key(mut group: Vec<usize>, key: impl Fn(usize) -> u32) -> Vec<Vec<usize>> {
    group.sort_by_key(|&i| std::cmp::Reverse(key(i)));
    let mut result: Vec<Vec<usize>> = Vec::new();
    for i in group {
        match result.last_mut() {
            Some(last) if key(last[0]) == key(i) => last.push(i),
            _ => result.push(vec![i]),
        }
    }
    result
}

/// Direct confrontation scores for a tied `group`: each player's wins against
/// the others in the group. Only defined — `Some` — when the group is a
/// complete subgraph (every pair in it has played each other at least once);
/// otherwise `None`, meaning the criterion doesn't apply to this group.
fn direct_confrontation(group: &[usize], standings: &[Standing]) -> Option<HashMap<Uuid, u32>> {
    let ids: Vec<Uuid> = group.iter().map(|&i| standings[i].player_id).collect();
    for (pos, &i) in group.iter().enumerate() {
        for &other_id in &ids[(pos + 1)..] {
            if !standings[i].opponents.contains(&other_id) {
                return None;
            }
        }
    }
    let id_set: std::collections::HashSet<Uuid> = ids.iter().copied().collect();
    Some(
        group
            .iter()
            .map(|&i| {
                let wins = standings[i]
                    .defeated
                    .iter()
                    .filter(|d| id_set.contains(d))
                    .count() as u32;
                (standings[i].player_id, wins)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::{Board, PairingSource, Winner};
    use crate::settings::{MacMahonThreshold, ThresholdCriterion};

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

    fn board(a: u32, b: u32, winner: Winner) -> Board {
        Board {
            result: Some(winner),
            ..Board::pending(a, b, 0, PairingSource::Swiss)
        }
    }

    fn round(number: u32, boards: Vec<Board>) -> Round {
        Round {
            number,
            boards,
            bye: None,
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        }
    }

    #[test]
    fn long_board_opponent_counts_twice_in_sos_and_sodos() {
        // R1: A beats B on a long board; C beats D (normal). R2: B beats C.
        // B ends on 1 point (2 halves). A faced B on a long board, so B is
        // counted *twice* for A's opponent-sums: SOSM(A) = SODOSM(A) = 2·2 = 4,
        // not 2 as it would be for a single game.
        let a = player(1, None);
        let b = player(2, None);
        let c = player(3, None);
        let d = player(4, None);
        let long_ab = Board {
            result: Some(Winner::Player1),
            long: true,
            ..Board::pending(
                a.tournament_id.unwrap(),
                b.tournament_id.unwrap(),
                0,
                PairingSource::Swiss,
            )
        };
        let rounds = vec![
            round(
                1,
                vec![
                    long_ab,
                    board(
                        c.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
            round(
                2,
                vec![board(
                    b.tournament_id.unwrap(),
                    c.tournament_id.unwrap(),
                    Winner::Player1,
                )],
            ),
        ];
        let players = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        let standings = compute_standings(&players, &TournamentSettings::default(), &rounds);
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        // A: 2 points (4 halves) and 2 victories from the long win.
        assert_eq!((of(a.id).points.halves(), of(a.id).victories.get()), (4, 2));
        // B: beat C once (1 point = 2 halves).
        assert_eq!(of(b.id).points, 2);
        // B counted twice in A's opponent-sums.
        assert_eq!(of(a.id).sosm, 4);
        assert_eq!(of(a.id).sodosm, 4);
        assert_eq!(of(a.id).sosw, 2); // B has 1 win, counted twice
    }

    #[test]
    fn sos_breaks_a_points_tie() {
        // A>C, D>B (upset) in R1; A>D, B>C in R2. Final points A=2, B=1, D=1,
        // C=0. B and D tie on 1 point, but D faced the stronger field (A, B)
        // and B the weaker (D, C), so SOS ranks D above B despite B's lower
        // tournament number.
        let a = player(1, Some(2000));
        let b = player(2, Some(1900));
        let c = player(3, Some(1800));
        let d = player(4, Some(1700));
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // A beats C
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // D beats B
                ],
            ),
            round(
                2,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // A beats D
                    board(
                        b.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // B beats C
                ],
            ),
        ];
        let players = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        let standings = compute_standings(&players, &TournamentSettings::default(), &rounds);

        let order: Vec<Uuid> = standings.iter().map(|s| s.player_id).collect();
        assert_eq!(order, vec![a.id, d.id, b.id, c.id]);

        // Points and SOSM are in half-point units (×2).
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        assert_eq!((of(a.id).points.halves(), of(a.id).sosm.halves()), (4, 2));
        assert_eq!((of(d.id).points.halves(), of(d.id).sosm.halves()), (2, 6)); // faced A(4)+B(2)
        assert_eq!((of(b.id).points.halves(), of(b.id).sosm.halves()), (2, 2)); // faced D(2)+C(0)
        assert_eq!(of(c.id).points, 0);
    }

    #[test]
    fn macmahon_points_feed_into_scores_and_sos() {
        // One completed round, A beats B, with a 1500 threshold: A rated 2000
        // starts at 1 MacMahon point (now 2 after the win), B at 1.
        let a = player(1, Some(2000));
        let b = player(2, Some(1600));
        let rounds = vec![round(
            1,
            vec![board(
                a.tournament_id.unwrap(),
                b.tournament_id.unwrap(),
                Winner::Player1,
            )],
        )];
        let settings = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)],
            ..Default::default()
        };
        let standings = compute_standings(&[a.clone(), b.clone()], &settings, &rounds);

        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        // MacMahon and points are in half-point units (×2); victories are whole.
        assert_eq!(
            (
                of(a.id).macmahon.halves(),
                of(a.id).victories.get(),
                of(a.id).points.halves()
            ),
            (2, 1, 4)
        );
        assert_eq!(
            (
                of(b.id).macmahon.halves(),
                of(b.id).victories.get(),
                of(b.id).points.halves()
            ),
            (2, 0, 2)
        );
        assert_eq!(of(a.id).sosm, 2); // opponent B has 1 point (2 halves)
        assert_eq!(of(a.id).sodosm, 2); // defeated B (1 point = 2 halves)
        assert_eq!(standings[0].player_id, a.id); // A ranks first
    }

    #[test]
    fn degressive_macmahon_drops_the_head_start_after_the_scheduled_round() {
        // A (2000) starts one MacMahon point above B (1000) thanks to the 1500
        // threshold, but that group is dropped at the end of round 1. After A
        // beats B in round 1, A's starting bonus is gone: A keeps only the real
        // win, so A is on 1 point rather than 2.
        let a = player(1, Some(2000));
        let b = player(2, Some(1000));
        let rounds = vec![round(
            1,
            vec![board(
                a.tournament_id.unwrap(),
                b.tournament_id.unwrap(),
                Winner::Player1,
            )],
        )];
        let settings = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold {
                criterion: ThresholdCriterion::Elo { value: 1500 },
                drops_after_round: Some(1),
            }],
            ..Default::default()
        };
        let standings = compute_standings(&[a.clone(), b.clone()], &settings, &rounds);

        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        // Points in half-point units: A keeps only the win (2 halves), not the
        // dropped MacMahon head start.
        assert_eq!(
            (
                of(a.id).macmahon.halves(),
                of(a.id).victories.get(),
                of(a.id).points.halves()
            ),
            (0, 1, 2)
        );
        assert_eq!(
            (
                of(b.id).macmahon.halves(),
                of(b.id).victories.get(),
                of(b.id).points.halves()
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn wins_only_tiebreaks_ignore_macmahon_start() {
        // A 1500 threshold gives the higher-rated players a MacMahon head start,
        // so the M and W flavours of the opponent-sum tie-breaks diverge. After
        // A>B and A>C: A faced B and C. With the threshold, B and C each have 1
        // point (MacMahon) but 0 wins, so A's SOSM = 2 but SOSW = 0. A also
        // defeated both, so SODOSM = 2, SODOSW = 0.
        let a = player(1, Some(2000));
        let b = player(2, Some(1800));
        let c = player(3, Some(1700));
        let rounds = vec![
            round(
                1,
                vec![board(
                    a.tournament_id.unwrap(),
                    b.tournament_id.unwrap(),
                    Winner::Player1,
                )],
            ),
            round(
                2,
                vec![board(
                    a.tournament_id.unwrap(),
                    c.tournament_id.unwrap(),
                    Winner::Player1,
                )],
            ),
        ];
        let settings = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)],
            ..Default::default()
        };
        let standings = compute_standings(&[a.clone(), b.clone(), c.clone()], &settings, &rounds);
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        // SOSM/SODOSM are in half-point units (opponents' MacMahon = 2 halves);
        // the W flavours are whole win counts (0 here).
        assert_eq!((of(a.id).sosm.halves(), of(a.id).sosw.get()), (4, 0));
        assert_eq!((of(a.id).sodosm.halves(), of(a.id).sodosw.get()), (4, 0));
    }

    #[test]
    fn buchholz_cut_drops_the_lowest_opponents() {
        // A round robin: A wins everything (3 wins); B beats C and D (2 wins); C
        // beats D (1 win); D loses everything (0 wins). A's opponents' win scores
        // are therefore {2, 1, 0}.
        let a = player(1, None);
        let b = player(2, None);
        let c = player(3, None);
        let d = player(4, None);
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        b.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                    board(
                        c.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
            round(
                2,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
            round(
                3,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                    board(
                        b.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
        ];
        let standings = compute_standings(
            &[a.clone(), b.clone(), c.clone(), d.clone()],
            &TournamentSettings::default(),
            &rounds,
        );
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        // A faced B (2 wins), C (1 win), D (0 wins) → {2, 1, 0}.
        assert_eq!(of(a.id).sosw, 3);
        assert_eq!(of(a.id).sosw1, 3); // drop the 0
        assert_eq!(of(a.id).sosw2, 2); // drop the 0 and the 1
    }

    #[test]
    fn cumulative_sums_running_totals_over_rounds() {
        // A: win, loss, win → running wins 1, 1, 2 → CUSSW = 4. B mirrors: loss,
        // win, loss → running wins 0, 1, 1 → CUSSW = 2.
        let a = player(1, None);
        let b = player(2, None);
        let c = player(3, None);
        let d = player(4, None);
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        b.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                    board(
                        c.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
            round(
                2,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player2,
                    ),
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                ],
            ),
            round(
                3,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ),
                    board(
                        b.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player2,
                    ),
                ],
            ),
        ];
        let standings = compute_standings(
            &[a.clone(), b.clone(), c.clone(), d.clone()],
            &TournamentSettings::default(),
            &rounds,
        );
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        assert_eq!(of(a.id).cussw, 4); // 1 + 1 + 2 (whole wins)
        assert_eq!(of(b.id).cussw, 2); // 0 + 1 + 1
                                       // CUSSM sums points (half-units), so with no MacMahon it is twice CUSSW.
        assert_eq!(of(a.id).cussm, 8);
    }

    #[test]
    fn estimated_elo_seeds_at_rating_and_tracks_upsets() {
        // No games: each player's estimate is their registration rating. Then a
        // lower-rated player beats a higher one, and their estimate rises above
        // its seed while the loser's falls.
        let a = player(1, Some(1400));
        let b = player(2, Some(1800));

        // The estimate is only computed in ELO mode, so enable it here.
        let elo_mode = TournamentSettings {
            elo_pairing_enabled: true,
            ..Default::default()
        };
        let none = compute_standings(&[a.clone(), b.clone()], &elo_mode, &[]);
        let of = |st: &[Standing], id| {
            st.iter()
                .find(|s| s.player_id == id)
                .unwrap()
                .estimated_elo
                .expect("estimate present in ELO mode")
        };
        assert_eq!(of(&none, a.id), 1400);
        assert_eq!(of(&none, b.id), 1800);

        let rounds = vec![round(
            1,
            vec![board(
                a.tournament_id.unwrap(),
                b.tournament_id.unwrap(),
                Winner::Player1,
            )],
        )]; // A upsets B
        let after = compute_standings(&[a.clone(), b.clone()], &elo_mode, &rounds);
        assert!(of(&after, a.id) > 1400, "the winner's estimate rises");
        assert!(of(&after, b.id) < 1800, "the loser's estimate falls");
    }

    #[test]
    fn est_elo_tiebreak_is_ignored_without_elo_mode() {
        // Equal on everything but rating; no games, so estimates sit at the seeds.
        let a = player(1, Some(1400));
        let b = player(2, Some(1800));
        let players = vec![a.clone(), b.clone()];
        let pos = |st: &[Standing], id| st.iter().position(|s| s.player_id == id).unwrap();

        // Mode off: est_elo can't break the tie, so the tournament number does
        // (A before B) — the stronger estimate does *not* win.
        let off = compute_standings(
            &players,
            &TournamentSettings {
                tiebreaks: vec![Tiebreak::EstElo],
                elo_pairing_enabled: false,
                ..Default::default()
            },
            &[],
        );
        assert!(pos(&off, a.id) < pos(&off, b.id));

        // Mode on: est_elo ranks the stronger estimate first (B above A).
        let on = compute_standings(
            &players,
            &TournamentSettings {
                tiebreaks: vec![Tiebreak::EstElo],
                elo_pairing_enabled: true,
                ..Default::default()
            },
            &[],
        );
        assert!(pos(&on, b.id) < pos(&on, a.id));
    }

    #[test]
    fn configured_tiebreak_order_decides_the_ranking() {
        // With a 1500 threshold, A and B start on 1 MacMahon point. E and F both
        // finish on 1 point. E faced A (1 pt, 0 wins) and B (3 pts, 2 wins); F
        // faced C (2 pts, 2 wins) and D (1 pt, 1 win). So SOSM(E)=4 > SOSM(F)=3,
        // but SOSW(E)=2 < SOSW(F)=3 — the two flavours order E and F oppositely.
        let a = player(1, Some(2000));
        let b = player(2, Some(2000));
        let c = player(3, Some(1000));
        let d = player(4, Some(1000));
        let e = player(5, Some(1000));
        let f = player(6, Some(1000));
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        e.tournament_id.unwrap(),
                        a.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // E beats A
                    board(
                        f.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // C beats F
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // D beats B
                ],
            ),
            round(
                2,
                vec![
                    board(
                        e.tournament_id.unwrap(),
                        b.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // B beats E
                    board(
                        f.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // F beats D
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // C beats A
                ],
            ),
        ];
        let players = vec![
            a.clone(),
            b.clone(),
            c.clone(),
            d.clone(),
            e.clone(),
            f.clone(),
        ];
        let base = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)],
            ..Default::default()
        };
        let pos = |st: &[Standing], id| st.iter().position(|s| s.player_id == id).unwrap();

        let by_m = compute_standings(
            &players,
            &TournamentSettings {
                tiebreaks: vec![Tiebreak::SosM],
                ..base.clone()
            },
            &rounds,
        );
        assert!(pos(&by_m, e.id) < pos(&by_m, f.id)); // E above F by SOSM

        let by_w = compute_standings(
            &players,
            &TournamentSettings {
                tiebreaks: vec![Tiebreak::SosW],
                ..base
            },
            &rounds,
        );
        assert!(pos(&by_w, f.id) < pos(&by_w, e.id)); // F above E by SOSW
    }

    #[test]
    fn direct_confrontation_breaks_a_tie_within_a_complete_subgraph() {
        // A, B and C each finish on 2 points but by different means: A beats
        // both B and C; B beats C plus outsider D; C beats outsiders D and E.
        // A, B and C have played each other (a complete subgraph), so DC ranks
        // them by wins *within that trio*: A beat both (2), B beat only C (1),
        // C beat neither (0) — A > B > C, even though their tournament numbers
        // (3, 2, 1) say the opposite.
        let a = player(3, None);
        let b = player(2, None);
        let c = player(1, None);
        let d = player(4, None);
        let e = player(5, None);
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        b.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // A beats B
                    board(
                        c.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // C beats D
                ],
            ),
            round(
                2,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // A beats C
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // B beats D
                ],
            ),
            round(
                3,
                vec![board(
                    b.tournament_id.unwrap(),
                    c.tournament_id.unwrap(),
                    Winner::Player1,
                )],
            ), // B beats C
            round(
                4,
                vec![board(
                    c.tournament_id.unwrap(),
                    e.tournament_id.unwrap(),
                    Winner::Player1,
                )],
            ), // C beats E
        ];
        let players = vec![a.clone(), b.clone(), c.clone(), d.clone(), e.clone()];
        let settings = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::Dc],
            ..Default::default()
        };
        let standings = compute_standings(&players, &settings, &rounds);
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();

        // Points in half-point units (2 points = 4 halves); DC is a whole count.
        assert_eq!(
            (
                of(a.id).points.halves(),
                of(b.id).points.halves(),
                of(c.id).points.halves()
            ),
            (4, 4, 4)
        );
        assert_eq!(
            (of(a.id).dc.get(), of(b.id).dc.get(), of(c.id).dc.get()),
            (2, 1, 0)
        );

        let pos = |id| standings.iter().position(|s| s.player_id == id).unwrap();
        assert!(pos(a.id) < pos(b.id));
        assert!(pos(b.id) < pos(c.id));
    }

    #[test]
    fn direct_confrontation_does_nothing_without_a_complete_subgraph() {
        // A, B, C and D all finish on 1 point, but they haven't all played each
        // other: A faced C and D, B faced C and D, but A never faced B and C
        // never faced D. DC can't apply to a tied group that isn't a complete
        // subgraph, so it stays 0 for everyone and the tournament number alone
        // breaks the tie.
        let a = player(1, None);
        let b = player(2, None);
        let c = player(3, None);
        let d = player(4, None);
        let rounds = vec![
            round(
                1,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // A beats C
                    board(
                        b.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player1,
                    ), // B beats D
                ],
            ),
            round(
                2,
                vec![
                    board(
                        a.tournament_id.unwrap(),
                        d.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // D beats A
                    board(
                        b.tournament_id.unwrap(),
                        c.tournament_id.unwrap(),
                        Winner::Player2,
                    ), // C beats B
                ],
            ),
        ];
        let players = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        let settings = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::Dc],
            ..Default::default()
        };
        let standings = compute_standings(&players, &settings, &rounds);
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();

        for id in [a.id, b.id, c.id, d.id] {
            assert_eq!(of(id).points, 2); // 1 point = 2 half-points
            assert_eq!(of(id).dc, 0);
        }
        let order: Vec<Uuid> = standings.iter().map(|s| s.player_id).collect();
        assert_eq!(order, vec![a.id, b.id, c.id, d.id]); // tournament number order
    }
}
