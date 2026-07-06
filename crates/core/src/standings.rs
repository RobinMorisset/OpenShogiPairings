//! Standings and tie-breaks.
//!
//! The ranked table shown on the Results tab — and, later, the ordering the
//! American grid (cross-table) is built from — is computed here so the server is
//! the single source of truth. It is derived from the **completed** rounds only.
//!
//! Each player's score is their **points** (MacMahon starting points plus
//! victories, counting the *effective* winner of a board — the handicap giver
//! always scores — and a bye as a win). Ties are broken, in order, by:
//!
//! 1. **SOS** — sum of opponents' points.
//! 2. **SODOS** — sum of defeated opponents' points.
//! 3. **SOSOS** — sum of opponents' SOS.
//!
//! and finally by tournament number, so the order is always deterministic.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::player::Player;
use crate::round::{Round, Winner};
use crate::settings::TournamentSettings;

/// One player's standing: score and tie-breaks. The position in the returned
/// [`compute_standings`] vector is the player's rank.
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
    pub sos: u32,
    /// Sum of defeated opponents' points.
    pub sodos: u32,
    /// Sum of opponents' SOS.
    pub sosos: u32,
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
    // Points start at each player's MacMahon points, then gain a point per
    // effective win (and per bye). Opponents faced / defeated are collected for
    // the tie-breaks.
    let mut points: HashMap<Uuid, u32> = players
        .iter()
        .map(|p| (p.id, settings.macmahon_points(p.rating)))
        .collect();
    let mut victories: HashMap<Uuid, u32> = players.iter().map(|p| (p.id, 0)).collect();
    let mut faced: HashMap<Uuid, Vec<Uuid>> = players.iter().map(|p| (p.id, Vec::new())).collect();
    let mut defeated: HashMap<Uuid, Vec<Uuid>> =
        players.iter().map(|p| (p.id, Vec::new())).collect();

    for round in rounds.iter().filter(|r| r.completed) {
        for board in &round.boards {
            if let Some(v) = faced.get_mut(&board.player1) {
                v.push(board.player2);
            }
            if let Some(v) = faced.get_mut(&board.player2) {
                v.push(board.player1);
            }
            let (winner_id, loser_id) = match board.effective_winner() {
                Some(Winner::Player1) => (board.player1, board.player2),
                Some(Winner::Player2) => (board.player2, board.player1),
                None => continue,
            };
            if let Some(p) = points.get_mut(&winner_id) {
                *p += 1;
            }
            if let Some(v) = victories.get_mut(&winner_id) {
                *v += 1;
            }
            if let Some(d) = defeated.get_mut(&winner_id) {
                d.push(loser_id);
            }
        }
        if let Some(bye) = round.bye {
            if let Some(p) = points.get_mut(&bye) {
                *p += 1;
            }
            if let Some(v) = victories.get_mut(&bye) {
                *v += 1;
            }
        }
    }

    let point_sum = |ids: &[Uuid]| ids.iter().map(|o| points.get(o).copied().unwrap_or(0)).sum();

    // SOS first (needed on its own for each player's SOSOS).
    let sos: HashMap<Uuid, u32> = faced
        .iter()
        .map(|(id, opps)| (*id, point_sum(opps)))
        .collect();
    let sos_sum = |ids: &[Uuid]| ids.iter().map(|o| sos.get(o).copied().unwrap_or(0)).sum();

    let mut standings: Vec<Standing> = players
        .iter()
        .map(|p| Standing {
            player_id: p.id,
            victories: victories[&p.id],
            macmahon: settings.macmahon_points(p.rating),
            points: points[&p.id],
            sos: sos[&p.id],
            sodos: point_sum(&defeated[&p.id]),
            sosos: sos_sum(&faced[&p.id]),
        })
        .collect();

    // Rank by points, then SOS, SODOS, SOSOS; tournament number breaks any
    // remaining tie so the order is deterministic (unnumbered players last).
    let tnum: HashMap<Uuid, u32> = players
        .iter()
        .map(|p| (p.id, p.tournament_id.unwrap_or(u32::MAX)))
        .collect();
    standings.sort_by(|a, b| {
        b.points
            .cmp(&a.points)
            .then(b.sos.cmp(&a.sos))
            .then(b.sodos.cmp(&a.sodos))
            .then(b.sosos.cmp(&a.sosos))
            .then(tnum[&a.player_id].cmp(&tnum[&b.player_id]))
    });
    standings
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

    fn board(a: Uuid, b: Uuid, winner: Winner) -> Board {
        Board {
            player1: a,
            player2: b,
            result: Some(winner),
            drawn: false,
            handicap: None,
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
        assert_eq!((of(a.id).points, of(a.id).sos), (2, 1));
        assert_eq!((of(d.id).points, of(d.id).sos), (1, 3)); // faced A(2)+B(1)
        assert_eq!((of(b.id).points, of(b.id).sos), (1, 1)); // faced D(1)+C(0)
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
        };
        let standings = compute_standings(&[a.clone(), b.clone()], &settings, &rounds);

        let of = |id| standings.iter().find(|s| s.player_id == id).unwrap();
        assert_eq!((of(a.id).macmahon, of(a.id).victories, of(a.id).points), (1, 1, 2));
        assert_eq!((of(b.id).macmahon, of(b.id).victories, of(b.id).points), (1, 0, 1));
        assert_eq!(of(a.id).sos, 1); // opponent B has 1 point
        assert_eq!(of(a.id).sodos, 1); // defeated B (1 point)
        assert_eq!(standings[0].player_id, a.id); // A ranks first
    }
}
