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

use crate::round::{CupStage, NoShow, Round};

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

/// The cup pairings for one round: the two-player bracket matches, plus any
/// players who advance unopposed (a cup bye — see [`Round::cup_byes`]).
#[derive(Debug, Clone, Default)]
pub struct CupPairings {
    pub matches: Vec<CupMatch>,
    pub byes: Vec<Uuid>,
}

/// The cup podium, once the final round is decided. Each place is `None` when it
/// couldn't be determined — e.g. both players of the final (or the small final)
/// were no-shows, so there is no champion / third to award.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct CupPodium {
    pub champion: Option<Uuid>,
    pub runner_up: Option<Uuid>,
    pub third: Option<Uuid>,
    pub fourth: Option<Uuid>,
}

/// The outcome of one decided cup match: a winner (a played result, or a single
/// no-show forfeit), or both players absent (a double no-show), which advances
/// nobody.
enum CupResult {
    Winner { winner: Uuid, loser: Uuid },
    BothAbsent,
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

    /// The bracket pairings for cup round `r`, reading earlier results from
    /// `rounds`: the two-player matches plus any unopposed advances (cup byes).
    /// `None` if a needed earlier cup result is missing (shouldn't happen for a
    /// properly gated round). Empty when `r` is past the cup.
    ///
    /// A slot goes empty when both players of the match that would fill it were
    /// no-shows: that match advances nobody, so whoever was drawn to face its
    /// winner advances unopposed as a bye.
    pub fn matches_for_round(&self, rounds: &[Round], r: u32) -> Option<CupPairings> {
        if !self.is_cup_round(r) {
            return Some(CupPairings::default());
        }
        let cup_rounds = self.cup_rounds();
        let (frontier, semifinal_losers) = self.replay_to(rounds, r)?;

        let mut pairings = CupPairings::default();
        if r < cup_rounds {
            let stage = stage_before_final(frontier.len() as u32);
            for pair in fold(&frontier) {
                push_pairing(pair, stage, &mut pairings);
            }
        } else {
            // The final round: the final, then the small final for third place.
            for pair in fold(&frontier) {
                push_pairing(pair, CupStage::Final, &mut pairings);
            }
            let losers = semifinal_losers.expect("semifinal replayed before the final round");
            for pair in fold(&losers) {
                push_pairing(pair, CupStage::SmallFinal, &mut pairings);
            }
        }
        Some(pairings)
    }

    /// The podium, if the final round's matches are all decided; otherwise
    /// `None`. Individual places are `None` when a double no-show left them
    /// undetermined (no champion / third to award), so awarding medals never
    /// panics on a missing winner.
    pub fn podium(&self, rounds: &[Round]) -> Option<CupPodium> {
        let cup_rounds = self.cup_rounds();
        // The final round must have been reached (its boards generated).
        rounds.iter().find(|r| r.number == cup_rounds)?;
        let (frontier, semifinal_losers) = self.replay_to(rounds, cup_rounds)?;
        let losers = semifinal_losers.expect("semifinal replayed before the final round");

        let (champion, runner_up) =
            self.decide_slot(*fold(&frontier).first()?, rounds, cup_rounds)?;
        let (third, fourth) = self.decide_slot(*fold(&losers).first()?, rounds, cup_rounds)?;
        Some(CupPodium {
            champion,
            runner_up,
            third,
            fourth,
        })
    }

    /// Replay the cup up to (but not including) round `r`, returning the frontier
    /// entering round `r` (a slot is `None` where both feeding players were
    /// no-shows) and the two semifinal losers (each `None` under the same). The
    /// losers are only populated once the semifinal has been replayed.
    #[allow(clippy::type_complexity)]
    fn replay_to(
        &self,
        rounds: &[Round],
        r: u32,
    ) -> Option<(Vec<Option<Uuid>>, Option<[Option<Uuid>; 2]>)> {
        let cup_rounds = self.cup_rounds();
        let mut frontier: Vec<Option<Uuid>> = self.seed_order.iter().copied().map(Some).collect();
        let mut semifinal_losers: Option<[Option<Uuid>; 2]> = None;
        for k in 1..r {
            let (winners, losers) = self.play_round(rounds, k, &frontier)?;
            if k == cup_rounds - 1 {
                semifinal_losers = Some([losers[0], losers[1]]);
            }
            frontier = winners;
        }
        Some((frontier, semifinal_losers))
    }

    /// The (winner, loser) of a single bracket slot, each `Option` because a
    /// double no-show or an empty feeding slot leaves it undetermined. `None`
    /// (the outer option) when a real two-player match hasn't been decided yet.
    fn decide_slot(
        &self,
        pair: (Option<Uuid>, Option<Uuid>),
        rounds: &[Round],
        k: u32,
    ) -> Option<(Option<Uuid>, Option<Uuid>)> {
        match pair {
            (Some(a), Some(b)) => match decide(rounds, k, a, b)? {
                CupResult::Winner { winner, loser } => Some((Some(winner), Some(loser))),
                CupResult::BothAbsent => Some((None, None)),
            },
            // A lone player faces an empty slot: they advance unopposed (a bye).
            (Some(a), None) | (None, Some(a)) => Some((Some(a), None)),
            (None, None) => Some((None, None)),
        }
    }

    /// Replay one cup round: fold the frontier, resolve each slot, and return the
    /// winners and losers in match order (each `None` where undetermined).
    #[allow(clippy::type_complexity)]
    fn play_round(
        &self,
        rounds: &[Round],
        k: u32,
        frontier: &[Option<Uuid>],
    ) -> Option<(Vec<Option<Uuid>>, Vec<Option<Uuid>>)> {
        let mut winners = Vec::new();
        let mut losers = Vec::new();
        for pair in fold(frontier) {
            let (w, l) = self.decide_slot(pair, rounds, k)?;
            winners.push(w);
            losers.push(l);
        }
        Some((winners, losers))
    }
}

/// Record a folded bracket pair as a match, a bye, or nothing (both slots dead).
fn push_pairing(pair: (Option<Uuid>, Option<Uuid>), stage: CupStage, out: &mut CupPairings) {
    match pair {
        (Some(player1), Some(player2)) => out.matches.push(CupMatch {
            player1,
            player2,
            stage,
        }),
        (Some(p), None) | (None, Some(p)) => out.byes.push(p),
        (None, None) => {}
    }
}

/// Pair the first with the last, second with second-last, … — the bracket fold.
fn fold<T: Copy>(list: &[T]) -> Vec<(T, T)> {
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

/// Find the board between `a` and `b` in round `k` and resolve its outcome.
/// `None` when the board is missing or not yet decided (neither a result nor a
/// no-show). A single no-show is a forfeit — the player who showed up wins —
/// while a double no-show advances nobody ([`CupResult::BothAbsent`]).
fn decide(rounds: &[Round], k: u32, a: Uuid, b: Uuid) -> Option<CupResult> {
    let round = rounds.iter().find(|r| r.number == k)?;
    let board = round
        .boards
        .iter()
        .find(|bd| (bd.player1 == a && bd.player2 == b) || (bd.player1 == b && bd.player2 == a))?;
    if matches!(board.no_show, Some(NoShow::Both)) {
        return Some(CupResult::BothAbsent);
    }
    // A single no-show: the player who showed up advances by forfeit.
    if let Some(winner) = board.no_show_opponent() {
        let loser = if winner == a { b } else { a };
        return Some(CupResult::Winner { winner, loser });
    }
    // Cup boards can never carry a handicap, so the Wiel-rule flag is moot here.
    Some(CupResult::Winner {
        winner: board.winner_id(true)?,
        loser: board.effective_loser(true)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::{Board, PairingSource, Winner};

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
            cup_byes: Vec::new(),
            absent: Vec::new(),
            completed: true,
        }
    }

    #[test]
    fn round_one_folds_the_seeds() {
        let seeds = ids(8);
        let cup = Cup {
            size: 8,
            seed_order: seeds.clone(),
        };
        let m = cup.matches_for_round(&[], 1).unwrap().matches;
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
        let cup = Cup {
            size: 8,
            seed_order: s.clone(),
        };
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
        let sf = cup
            .matches_for_round(std::slice::from_ref(&r1), 2)
            .unwrap()
            .matches;
        assert_eq!((sf[0].player1, sf[0].player2), (s[0], s[3]));
        assert_eq!((sf[1].player1, sf[1].player2), (s[1], s[2]));
        assert!(matches!(sf[0].stage, CupStage::Semifinal));
        let r2 = cup_round(
            2,
            &[
                (s[0], s[3], CupStage::Semifinal),
                (s[1], s[2], CupStage::Semifinal),
            ],
        );
        // R3 (final round): final s0 vs s1, small final s3 vs s2.
        let f = cup.matches_for_round(&[r1, r2], 3).unwrap().matches;
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].player1, f[0].player2), (s[0], s[1]));
        assert!(matches!(f[0].stage, CupStage::Final));
        assert_eq!((f[1].player1, f[1].player2), (s[3], s[2]));
        assert!(matches!(f[1].stage, CupStage::SmallFinal));
    }

    #[test]
    fn stage_names_scale_with_size() {
        let cup = Cup {
            size: 32,
            seed_order: ids(32),
        };
        assert_eq!(cup.cup_rounds(), 5);
        assert!(matches!(
            cup.matches_for_round(&[], 1).unwrap().matches[0].stage,
            CupStage::RoundOf(32)
        ));
    }
}
