//! The direct-elimination cup that runs alongside the Swiss in a hybrid
//! tournament (the French / European Shogi Championship format).
//!
//! The top `size` eligible players (a power of two, 8..64) are seeded into a
//! single-elimination bracket that occupies the first `log2(size)` rounds. Only
//! the immutable `{size, seed_order}` is stored; the state of the bracket in any
//! given round is **derived** by replaying the recorded results, so editing a
//! past result or cancelling a round re-derives correctly (same philosophy as the
//! live-recomputed scores).
//!
//! Bracket shape: round 1 folds the seeds `(1,size), (2,size-1), …`; each later
//! round folds the previous round's winners *in match order*, so seeds stay
//! separated exactly as in a standard seeded bracket. The semifinal's two losers
//! meet in a **small final** for third place, played in the same (final) round as
//! the final itself; after that round everyone — champion included — is in the
//! Swiss pool.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::round::{CupStage, PairingSource, Round};

/// The valid cup sizes (top-N), each a power of two.
pub const CUP_SIZES: [u32; 4] = [8, 16, 32, 64];

/// A hybrid tournament's cup, fixed at finalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Cup {
    /// Bracket size (8/16/32/64).
    pub size: u32,
    /// The seeded players, seed 1..size (index 0 = top seed). Frozen at finalize.
    pub seed_order: Vec<Uuid>,
}

/// One bracket pairing for a round, with the stage it belongs to.
#[derive(Debug, Clone)]
pub struct CupMatch {
    pub player1: Uuid,
    pub player2: Uuid,
    pub stage: CupStage,
}

/// The cup podium, once the final round is decided.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct CupPodium {
    pub champion: Uuid,
    pub runner_up: Uuid,
    pub third: Uuid,
    pub fourth: Uuid,
}

impl Cup {
    /// Number of tournament rounds the cup spans (`log2(size)`).
    pub fn cup_rounds(&self) -> u32 {
        self.size.trailing_zeros()
    }

    /// Whether tournament round `r` (1-based) is part of the cup.
    pub fn is_cup_round(&self, r: u32) -> bool {
        r >= 1 && r <= self.cup_rounds()
    }

    /// The bracket matches for cup round `r`, reading earlier results from
    /// `rounds`. `None` if a needed earlier cup result is missing (shouldn't
    /// happen for a properly gated round). Empty when `r` is past the cup.
    pub fn matches_for_round(&self, rounds: &[Round], r: u32) -> Option<Vec<CupMatch>> {
        if !self.is_cup_round(r) {
            return Some(Vec::new());
        }
        let cup_rounds = self.cup_rounds();
        let mut frontier = self.seed_order.clone();
        let mut semifinal_losers: Option<[Uuid; 2]> = None;
        for k in 1..r {
            let (winners, losers) = self.play_round(rounds, k, &frontier)?;
            if k == cup_rounds - 1 {
                semifinal_losers = Some([losers[0], losers[1]]);
            }
            frontier = winners;
        }

        let matches = if r < cup_rounds {
            let stage = stage_before_final(frontier.len() as u32);
            fold(&frontier)
                .into_iter()
                .map(|(player1, player2)| CupMatch { player1, player2, stage })
                .collect()
        } else {
            // The final round: the final, then the small final for third place.
            let losers = semifinal_losers.expect("semifinal replayed before the final round");
            let mut v = Vec::new();
            for (player1, player2) in fold(&frontier) {
                v.push(CupMatch { player1, player2, stage: CupStage::Final });
            }
            for (player1, player2) in fold(&losers) {
                v.push(CupMatch { player1, player2, stage: CupStage::SmallFinal });
            }
            v
        };
        Some(matches)
    }

    /// The podium, if the final round's boards are decided; otherwise `None`.
    pub fn podium(&self, rounds: &[Round]) -> Option<CupPodium> {
        let final_round = rounds.iter().find(|r| r.number == self.cup_rounds())?;
        let stage_board = |want: CupStage| {
            final_round
                .boards
                .iter()
                .find(|b| matches!(b.source, PairingSource::Cup { stage } if stage == want))
        };
        let final_board = stage_board(CupStage::Final)?;
        let small = stage_board(CupStage::SmallFinal)?;
        Some(CupPodium {
            champion: final_board.winner_id()?,
            runner_up: final_board.effective_loser()?,
            third: small.winner_id()?,
            fourth: small.effective_loser()?,
        })
    }

    /// Replay one cup round: fold the frontier, look up each match's result, and
    /// return the winners and losers in match order.
    fn play_round(
        &self,
        rounds: &[Round],
        k: u32,
        frontier: &[Uuid],
    ) -> Option<(Vec<Uuid>, Vec<Uuid>)> {
        let mut winners = Vec::new();
        let mut losers = Vec::new();
        for (a, b) in fold(frontier) {
            let (w, l) = decide(rounds, k, a, b)?;
            winners.push(w);
            losers.push(l);
        }
        Some((winners, losers))
    }
}

/// Pair the first with the last, second with second-last, … — the bracket fold.
fn fold(list: &[Uuid]) -> Vec<(Uuid, Uuid)> {
    let n = list.len();
    (0..n / 2).map(|i| (list[i], list[n - 1 - i])).collect()
}

/// The stage name for a pre-final round given how many players are still alive.
fn stage_before_final(alive: u32) -> CupStage {
    match alive {
        8 => CupStage::Quarterfinal,
        4 => CupStage::Semifinal,
        n => CupStage::RoundOf(n),
    }
}

/// Find the board between `a` and `b` in round `k` and return (winner, loser).
fn decide(rounds: &[Round], k: u32, a: Uuid, b: Uuid) -> Option<(Uuid, Uuid)> {
    let round = rounds.iter().find(|r| r.number == k)?;
    let board = round.boards.iter().find(|bd| {
        (bd.player1 == a && bd.player2 == b) || (bd.player1 == b && bd.player2 == a)
    })?;
    Some((board.winner_id()?, board.effective_loser()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::{Board, Winner};

    fn ids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    /// Build a completed round whose cup boards are the given (winner, loser) pairs.
    fn cup_round(number: u32, results: &[(Uuid, Uuid, CupStage)]) -> Round {
        Round {
            number,
            boards: results
                .iter()
                .map(|&(w, l, stage)| Board {
                    result: Some(Winner::Player1), // player1 = winner
                    source: PairingSource::Cup { stage },
                    ..Board::pending(w, l, None, PairingSource::Swiss)
                })
                .collect(),
            bye: None,
            absent: Vec::new(),
            completed: true,
        }
    }

    #[test]
    fn round_one_folds_the_seeds() {
        let seeds = ids(8);
        let cup = Cup { size: 8, seed_order: seeds.clone() };
        let m = cup.matches_for_round(&[], 1).unwrap();
        // 1v8, 2v7, 3v6, 4v5 — all quarterfinals.
        assert_eq!(m.len(), 4);
        assert_eq!((m[0].player1, m[0].player2), (seeds[0], seeds[7]));
        assert_eq!((m[1].player1, m[1].player2), (seeds[1], seeds[6]));
        assert_eq!((m[3].player1, m[3].player2), (seeds[3], seeds[4]));
        assert!(matches!(m[0].stage, CupStage::Quarterfinal));
    }

    #[test]
    fn semifinal_winners_meet_in_the_final_and_losers_in_the_small_final() {
        let s = ids(8);
        let cup = Cup { size: 8, seed_order: s.clone() };
        // R1 (QF): top seed of each match wins → winners s0,s1,s2,s3 in match order.
        let r1 = cup_round(
            1,
            &[
                (s[0], s[7], CupStage::Quarterfinal),
                (s[1], s[6], CupStage::Quarterfinal),
                (s[2], s[5], CupStage::Quarterfinal),
                (s[3], s[4], CupStage::Quarterfinal),
            ],
        );
        // R2 (SF): fold [s0,s1,s2,s3] → (s0,s3) and (s1,s2). Say s0 and s1 win.
        let sf = cup.matches_for_round(&[r1.clone()], 2).unwrap();
        assert_eq!((sf[0].player1, sf[0].player2), (s[0], s[3]));
        assert_eq!((sf[1].player1, sf[1].player2), (s[1], s[2]));
        assert!(matches!(sf[0].stage, CupStage::Semifinal));
        let r2 = cup_round(
            2,
            &[(s[0], s[3], CupStage::Semifinal), (s[1], s[2], CupStage::Semifinal)],
        );
        // R3 (final round): final s0 vs s1, small final s3 vs s2.
        let f = cup.matches_for_round(&[r1, r2], 3).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].player1, f[0].player2), (s[0], s[1]));
        assert!(matches!(f[0].stage, CupStage::Final));
        assert_eq!((f[1].player1, f[1].player2), (s[3], s[2]));
        assert!(matches!(f[1].stage, CupStage::SmallFinal));
    }

    #[test]
    fn stage_names_scale_with_size() {
        let cup = Cup { size: 32, seed_order: ids(32) };
        assert_eq!(cup.cup_rounds(), 5);
        assert!(matches!(cup.matches_for_round(&[], 1).unwrap()[0].stage, CupStage::RoundOf(32)));
    }
}
