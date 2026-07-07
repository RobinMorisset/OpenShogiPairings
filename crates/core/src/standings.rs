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

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::player::Player;
use crate::round::Round;
use crate::scoring::compute_scores;
use crate::settings::{Tiebreak, TournamentSettings};

/// One player's standing: score and every tie-break metric. The position in the
/// returned [`compute_standings`] vector is the player's rank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    pub player_id: Uuid,
    /// Games won (effective winner; a bye counts as a win).
    pub victories: u32,
    /// MacMahon starting points.
    pub macmahon: u32,
    /// Total score: `macmahon + victories`.
    pub points: u32,
    /// Sum of opponents' points.
    pub sosm: u32,
    /// Sum of opponents' wins.
    pub sosw: u32,
    /// Sum of defeated opponents' points.
    pub sodosm: u32,
    /// Sum of defeated opponents' wins.
    pub sodosw: u32,
    /// Sum of opponents' SOSM.
    pub sososm: u32,
    /// Sum of opponents' SOSW.
    pub sososw: u32,
    /// SOSM dropping the single lowest-scoring opponent.
    pub sosm1: u32,
    /// SOSM dropping the two lowest-scoring opponents.
    pub sosm2: u32,
    /// SOSW dropping the single lowest-scoring opponent.
    pub sosw1: u32,
    /// SOSW dropping the two lowest-scoring opponents.
    pub sosw2: u32,
    /// Cumulative sum of the running points total after each round.
    pub cussm: u32,
    /// Cumulative sum of the running win total after each round.
    pub cussw: u32,
}

impl Standing {
    /// The value of a given ranking criterion for this player.
    pub fn tiebreak(&self, tb: Tiebreak) -> u32 {
        match tb {
            Tiebreak::Points => self.points,
            Tiebreak::SosM => self.sosm,
            Tiebreak::SosW => self.sosw,
            Tiebreak::SodosM => self.sodosm,
            Tiebreak::SodosW => self.sodosw,
            Tiebreak::SososM => self.sososm,
            Tiebreak::SososW => self.sososw,
            Tiebreak::SosM1 => self.sosm1,
            Tiebreak::SosM2 => self.sosm2,
            Tiebreak::SosW1 => self.sosw1,
            Tiebreak::SosW2 => self.sosw2,
            Tiebreak::CussM => self.cussm,
            Tiebreak::CussW => self.cussw,
        }
    }
}

/// Sum a list of opponent scores after dropping the `drop` lowest — the
/// Buchholz-cut family (`drop` = 0 is the plain sum).
fn sum_dropping_lowest(mut scores: Vec<u32>, drop: usize) -> u32 {
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
    let score_m = |o: &Uuid| scores.points(o);
    let score_w = |o: &Uuid| scores.victories(o);

    // SOSM and SOSW first — each needed on its own for the SOSOS metrics.
    let sosm: HashMap<Uuid, u32> = players
        .iter()
        .map(|p| (p.id, scores.get(&p.id).opponents.iter().map(score_m).sum()))
        .collect();
    let sosw: HashMap<Uuid, u32> = players
        .iter()
        .map(|p| (p.id, scores.get(&p.id).opponents.iter().map(score_w).sum()))
        .collect();

    let mut standings: Vec<Standing> = players
        .iter()
        .map(|p| {
            let s = scores.get(&p.id);
            let opp_m: Vec<u32> = s.opponents.iter().map(&score_m).collect();
            let opp_w: Vec<u32> = s.opponents.iter().map(&score_w).collect();
            Standing {
                player_id: p.id,
                victories: s.victories,
                macmahon: s.macmahon,
                points: s.points,
                sosm: sosm[&p.id],
                sosw: sosw[&p.id],
                sodosm: s.defeated.iter().map(&score_m).sum(),
                sodosw: s.defeated.iter().map(&score_w).sum(),
                sososm: s.opponents.iter().map(|o| sosm.get(o).copied().unwrap_or(0)).sum(),
                sososw: s.opponents.iter().map(|o| sosw.get(o).copied().unwrap_or(0)).sum(),
                sosm1: sum_dropping_lowest(opp_m.clone(), 1),
                sosm2: sum_dropping_lowest(opp_m, 2),
                sosw1: sum_dropping_lowest(opp_w.clone(), 1),
                sosw2: sum_dropping_lowest(opp_w, 2),
                cussm: s.cuss_m,
                cussw: s.cuss_w,
            }
        })
        .collect();

    // Rank by each configured criterion in order (points is one of them, normally
    // first); the tournament number breaks any remaining tie so the order is
    // deterministic (unnumbered players last).
    let tnum: HashMap<Uuid, u32> = players
        .iter()
        .map(|p| (p.id, p.tournament_id.unwrap_or(u32::MAX)))
        .collect();
    standings.sort_by(|a, b| {
        let mut ord = std::cmp::Ordering::Equal;
        for &tb in &settings.tiebreaks {
            ord = ord.then_with(|| b.tiebreak(tb).cmp(&a.tiebreak(tb)));
        }
        ord.then(tnum[&a.player_id].cmp(&tnum[&b.player_id]))
    });
    standings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::{Board, PairingSource, Winner};

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

    fn board(a: Uuid, b: Uuid, winner: Winner) -> Board {
        Board {
            result: Some(winner),
            ..Board::pending(a, b, None, PairingSource::Swiss)
        }
    }

    fn round(number: u32, boards: Vec<Board>) -> Round {
        Round {
            number,
            boards,
            bye: None,
            absent: Vec::new(),
            completed: true,
        }
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
                    board(a.id, c.id, Winner::Player1), // A beats C
                    board(b.id, d.id, Winner::Player2), // D beats B
                ],
            ),
            round(
                2,
                vec![
                    board(a.id, d.id, Winner::Player1), // A beats D
                    board(b.id, c.id, Winner::Player1), // B beats C
                ],
            ),
        ];
        let players = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        let standings = compute_standings(&players, &TournamentSettings::default(), &rounds);

        let order: Vec<Uuid> = standings.iter().map(|s| s.player_id).collect();
        assert_eq!(order, vec![a.id, d.id, b.id, c.id]);

        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        assert_eq!((of(a.id).points, of(a.id).sosm), (2, 1));
        assert_eq!((of(d.id).points, of(d.id).sosm), (1, 3)); // faced A(2)+B(1)
        assert_eq!((of(b.id).points, of(b.id).sosm), (1, 1)); // faced D(1)+C(0)
        assert_eq!(of(c.id).points, 0);
    }

    #[test]
    fn macmahon_points_feed_into_scores_and_sos() {
        // One completed round, A beats B, with a 1500 threshold: A rated 2000
        // starts at 1 MacMahon point (now 2 after the win), B at 1.
        let a = player(1, Some(2000));
        let b = player(2, Some(1600));
        let rounds = vec![round(1, vec![board(a.id, b.id, Winner::Player1)])];
        let settings = TournamentSettings {
            macmahon_thresholds: vec![1500],
            ..Default::default()
        };
        let standings = compute_standings(&[a.clone(), b.clone()], &settings, &rounds);

        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        assert_eq!((of(a.id).macmahon, of(a.id).victories, of(a.id).points), (1, 1, 2));
        assert_eq!((of(b.id).macmahon, of(b.id).victories, of(b.id).points), (1, 0, 1));
        assert_eq!(of(a.id).sosm, 1); // opponent B has 1 point
        assert_eq!(of(a.id).sodosm, 1); // defeated B (1 point)
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
        let rounds = vec![round(1, vec![board(a.id, b.id, Winner::Player1)])];
        let settings = TournamentSettings {
            macmahon_thresholds: vec![1500],
            macmahon_removals: vec![1],
            ..Default::default()
        };
        let standings = compute_standings(&[a.clone(), b.clone()], &settings, &rounds);

        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        assert_eq!((of(a.id).macmahon, of(a.id).victories, of(a.id).points), (0, 1, 1));
        assert_eq!((of(b.id).macmahon, of(b.id).victories, of(b.id).points), (0, 0, 0));
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
            round(1, vec![board(a.id, b.id, Winner::Player1)]),
            round(2, vec![board(a.id, c.id, Winner::Player1)]),
        ];
        let settings = TournamentSettings {
            macmahon_thresholds: vec![1500],
            ..Default::default()
        };
        let standings = compute_standings(&[a.clone(), b.clone(), c.clone()], &settings, &rounds);
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        assert_eq!((of(a.id).sosm, of(a.id).sosw), (2, 0));
        assert_eq!((of(a.id).sodosm, of(a.id).sodosw), (2, 0));
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
            round(1, vec![board(a.id, b.id, Winner::Player1), board(c.id, d.id, Winner::Player1)]),
            round(2, vec![board(a.id, c.id, Winner::Player1), board(b.id, d.id, Winner::Player1)]),
            round(3, vec![board(a.id, d.id, Winner::Player1), board(b.id, c.id, Winner::Player1)]),
        ];
        let standings =
            compute_standings(&[a.clone(), b.clone(), c.clone(), d.clone()], &TournamentSettings::default(), &rounds);
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
            round(1, vec![board(a.id, b.id, Winner::Player1), board(c.id, d.id, Winner::Player1)]),
            round(2, vec![board(a.id, c.id, Winner::Player2), board(b.id, d.id, Winner::Player1)]),
            round(3, vec![board(a.id, d.id, Winner::Player1), board(b.id, c.id, Winner::Player2)]),
        ];
        let standings =
            compute_standings(&[a.clone(), b.clone(), c.clone(), d.clone()], &TournamentSettings::default(), &rounds);
        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        assert_eq!(of(a.id).cussw, 4); // 1 + 1 + 2
        assert_eq!(of(b.id).cussw, 2); // 0 + 1 + 1
        // No MacMahon here, so the M flavour matches the W flavour.
        assert_eq!(of(a.id).cussm, 4);
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
                    board(e.id, a.id, Winner::Player1), // E beats A
                    board(f.id, c.id, Winner::Player2), // C beats F
                    board(b.id, d.id, Winner::Player2), // D beats B
                ],
            ),
            round(
                2,
                vec![
                    board(e.id, b.id, Winner::Player2), // B beats E
                    board(f.id, d.id, Winner::Player1), // F beats D
                    board(a.id, c.id, Winner::Player2), // C beats A
                ],
            ),
        ];
        let players = vec![a.clone(), b.clone(), c.clone(), d.clone(), e.clone(), f.clone()];
        let base = TournamentSettings {
            macmahon_thresholds: vec![1500],
            ..Default::default()
        };
        let pos = |st: &[Standing], id| st.iter().position(|s| s.player_id == id).unwrap();

        let by_m = compute_standings(
            &players,
            &TournamentSettings { tiebreaks: vec![Tiebreak::SosM], ..base.clone() },
            &rounds,
        );
        assert!(pos(&by_m, e.id) < pos(&by_m, f.id)); // E above F by SOSM

        let by_w = compute_standings(
            &players,
            &TournamentSettings { tiebreaks: vec![Tiebreak::SosW], ..base },
            &rounds,
        );
        assert!(pos(&by_w, f.id) < pos(&by_w, e.id)); // F above E by SOSW
    }
}
