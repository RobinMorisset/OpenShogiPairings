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

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::round::{CupStage, NoShow, PairingSource, Round};

/// The valid cup sizes (top-N), each a power of two.
pub const CUP_SIZES: [u32; 4] = [8, 16, 32, 64];

/// A hybrid tournament's cup, fixed at finalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Cup {
    /// Bracket size (8/16/32/64).
    pub size: u32,
    /// The seeded players, seed 1..size (index 0 = top seed). Frozen at finalize.
    pub seed_order: Vec<u32>,
}

/// One bracket pairing for a round, with the stage it belongs to.
#[derive(Debug, Clone)]
pub struct CupMatch {
    pub player1: u32,
    pub player2: u32,
    pub stage: CupStage,
}

/// The cup pairings for one round: the two-player bracket matches, plus any
/// players who advance unopposed (a cup bye — see [`Round::cup_byes`]).
#[derive(Debug, Clone, Default)]
pub struct CupPairings {
    pub matches: Vec<CupMatch>,
    pub byes: Vec<u32>,
}

/// The cup podium, once the final round is decided. Each place is `None` when it
/// couldn't be determined — e.g. both players of the final (or the small final)
/// were no-shows, so there is no champion / third to award.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct CupPodium {
    pub champion: Option<u32>,
    pub runner_up: Option<u32>,
    pub third: Option<u32>,
    pub fourth: Option<u32>,
}

/// The outcome of one decided cup match: a winner (a played result, or a single
/// no-show forfeit), or both players absent (a double no-show), which advances
/// nobody.
enum CupResult {
    Winner { winner: u32, loser: u32 },
    BothAbsent,
}

impl Cup {
    /// Number of tournament rounds the cup spans (`log2(size)`).
    pub fn cup_rounds(&self) -> u32 {
        self.size.trailing_zeros()
    }

    /// The tournament round each bracket round is played in: index `i` gives the
    /// (1-based) tournament round of bracket round `i + 1`. Normally the identity
    /// (bracket round `k` in tournament round `k`), but a bracket round played as
    /// **long** games (double time control) spans *two* tournament rounds — the
    /// second is a gap round the rest of the field plays through as pure Swiss —
    /// so every later bracket round shifts one further on. Derived by replaying
    /// the recorded long flags forward; a bracket round not yet played (or not
    /// marked long) defaults to a single round. See `docs/two-round-boards.md`.
    fn cup_schedule(&self, rounds: &[Round]) -> Vec<u32> {
        let mut sched = Vec::with_capacity(self.cup_rounds() as usize);
        let mut t = 1u32;
        for _ in 0..self.cup_rounds() {
            sched.push(t);
            // A bracket round is long when the cup boards in its tournament round
            // are flagged long (they are coupled, so all or none).
            let long = rounds.iter().find(|r| r.number == t).is_some_and(|r| {
                r.boards
                    .iter()
                    .any(|b| matches!(b.source, PairingSource::Cup { .. }) && b.long)
            });
            t += if long { 2 } else { 1 };
        }
        sched
    }

    /// Whether tournament round `r` hosts cup bracket matches (as opposed to a
    /// gap round between two long cup rounds, or a round past the cup).
    pub fn is_cup_round(&self, rounds: &[Round], r: u32) -> bool {
        self.cup_schedule(rounds).contains(&r)
    }

    /// The bracket pairings for tournament round `r`, reading earlier results from
    /// `rounds`: the two-player matches plus any unopposed advances (cup byes).
    /// `None` if a needed earlier cup result is missing (shouldn't happen for a
    /// properly gated round). Empty when `r` is a gap round (the idle second round
    /// of a long cup round) or past the cup.
    ///
    /// A slot goes empty when both players of the match that would fill it were
    /// no-shows: that match advances nobody, so whoever was drawn to face its
    /// winner advances unopposed as a bye.
    pub fn matches_for_round(&self, rounds: &[Round], r: u32) -> Option<CupPairings> {
        let sched = self.cup_schedule(rounds);
        // Which bracket round (1-based) is played in tournament round `r`? None →
        // `r` is a gap round or past the cup, so there are no cup pairings.
        let Some(pos) = sched.iter().position(|&t| t == r) else {
            return Some(CupPairings::default());
        };
        let bracket = pos as u32 + 1;
        let cup_rounds = self.cup_rounds();
        let (frontier, semifinal_losers) = self.replay_to(rounds, &sched, bracket)?;

        let mut pairings = CupPairings::default();
        if bracket < cup_rounds {
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
        let sched = self.cup_schedule(rounds);
        let final_tround = sched[cup_rounds as usize - 1];
        // The final round must have been reached (its boards generated).
        rounds.iter().find(|r| r.number == final_tround)?;
        let (frontier, semifinal_losers) = self.replay_to(rounds, &sched, cup_rounds)?;
        let losers = semifinal_losers.expect("semifinal replayed before the final round");

        let (champion, runner_up) =
            self.decide_slot(*fold(&frontier).first()?, rounds, final_tround)?;
        let (third, fourth) = self.decide_slot(*fold(&losers).first()?, rounds, final_tround)?;
        Some(CupPodium {
            champion,
            runner_up,
            third,
            fourth,
        })
    }

    /// Replay the cup up to (but not including) bracket round `bracket`, returning
    /// the frontier entering it (a slot is `None` where both feeding players were
    /// no-shows) and the two semifinal losers (each `None` under the same). Each
    /// earlier bracket round's results are read from its own tournament round via
    /// `sched`, so long cup rounds (which span two tournament rounds) replay
    /// correctly. The losers are only populated once the semifinal is replayed.
    #[allow(clippy::type_complexity)]
    fn replay_to(
        &self,
        rounds: &[Round],
        sched: &[u32],
        bracket: u32,
    ) -> Option<(Vec<Option<u32>>, Option<[Option<u32>; 2]>)> {
        let cup_rounds = self.cup_rounds();
        let mut frontier: Vec<Option<u32>> = self.seed_order.iter().copied().map(Some).collect();
        let mut semifinal_losers: Option<[Option<u32>; 2]> = None;
        for k in 1..bracket {
            let tround = sched[k as usize - 1];
            let (winners, losers) = self.play_round(rounds, tround, &frontier)?;
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
        pair: (Option<u32>, Option<u32>),
        rounds: &[Round],
        k: u32,
    ) -> Option<(Option<u32>, Option<u32>)> {
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
        frontier: &[Option<u32>],
    ) -> Option<(Vec<Option<u32>>, Vec<Option<u32>>)> {
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
fn push_pairing(pair: (Option<u32>, Option<u32>), stage: CupStage, out: &mut CupPairings) {
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
fn decide(rounds: &[Round], k: u32, a: u32, b: u32) -> Option<CupResult> {
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

/// Whether `seeds` (a power-of-two set of players) played a single-elimination
/// knockout over the recorded `rounds` — *irrespective of the bracket's seeding or
/// fold* — returning the champion if so. Each bracket round must be a **closed
/// internal matching**: every still-alive player faces another still-alive player
/// on a decided board, halving the field until one remains; rounds where the field
/// does not play entirely within itself (the Swiss pool, or a long-game gap round)
/// are skipped.
///
/// This is the seeding-robust test for "was this tournament actually run as a
/// top-`size` cup", unlike [`Cup::podium`], which replays one *specific* seeded
/// fold and so is thrown off by the near-tie seeding perturbations real events pick
/// up from slightly stale rating lists (a WOSC seeded off last month's list still
/// plays a standard bracket, just with a few close seeds a position or two out).
pub fn knockout_champion(rounds: &[Round], seeds: &[u32]) -> Option<u32> {
    let n = seeds.len();
    if n < 2 || !n.is_power_of_two() {
        return None;
    }
    let mut frontier: HashSet<u32> = seeds.iter().copied().collect();
    if frontier.len() != n {
        return None; // duplicate seeds
    }
    let max_round = rounds.iter().map(|r| r.number).max().unwrap_or(0);
    let mut r = 1u32;
    for _ in 0..n.trailing_zeros() {
        let winners = loop {
            if r > max_round {
                return None; // ran out of rounds before the field resolved
            }
            let found = frontier_round_winners(rounds, r, &frontier);
            r += 1;
            if let Some(w) = found {
                break w;
            }
        };
        frontier = winners;
    }
    (frontier.len() == 1).then(|| frontier.into_iter().next().unwrap())
}

/// Reconstruct the exact roster of a hybrid tournament's cup from its two
/// finalists (e.g. a championship's gold and silver medalists), working *backward*
/// from the final: the finalists' game is the final, each finalist's game in the
/// previous bracket round is a semifinal (their opponents the two beaten
/// semifinalists), and so on — the frontier doubles each round until it stops
/// growing. Returns `(size, roster)` with `size` a power of two, or `None` if the
/// two never met or the frontier doesn't resolve to a clean power-of-two bracket.
///
/// Unlike a seeded reconstruction, the roster is read straight off the played
/// games, so it is immune to the stale seeding ratings, declined entries and
/// borderline nationalities that make a top-`size`-by-rating guess unreliable —
/// all that's needed is who reached the final. Long-game gap rounds (where the
/// alive players are mid-game and record no new pairing) are skipped automatically.
pub fn reconstruct_cup_from_final(
    rounds: &[Round],
    gold: u32,
    silver: u32,
) -> Option<(u32, Vec<u32>)> {
    let max_round = rounds.iter().map(|r| r.number).max()?;
    // The final is the latest round in which the two finalists actually met.
    let final_round = (1..=max_round)
        .rev()
        .find(|&r| played(rounds, r, gold, silver))?;

    let mut frontier: HashSet<u32> = [gold, silver].into_iter().collect();
    let mut r = final_round;
    loop {
        // The previous bracket round: the latest earlier round where *every* still-
        // alive player has a game against a distinct opponent not already alive
        // (their beaten predecessor). Earlier rounds where that fails are gap rounds.
        let mut found = None;
        for rp in (1..r).rev() {
            if let Some(opps) = frontier_predecessors(rounds, rp, &frontier) {
                found = Some((rp, opps));
                break;
            }
        }
        match found {
            Some((rp, opps)) => {
                frontier.extend(opps);
                r = rp;
            }
            None => break,
        }
    }

    let size = frontier.len() as u32;
    (size >= 2 && size.is_power_of_two()).then(|| (size, frontier.into_iter().collect()))
}

/// Whether a board between `a` and `b` exists in round `r`.
fn played(rounds: &[Round], r: u32, a: u32, b: u32) -> bool {
    rounds.iter().find(|rd| rd.number == r).is_some_and(|rd| {
        rd.boards
            .iter()
            .any(|bd| (bd.player1 == a && bd.player2 == b) || (bd.player1 == b && bd.player2 == a))
    })
}

/// The beaten predecessors if, in round `r`, every `frontier` player has a board
/// against a distinct opponent *outside* the frontier; else `None` (a gap round, or
/// a round where the field played itself or someone twice).
fn frontier_predecessors(rounds: &[Round], r: u32, frontier: &HashSet<u32>) -> Option<Vec<u32>> {
    let round = rounds.iter().find(|rd| rd.number == r)?;
    let mut opps = Vec::with_capacity(frontier.len());
    let mut seen = HashSet::with_capacity(frontier.len());
    for &p in frontier {
        let board = round
            .boards
            .iter()
            .find(|b| b.player1 == p || b.player2 == p)?;
        let opp = if board.player1 == p {
            board.player2
        } else {
            board.player1
        };
        if frontier.contains(&opp) || !seen.insert(opp) {
            return None;
        }
        opps.push(opp);
    }
    Some(opps)
}

/// The winners if, in round `r`, every player of `frontier` faces another
/// `frontier` player on a decided board (a closed internal matching); else `None`.
fn frontier_round_winners(
    rounds: &[Round],
    r: u32,
    frontier: &HashSet<u32>,
) -> Option<HashSet<u32>> {
    let round = rounds.iter().find(|rd| rd.number == r)?;
    let mut winners = HashSet::with_capacity(frontier.len() / 2);
    let mut matched: HashSet<u32> = HashSet::with_capacity(frontier.len());
    for &p in frontier {
        if matched.contains(&p) {
            continue;
        }
        let board = round
            .boards
            .iter()
            .find(|b| b.player1 == p || b.player2 == p)?;
        let opp = if board.player1 == p {
            board.player2
        } else {
            board.player1
        };
        if !frontier.contains(&opp) {
            return None; // plays outside the set -> not a bracket round
        }
        match decide(rounds, r, p, opp)? {
            CupResult::Winner { winner, .. } => winners.insert(winner),
            CupResult::BothAbsent => return None,
        };
        matched.insert(p);
        matched.insert(opp);
    }
    (winners.len() == frontier.len() / 2).then_some(winners)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::{Board, PairingSource, Winner};

    fn ids(n: usize) -> Vec<u32> {
        (1..=n as u32).collect()
    }

    /// Build a completed round whose cup boards are the given (winner, loser) pairs.
    fn cup_round(number: u32, results: &[(u32, u32, CupStage)]) -> Round {
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
    fn reconstruct_cup_from_final_recovers_the_roster_backward() {
        let s = ids(8);
        let rounds = [
            cup_round(
                1,
                &[
                    (s[0], s[7], CupStage::Quarterfinal),
                    (s[1], s[6], CupStage::Quarterfinal),
                    (s[2], s[5], CupStage::Quarterfinal),
                    (s[3], s[4], CupStage::Quarterfinal),
                ],
            ),
            cup_round(
                2,
                &[
                    (s[0], s[3], CupStage::Semifinal),
                    (s[1], s[2], CupStage::Semifinal),
                ],
            ),
            cup_round(
                3,
                &[
                    (s[0], s[1], CupStage::Final),
                    (s[2], s[3], CupStage::SmallFinal),
                ],
            ),
        ];
        // From the two finalists s0 (gold) and s1 (silver), the whole 8-player
        // bracket is recovered — no seeding, nationality or attendance needed.
        let (size, roster) = reconstruct_cup_from_final(&rounds, s[0], s[1]).unwrap();
        assert_eq!(size, 8);
        assert_eq!(
            roster.into_iter().collect::<HashSet<_>>(),
            s.iter().copied().collect::<HashSet<_>>()
        );
        // Two players who never met can't be a final.
        assert!(reconstruct_cup_from_final(&rounds, s[0], s[4]).is_none());
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

    #[test]
    fn a_long_cup_round_consumes_two_tournament_rounds_and_the_bracket_resumes_after() {
        let s = ids(8);
        let cup = Cup {
            size: 8,
            seed_order: s.clone(),
        };

        // Tournament round 1 = quarterfinals, played as LONG games (double time
        // control), so they span rounds 1 and 2. Top seed of each match wins.
        let mut r1 = cup_round(
            1,
            &[
                (s[0], s[7], CupStage::Quarterfinal),
                (s[1], s[6], CupStage::Quarterfinal),
                (s[2], s[5], CupStage::Quarterfinal),
                (s[3], s[4], CupStage::Quarterfinal),
            ],
        );
        for b in &mut r1.boards {
            b.long = true;
        }

        // Round 2 is the gap round: no cup pairings, and the semifinal is pushed to
        // round 3 instead of round 2.
        assert!(cup
            .matches_for_round(std::slice::from_ref(&r1), 2)
            .unwrap()
            .matches
            .is_empty());
        assert!(!cup.is_cup_round(std::slice::from_ref(&r1), 2));
        assert!(cup.is_cup_round(std::slice::from_ref(&r1), 3));

        // Round 3 hosts the semifinal, fed by the round-1 quarterfinal results.
        let sf = cup
            .matches_for_round(std::slice::from_ref(&r1), 3)
            .unwrap()
            .matches;
        assert_eq!((sf[0].player1, sf[0].player2), (s[0], s[3]));
        assert_eq!((sf[1].player1, sf[1].player2), (s[1], s[2]));
        assert!(matches!(sf[0].stage, CupStage::Semifinal));

        // Play the semifinal in round 3 (ordinary length): s0, s1 win.
        let r3 = cup_round(
            3,
            &[
                (s[0], s[3], CupStage::Semifinal),
                (s[1], s[2], CupStage::Semifinal),
            ],
        );
        // Round 4 hosts the final and small final.
        let f = cup
            .matches_for_round(&[r1.clone(), r3.clone()], 4)
            .unwrap()
            .matches;
        assert_eq!((f[0].player1, f[0].player2), (s[0], s[1]));
        assert!(matches!(f[0].stage, CupStage::Final));
        assert_eq!((f[1].player1, f[1].player2), (s[3], s[2]));
        assert!(matches!(f[1].stage, CupStage::SmallFinal));

        // The podium follows the shifted schedule (final is round 4, not round 3).
        let r4 = cup_round(
            4,
            &[
                (s[0], s[1], CupStage::Final),
                (s[3], s[2], CupStage::SmallFinal),
            ],
        );
        assert!(cup.podium(&[r1.clone(), r3.clone()]).is_none()); // final not reached
        let podium = cup.podium(&[r1, r3, r4]).unwrap();
        assert_eq!(podium.champion, Some(s[0]));
        assert_eq!(podium.runner_up, Some(s[1]));
        assert_eq!(podium.third, Some(s[3]));
        assert_eq!(podium.fourth, Some(s[2]));
    }
}
