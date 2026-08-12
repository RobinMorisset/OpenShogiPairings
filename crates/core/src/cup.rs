//! The direct-elimination cup that runs alongside the Swiss in a hybrid
//! tournament (the French / European Shogi Championship format).
//!
//! The `size`-player bracket (a power of two, 8..64) occupies the first
//! `log2(size)` rounds. Only the immutable `{size, format, seed_order}` is
//! stored; the state of the bracket in any given round is **derived** by
//! replaying the recorded results, so editing a past result or cancelling a round
//! re-derives correctly (same philosophy as the live-recomputed scores).
//!
//! Bracket shape: round 1 folds the entrants `(1,size), (2,size-1), …`; each
//! later round folds the previous round's winners *in match order*, so seeds stay
//! separated exactly as in a standard seeded bracket. The semifinal's two losers
//! meet in a **small final** for third place, played in the same (final) round as
//! the final itself; after that round everyone — champion included — is in the
//! Swiss pool.
//!
//! Two formats fill that bracket (see [`CupFormat`]):
//!
//! * [`Direct`](CupFormat::Direct) — the top `size` eligible players *are* the
//!   bracket; it starts in round 1.
//! * [`Qualifier`](CupFormat::Qualifier) — the top `size / 2` eligible players are
//!   **pre-qualified** and spend round 1 playing an ordinary Swiss game in the
//!   open, while the next `size` play round 1 off against each other in a
//!   **qualification** round. Its `size / 2` winners join the pre-qualified in the
//!   bracket, which then runs from round 2. So the cup takes `1.5 × size` eligible
//!   players and `log2(size) + 1` rounds. Seeding still works out: bracket round 1
//!   folds `pre-qualified ++ qualifiers-in-match-order`, and because the
//!   qualification round is itself a fold, the top seed meets the winner of the
//!   weakest qualification match.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::round::{CupStage, Forfeit, PairingSource, Round};
use crate::tournament::TournamentError;
use crate::units::TournamentId;

/// The valid cup **bracket** sizes, each a power of two. The qualifier format
/// needs half as many players again (see [`cup_field_size`]).
pub const CUP_SIZES: [u32; 4] = [8, 16, 32, 64];

/// How the cup bracket is filled. See the module docs for the shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum CupFormat {
    /// The top `size` eligible players are seeded straight into the bracket,
    /// which runs from round 1.
    #[default]
    Direct,
    /// The top `size / 2` eligible players are pre-qualified (they play the open
    /// in round 1); the next `size` play a qualification round, and its winners
    /// complete the bracket, which runs from round 2.
    Qualifier,
}

/// How many eligible players a cup of this bracket size and format takes: `size`
/// for a direct bracket, `1.5 × size` for the qualifier format (`size / 2`
/// pre-qualified plus `size` in the qualification round).
pub const fn cup_field_size(size: u32, format: CupFormat) -> u32 {
    match format {
        CupFormat::Direct => size,
        CupFormat::Qualifier => size + size / 2,
    }
}

/// A hybrid tournament's cup, fixed at finalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Cup {
    /// Bracket size (8/16/32/64) — *not* the number of players taken, which is
    /// [`cup_field_size`] of this and `format`.
    pub size: u32,
    /// How the bracket is filled (see [`CupFormat`]).
    pub format: CupFormat,
    /// The seeded players, seed 1..`cup_field_size(size, format)` (index 0 = top
    /// seed). Frozen at finalize. Under [`CupFormat::Qualifier`] the first
    /// `size / 2` are the pre-qualified and the rest are the qualification field.
    pub seed_order: Vec<TournamentId>,
}

/// One bracket pairing for a round, with the stage it belongs to.
#[derive(Debug, Clone)]
pub struct CupMatch {
    pub player1: TournamentId,
    pub player2: TournamentId,
    pub stage: CupStage,
}

/// The cup pairings for one round: the two-player bracket matches, plus any
/// players who advance unopposed (a cup bye), each with the stage they advanced
/// through. A cup bye becomes a [`SitoutKind::CupBye`] sit-out on the round.
///
/// [`SitoutKind::CupBye`]: crate::round::SitoutKind::CupBye
#[derive(Debug, Clone, Default)]
pub struct CupPairings {
    pub matches: Vec<CupMatch>,
    pub byes: Vec<(TournamentId, CupStage)>,
}

/// The cup podium, once the final round is decided. Each place is `None` when it
/// couldn't be determined — e.g. both players of the final (or the small final)
/// were no-shows, so there is no champion / third to award.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct CupPodium {
    pub champion: Option<TournamentId>,
    pub runner_up: Option<TournamentId>,
    pub third: Option<TournamentId>,
    pub fourth: Option<TournamentId>,
}

/// One match (a node of the bracket tree) in the derived view sent to clients.
/// Either player is `None` while still to be determined (a feeding match not yet
/// decided); `winner` is `None` until this match itself is decided. A player
/// facing a `None` opponent advanced unopposed (a cup bye), so `winner` is set
/// while the other slot stays empty.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct BracketMatch {
    pub player1: Option<TournamentId>,
    pub player2: Option<TournamentId>,
    pub winner: Option<TournamentId>,
    pub stage: CupStage,
}

/// The whole cup bracket, derived for the client from the frozen seeding and the
/// recorded results — the authoritative shape the UI renders, so it never re-runs
/// the fold. The full tree is drawn the instant the bracket is frozen (every
/// later-round slot `None`), then fills in as results land. Mirrors the replay in
/// [`Cup::matches_for_round`] / [`Cup::podium`], but tolerant of undecided and
/// future rounds so the empty bracket still renders.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct CupBracketView {
    /// Bracket size (top-N), for labelling / sizing.
    pub size: u32,
    /// The qualification round, under [`CupFormat::Qualifier`] only: `size / 2`
    /// play-off matches whose winners fill the bracket alongside the
    /// pre-qualified. Kept out of `rounds` because it does not double into the
    /// next column like a bracket round does — match `j` here feeds the *second*
    /// slot of `rounds[0][size / 2 - 1 - j]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualification: Option<Vec<BracketMatch>>,
    /// Columns of matches, index 0 = the first bracket round, last = the final.
    pub rounds: Vec<Vec<BracketMatch>>,
    /// The third-place match — the two semifinal losers — present once the
    /// bracket reaches a semifinal (its players fill in with the results).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub small_final: Option<BracketMatch>,
}

/// The outcome of one decided cup match: a winner (a played result, or a single
/// no-show forfeit), or both players absent (a double no-show), which advances
/// nobody.
enum CupResult {
    Winner {
        winner: TournamentId,
        loser: TournamentId,
    },
    BothAbsent,
}

impl Cup {
    /// Number of tournament rounds the cup spans: the `log2(size)` bracket rounds
    /// plus the qualification round, if the format has one.
    pub fn cup_rounds(&self) -> u32 {
        self.qualification_rounds() + self.bracket_rounds()
    }

    /// Number of *bracket* rounds (`log2(size)`), excluding any qualification
    /// round. The semifinal is bracket round `bracket_rounds() - 1`.
    fn bracket_rounds(&self) -> u32 {
        self.size.trailing_zeros()
    }

    /// `1` when a qualification round precedes the bracket, `0` otherwise — also
    /// the index of bracket round 1 in the cup schedule.
    fn qualification_rounds(&self) -> u32 {
        match self.format {
            CupFormat::Direct => 0,
            CupFormat::Qualifier => 1,
        }
    }

    /// How many eligible players this cup takes (see [`cup_field_size`]).
    pub fn field_size(&self) -> u32 {
        cup_field_size(self.size, self.format)
    }

    /// Re-check the shape [`Tournament::finalize_registration_with`] froze, for a
    /// cup that arrived from a save file rather than from that constructor.
    ///
    /// Everything below this point takes the shape as given and *panics* if it
    /// doesn't hold: [`Self::qualifier_split`] splits the seed list at `size / 2`,
    /// [`Self::bracket_rounds`] reads `size` as a power of two, and both
    /// [`Self::podium`] and [`Self::bracket_view`] index the semifinal's two
    /// losers. [`Self::bracket_view`] is rebuilt for *every* tournament response,
    /// so a cup that disagrees with its bracket size takes down a plain read —
    /// which is why this runs on load, from [`Tournament::validate_loaded`].
    ///
    /// [`Tournament::finalize_registration_with`]: crate::Tournament::finalize_registration_with
    /// [`Tournament::validate_loaded`]: crate::Tournament::validate_loaded
    pub(crate) fn validate_shape(&self) -> Result<(), TournamentError> {
        if !CUP_SIZES.contains(&self.size) {
            return Err(TournamentError::InvalidCupSize { size: self.size });
        }
        let expected = self.field_size();
        if self.seed_order.len() != expected as usize {
            return Err(TournamentError::CupSeedCountMismatch {
                size: self.size,
                expected,
                found: self.seed_order.len(),
            });
        }
        // A repeated seed doesn't panic, but it puts one player in two slots of
        // the same bracket — a fold that can pair them with themselves, and a
        // podium that can award them two places.
        let mut seen = HashSet::with_capacity(self.seed_order.len());
        if let Some(&seed) = self.seed_order.iter().find(|&&s| !seen.insert(s)) {
            return Err(TournamentError::DuplicateCupSeed { seed });
        }
        Ok(())
    }

    /// The qualifier format's two groups of seeds: `(pre-qualified,
    /// qualification field)`, of `size / 2` and `size` players. `None` for a
    /// direct cup, whose seeds are all bracket entrants.
    fn qualifier_split(&self) -> Option<(&[TournamentId], &[TournamentId])> {
        match self.format {
            CupFormat::Direct => None,
            CupFormat::Qualifier => Some(self.seed_order.split_at((self.size / 2) as usize)),
        }
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
        // Which cup round (0-based) is played in tournament round `r`? None →
        // `r` is a gap round or past the cup, so there are no cup pairings.
        let Some(pos) = sched.iter().position(|&t| t == r) else {
            return Some(CupPairings::default());
        };
        let qual_rounds = self.qualification_rounds() as usize;
        if pos < qual_rounds {
            // The qualification round: the `size` seeds below the pre-qualified
            // fold among themselves. Every slot is a known player, so this can
            // never produce a bye.
            let (_, qualifiers) = self
                .qualifier_split()
                .expect("a qualification round only exists in the qualifier format");
            let qualifiers: Vec<Option<TournamentId>> =
                qualifiers.iter().copied().map(Some).collect();
            let mut pairings = CupPairings::default();
            for pair in fold(&qualifiers) {
                push_pairing(pair, CupStage::Qualification, &mut pairings);
            }
            return Some(pairings);
        }
        let bracket = (pos - qual_rounds) as u32 + 1;
        let cup_rounds = self.bracket_rounds();
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

    /// The **pre-qualified** players of tournament round `r`: the top `size / 2`
    /// seeds, who sit out the qualification round and play the open instead.
    /// Empty for a direct cup and for every round but the qualification round.
    ///
    /// They are ordinary Swiss players that round, with one exception the pairing
    /// engine has to be told about — they must not be paired with each other, so
    /// it needs them by name. See `Rule::CupPrequalified` in [`crate::pairing`].
    pub fn prequalified_for_round(&self, rounds: &[Round], r: u32) -> &[TournamentId] {
        let Some((prequalified, _)) = self.qualifier_split() else {
            return &[];
        };
        if self.cup_schedule(rounds).first() == Some(&r) {
            prequalified
        } else {
            &[]
        }
    }

    /// The podium, if the final round's matches are all decided; otherwise
    /// `None`. Individual places are `None` when a double no-show left them
    /// undetermined (no champion / third to award), so awarding medals never
    /// panics on a missing winner.
    pub fn podium(&self, rounds: &[Round]) -> Option<CupPodium> {
        let sched = self.cup_schedule(rounds);
        let final_tround = *sched.last().expect("a cup always has at least one round");
        // The final round must have been reached (its boards generated).
        rounds.iter().find(|r| r.number == final_tround)?;
        let (frontier, semifinal_losers) = self.replay_to(rounds, &sched, self.bracket_rounds())?;
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

    /// The full bracket as a tree of matches for the client: round 1 folded from
    /// the seeds, each later round pairing the previous round's winners in match
    /// order, plus a third-place match off the semifinal losers. Unlike the
    /// replay helpers this never fails — an unknown player or an undecided board
    /// is simply `None` — so the whole tree (including rounds not yet reached)
    /// always renders. Long cup rounds are honoured via [`Self::cup_schedule`].
    pub fn bracket_view(&self, rounds: &[Round]) -> CupBracketView {
        let sched = self.cup_schedule(rounds);
        let cup_rounds = self.bracket_rounds();
        let qual_rounds = self.qualification_rounds() as usize;
        let mut columns: Vec<Vec<BracketMatch>> = Vec::with_capacity(cup_rounds as usize);
        // The qualification round, when the format has one, and the players it
        // leaves entering bracket round 1 (every seed known at round 1 otherwise).
        let (qualification, mut frontier) = self.qualification_view(rounds, &sched);
        let mut semifinal_losers: Option<[Slot; 2]> = None;

        for bracket in 1..=cup_rounds {
            let tround = sched[qual_rounds + bracket as usize - 1];
            let stage = if bracket < cup_rounds {
                stage_before_final(frontier.len() as u32)
            } else {
                CupStage::Final
            };
            let mut col = Vec::with_capacity(frontier.len() / 2);
            let mut winners = Vec::with_capacity(frontier.len() / 2);
            let mut losers = Vec::with_capacity(frontier.len() / 2);
            for (p1, p2) in fold(&frontier) {
                let (winner, loser) = resolve_slot(rounds, tround, p1, p2);
                col.push(BracketMatch {
                    player1: p1.tid(),
                    player2: p2.tid(),
                    winner: winner.tid(),
                    stage,
                });
                winners.push(winner);
                losers.push(loser);
            }
            // The semifinal is the round before the final; its two losers meet in
            // the small final (played in the same tournament round as the final).
            if bracket == cup_rounds - 1 {
                semifinal_losers = Some([losers[0], losers[1]]);
            }
            columns.push(col);
            frontier = winners;
        }

        let small_final = semifinal_losers.map(|[l0, l1]| {
            let final_tround = sched[qual_rounds + cup_rounds as usize - 1];
            let (winner, _) = resolve_slot(rounds, final_tround, l0, l1);
            BracketMatch {
                player1: l0.tid(),
                player2: l1.tid(),
                winner: winner.tid(),
                stage: CupStage::SmallFinal,
            }
        });

        CupBracketView {
            size: self.size,
            qualification,
            rounds: columns,
            small_final,
        }
    }

    /// The qualification column for [`Self::bracket_view`] and the slots entering
    /// bracket round 1. For a direct cup there is no column and the seeds enter
    /// as they are; for the qualifier format the lower `size` seeds fold into
    /// play-off matches, and the pre-qualified are followed by those matches'
    /// winners in match order. Like the rest of the view this never fails — an
    /// undecided play-off simply leaves a `Pending` slot.
    fn qualification_view(
        &self,
        rounds: &[Round],
        sched: &[u32],
    ) -> (Option<Vec<BracketMatch>>, Vec<Slot>) {
        let Some((prequalified, qualifiers)) = self.qualifier_split() else {
            return (
                None,
                self.seed_order.iter().copied().map(Slot::Player).collect(),
            );
        };
        let qualifiers: Vec<Slot> = qualifiers.iter().copied().map(Slot::Player).collect();
        let mut column = Vec::with_capacity(qualifiers.len() / 2);
        let mut winners = Vec::with_capacity(qualifiers.len() / 2);
        for (p1, p2) in fold(&qualifiers) {
            let (winner, _) = resolve_slot(rounds, sched[0], p1, p2);
            column.push(BracketMatch {
                player1: p1.tid(),
                player2: p2.tid(),
                winner: winner.tid(),
                stage: CupStage::Qualification,
            });
            winners.push(winner);
        }
        let frontier = prequalified
            .iter()
            .copied()
            .map(Slot::Player)
            .chain(winners)
            .collect();
        (Some(column), frontier)
    }

    /// The `size` players entering bracket round 1: the seeds themselves for a
    /// direct cup, or the pre-qualified followed by the qualification round's
    /// winners **in match order** for the qualifier format. A winner slot is
    /// `None` when that qualification match was a double no-show, which leaves the
    /// pre-qualified player it feeds facing an empty slot (a cup bye). `None` when
    /// a qualification result is still missing.
    fn initial_frontier(
        &self,
        rounds: &[Round],
        sched: &[u32],
    ) -> Option<Vec<Option<TournamentId>>> {
        let Some((prequalified, qualifiers)) = self.qualifier_split() else {
            return Some(self.seed_order.iter().copied().map(Some).collect());
        };
        let qualifiers: Vec<Option<TournamentId>> = qualifiers.iter().copied().map(Some).collect();
        let (winners, _) = self.play_round(rounds, sched[0], &qualifiers)?;
        Some(
            prequalified
                .iter()
                .copied()
                .map(Some)
                .chain(winners)
                .collect(),
        )
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
    ) -> Option<(Vec<Option<TournamentId>>, Option<[Option<TournamentId>; 2]>)> {
        let bracket_rounds = self.bracket_rounds();
        // Bracket round `k` is played in `sched[qual_rounds + k - 1]`: the
        // qualification round, when there is one, occupies the first slot.
        let qual_rounds = self.qualification_rounds() as usize;
        let mut frontier = self.initial_frontier(rounds, sched)?;
        let mut semifinal_losers: Option<[Option<TournamentId>; 2]> = None;
        for k in 1..bracket {
            let tround = sched[qual_rounds + k as usize - 1];
            let (winners, losers) = self.play_round(rounds, tround, &frontier)?;
            if k == bracket_rounds - 1 {
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
        pair: (Option<TournamentId>, Option<TournamentId>),
        rounds: &[Round],
        k: u32,
    ) -> Option<(Option<TournamentId>, Option<TournamentId>)> {
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
        frontier: &[Option<TournamentId>],
    ) -> Option<(Vec<Option<TournamentId>>, Vec<Option<TournamentId>>)> {
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
fn push_pairing(
    pair: (Option<TournamentId>, Option<TournamentId>),
    stage: CupStage,
    out: &mut CupPairings,
) {
    match pair {
        (Some(player1), Some(player2)) => out.matches.push(CupMatch {
            player1,
            player2,
            stage,
        }),
        (Some(p), None) | (None, Some(p)) => out.byes.push((p, stage)),
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

/// A bracket slot for the [`CupBracketView`]. Three states, because the empty
/// bracket must distinguish a player whose opponent is merely *not yet known*
/// (nobody advances) from one whose opponent slot is genuinely dead (a real bye).
/// Both flatten to `None` in the emitted view — the distinction only governs
/// advancement.
#[derive(Clone, Copy)]
enum Slot {
    /// A known player.
    Player(TournamentId),
    /// Permanently empty — both feeding players were no-shows, so the opposing
    /// slot's player advances unopposed (a cup bye).
    Empty,
    /// Not yet determined — the feeding match hasn't been decided; the winner
    /// stays pending until it is.
    Pending,
}

impl Slot {
    /// The tournament number, or `None` for an empty or pending slot.
    fn tid(self) -> Option<TournamentId> {
        match self {
            Slot::Player(p) => Some(p),
            Slot::Empty | Slot::Pending => None,
        }
    }
}

/// Resolve one bracket slot for the [`CupBracketView`]: `(winner, loser)`. A real
/// player advances only against a genuinely `Empty` opponent (a bye); against a
/// still-`Pending` opponent the outcome stays `Pending`, so a lone finalist is
/// never crowned before their opponent is even known. Unlike [`Cup::decide_slot`]
/// this never fails — an undecided board is just a `Pending` result.
fn resolve_slot(rounds: &[Round], k: u32, a: Slot, b: Slot) -> (Slot, Slot) {
    match (a, b) {
        (Slot::Player(x), Slot::Player(y)) => match decide(rounds, k, x, y) {
            Some(CupResult::Winner { winner, loser }) => {
                (Slot::Player(winner), Slot::Player(loser))
            }
            Some(CupResult::BothAbsent) => (Slot::Empty, Slot::Empty),
            None => (Slot::Pending, Slot::Pending),
        },
        (Slot::Player(x), Slot::Empty) | (Slot::Empty, Slot::Player(x)) => {
            (Slot::Player(x), Slot::Empty)
        }
        (Slot::Pending, _) | (_, Slot::Pending) => (Slot::Pending, Slot::Pending),
        (Slot::Empty, Slot::Empty) => (Slot::Empty, Slot::Empty),
    }
}

/// Find the board between `a` and `b` in round `k` and resolve its outcome.
/// `None` when the board is missing or not yet decided (neither a result nor a
/// no-show). A single no-show is a forfeit — the player who showed up wins —
/// while a double no-show advances nobody ([`CupResult::BothAbsent`]).
fn decide(rounds: &[Round], k: u32, a: TournamentId, b: TournamentId) -> Option<CupResult> {
    let round = rounds.iter().find(|r| r.number == k)?;
    let board = round
        .boards
        .iter()
        .find(|bd| (bd.player1 == a && bd.player2 == b) || (bd.player1 == b && bd.player2 == a))?;
    if matches!(board.outcome.forfeit(), Some(Forfeit::Both(..))) {
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
pub fn knockout_champion(rounds: &[Round], seeds: &[TournamentId]) -> Option<TournamentId> {
    let n = seeds.len();
    if n < 2 || !n.is_power_of_two() {
        return None;
    }
    let mut frontier: HashSet<TournamentId> = seeds.iter().copied().collect();
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
    gold: TournamentId,
    silver: TournamentId,
) -> Option<(u32, Vec<TournamentId>)> {
    let max_round = rounds.iter().map(|r| r.number).max()?;
    // The final is the latest round in which the two finalists actually met.
    let final_round = (1..=max_round)
        .rev()
        .find(|&r| played(rounds, r, gold, silver))?;

    let mut frontier: HashSet<TournamentId> = [gold, silver].into_iter().collect();
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
fn played(rounds: &[Round], r: u32, a: TournamentId, b: TournamentId) -> bool {
    rounds.iter().find(|rd| rd.number == r).is_some_and(|rd| {
        rd.boards
            .iter()
            .any(|bd| (bd.player1 == a && bd.player2 == b) || (bd.player1 == b && bd.player2 == a))
    })
}

/// The beaten predecessors if, in round `r`, every `frontier` player has a board
/// against a distinct opponent *outside* the frontier; else `None` (a gap round, or
/// a round where the field played itself or someone twice).
fn frontier_predecessors(
    rounds: &[Round],
    r: u32,
    frontier: &HashSet<TournamentId>,
) -> Option<Vec<TournamentId>> {
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
    frontier: &HashSet<TournamentId>,
) -> Option<HashSet<TournamentId>> {
    let round = rounds.iter().find(|rd| rd.number == r)?;
    let mut winners = HashSet::with_capacity(frontier.len() / 2);
    let mut matched: HashSet<TournamentId> = HashSet::with_capacity(frontier.len());
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
    use crate::pairing::RoundExplanation;
    use crate::round::{AbsenceKind, Board, Outcome, PairingSource, Winner};

    fn ids(n: usize) -> Vec<TournamentId> {
        (1..=n as u32).map(TournamentId).collect()
    }

    /// Build a completed round whose cup boards are the given (winner, loser) pairs.
    fn cup_round(number: u32, results: &[(TournamentId, TournamentId, CupStage)]) -> Round {
        Round {
            number,
            explanation: RoundExplanation::empty(number),
            boards: results
                .iter()
                .map(|&(w, l, stage)| Board {
                    outcome: Outcome::won(Winner::Player1), // player1 = winner
                    source: PairingSource::Cup { stage },
                    ..Board::pending(w, l, 0, PairingSource::Swiss)
                })
                .collect(),
            sitouts: Vec::new(),
            completed: true,
        }
    }

    #[test]
    fn round_one_folds_the_seeds() {
        let seeds = ids(8);
        let cup = Cup {
            format: CupFormat::Direct,
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
    fn bracket_view_builds_the_full_tree_with_results() {
        let s = ids(8);
        let cup = Cup {
            format: CupFormat::Direct,
            size: 8,
            seed_order: s.clone(),
        };
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
        let v = cup.bracket_view(&rounds);
        assert_eq!(v.size, 8);
        assert_eq!(v.rounds.len(), 3);

        // Quarterfinals: the seed fold, player1 wins each.
        let qf = &v.rounds[0];
        assert_eq!(qf.len(), 4);
        assert_eq!((qf[0].player1, qf[0].player2), (Some(s[0]), Some(s[7])));
        assert_eq!((qf[3].player1, qf[3].player2), (Some(s[3]), Some(s[4])));
        assert!(matches!(qf[0].stage, CupStage::Quarterfinal));
        assert_eq!(qf[0].winner, Some(s[0]));

        // Semifinals: winners folded in match order — (s0,s3),(s1,s2).
        let sf = &v.rounds[1];
        assert_eq!(sf.len(), 2);
        assert_eq!((sf[0].player1, sf[0].player2), (Some(s[0]), Some(s[3])));
        assert!(matches!(sf[0].stage, CupStage::Semifinal));
        assert_eq!((sf[0].winner, sf[1].winner), (Some(s[0]), Some(s[1])));

        // Final + champion (the last column's single match).
        let fin = &v.rounds[2];
        assert_eq!(fin.len(), 1);
        assert_eq!((fin[0].player1, fin[0].player2), (Some(s[0]), Some(s[1])));
        assert!(matches!(fin[0].stage, CupStage::Final));
        assert_eq!(fin[0].winner, Some(s[0]));

        // Small final: the two semifinal losers (s3, s2); s2 takes third, s3 fourth.
        let small = v.small_final.expect("small final present");
        assert_eq!((small.player1, small.player2), (Some(s[3]), Some(s[2])));
        assert!(matches!(small.stage, CupStage::SmallFinal));
        assert_eq!(small.winner, Some(s[2]));
    }

    #[test]
    fn bracket_view_keeps_a_lone_finalist_pending_until_the_other_semifinal_is_decided() {
        // Regression: a `None` opposing slot from an *undecided* feeding match must
        // not advance the lone player — only a genuinely empty (double no-show) slot
        // does. Otherwise a finalist is crowned before their opponent is even known.
        let s = ids(8);
        let cup = Cup {
            format: CupFormat::Direct,
            size: 8,
            seed_order: s.clone(),
        };
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
            // Only one semifinal decided; the other is still pending.
            cup_round(2, &[(s[0], s[3], CupStage::Semifinal)]),
        ];
        let v = cup.bracket_view(&rounds);
        let fin = &v.rounds[2][0];
        // One finalist known (s0), the other still to be determined, nobody crowned.
        assert_eq!((fin.player1, fin.player2), (Some(s[0]), None));
        assert_eq!(fin.winner, None);
        // Likewise the third-place match: one semifinal loser known, the rest pending.
        let small = v.small_final.expect("small final slot present");
        assert_eq!((small.player1, small.player2), (Some(s[3]), None));
        assert_eq!(small.winner, None);
    }

    #[test]
    fn bracket_view_advances_a_lone_player_past_a_double_no_show() {
        // A genuinely empty slot (both feeding players no-showed) is a real bye: the
        // opponent advances unopposed even though the next round isn't played yet.
        let s = ids(8);
        let cup = Cup {
            format: CupFormat::Direct,
            size: 8,
            seed_order: s.clone(),
        };
        let mut r1 = cup_round(
            1,
            &[
                (s[1], s[6], CupStage::Quarterfinal),
                (s[2], s[5], CupStage::Quarterfinal),
                (s[3], s[4], CupStage::Quarterfinal),
            ],
        );
        // The top quarterfinal (seeds 1 & 8) is a double no-show — it advances nobody.
        r1.boards.push(Board {
            outcome: Outcome::Forfeit {
                absent: Forfeit::Both(AbsenceKind::NoShow, AbsenceKind::NoShow),
            },
            source: PairingSource::Cup {
                stage: CupStage::Quarterfinal,
            },
            ..Board::pending(s[0], s[7], 0, PairingSource::Swiss)
        });
        let v = cup.bracket_view(&[r1]);

        // That quarterfinal resolves to nobody.
        assert_eq!(v.rounds[0][0].winner, None);
        // Its winner's semifinal slot is empty, so the seed drawn to face it (s3,
        // the winner of 4v5 folded opposite) advances as a bye — winner set, one
        // slot empty — despite the semifinal round not having been played.
        let sf0 = &v.rounds[1][0];
        assert_eq!((sf0.player1, sf0.player2), (None, Some(s[3])));
        assert_eq!(sf0.winner, Some(s[3]));
    }

    #[test]
    fn bracket_view_draws_the_empty_tree_before_any_result() {
        let cup = Cup {
            format: CupFormat::Direct,
            size: 16,
            seed_order: ids(16),
        };
        let v = cup.bracket_view(&[]);
        // 16 → round of 16, quarterfinal, semifinal, final: four columns.
        assert_eq!(v.rounds.len(), 4);
        assert_eq!(v.rounds[0].len(), 8);
        assert!(matches!(v.rounds[0][0].stage, CupStage::RoundOf(16)));
        // Round 1's players are the known seeds; every winner and later-round slot
        // is still undetermined.
        assert!(v.rounds[0]
            .iter()
            .all(|m| m.player1.is_some() && m.player2.is_some() && m.winner.is_none()));
        assert!(v.rounds[1]
            .iter()
            .all(|m| m.player1.is_none() && m.player2.is_none()));
        // The third-place slot exists from the start, both players undetermined.
        let small = v
            .small_final
            .expect("small final slot present even when empty");
        assert_eq!((small.player1, small.player2), (None, None));
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
            format: CupFormat::Direct,
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
            format: CupFormat::Direct,
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
            format: CupFormat::Direct,
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

    // --- the qualifier format ------------------------------------------------

    /// A size-8 qualifier cup: seeds 1-4 pre-qualified, seeds 5-12 play off.
    fn qualifier_cup() -> (Cup, Vec<TournamentId>) {
        let seeds = ids(12);
        let cup = Cup {
            format: CupFormat::Qualifier,
            size: 8,
            seed_order: seeds.clone(),
        };
        (cup, seeds)
    }

    /// The qualification round played to form, then the bracket the top half wins.
    /// Seeds 5-12 fold to (5,12),(6,11),(7,10),(8,9); the higher seed wins each,
    /// so the qualifiers are 5,6,7,8 in match order.
    fn qualifier_rounds(s: &[TournamentId]) -> Vec<Round> {
        vec![
            cup_round(
                1,
                &[
                    (s[4], s[11], CupStage::Qualification),
                    (s[5], s[10], CupStage::Qualification),
                    (s[6], s[9], CupStage::Qualification),
                    (s[7], s[8], CupStage::Qualification),
                ],
            ),
            // Bracket round 1 folds [1,2,3,4] ++ [5,6,7,8] → (1,8),(2,7),(3,6),(4,5).
            cup_round(
                2,
                &[
                    (s[0], s[7], CupStage::Quarterfinal),
                    (s[1], s[6], CupStage::Quarterfinal),
                    (s[2], s[5], CupStage::Quarterfinal),
                    (s[3], s[4], CupStage::Quarterfinal),
                ],
            ),
            cup_round(
                3,
                &[
                    (s[0], s[3], CupStage::Semifinal),
                    (s[1], s[2], CupStage::Semifinal),
                ],
            ),
            cup_round(
                4,
                &[
                    (s[0], s[1], CupStage::Final),
                    (s[2], s[3], CupStage::SmallFinal),
                ],
            ),
        ]
    }

    #[test]
    fn qualifier_takes_half_as_many_players_again_and_one_more_round() {
        let (cup, _) = qualifier_cup();
        assert_eq!(cup.field_size(), 12);
        assert_eq!(cup.cup_rounds(), 4); // log2(8) + the qualification round
        assert_eq!(cup_field_size(64, CupFormat::Qualifier), 96);
        assert_eq!(cup_field_size(64, CupFormat::Direct), 64);
    }

    #[test]
    fn qualifier_round_one_is_the_playoff_and_leaves_the_prequalified_in_the_open() {
        let (cup, s) = qualifier_cup();
        let m = cup.matches_for_round(&[], 1).unwrap().matches;
        // Only the eight below the pre-qualified play, folded 5v12 … 8v9.
        assert_eq!(m.len(), 4);
        assert_eq!((m[0].player1, m[0].player2), (s[4], s[11]));
        assert_eq!((m[3].player1, m[3].player2), (s[7], s[8]));
        assert!(matches!(m[0].stage, CupStage::Qualification));
        // The pre-qualified are nowhere in round 1 — they are in the Swiss pool.
        let playing: HashSet<TournamentId> =
            m.iter().flat_map(|m| [m.player1, m.player2]).collect();
        assert!(s[..4].iter().all(|p| !playing.contains(p)));
    }

    #[test]
    fn qualifier_bracket_folds_the_prequalified_against_the_qualifiers() {
        let (cup, s) = qualifier_cup();
        let rounds = qualifier_rounds(&s);
        let m = cup.matches_for_round(&rounds[..1], 2).unwrap().matches;
        // The top seed draws the winner of the *weakest* qualification match, and
        // so on down — a plain fold of [pre-qualified ++ qualifiers].
        assert_eq!(m.len(), 4);
        assert_eq!((m[0].player1, m[0].player2), (s[0], s[7]));
        assert_eq!((m[1].player1, m[1].player2), (s[1], s[6]));
        assert_eq!((m[2].player1, m[2].player2), (s[2], s[5]));
        assert_eq!((m[3].player1, m[3].player2), (s[3], s[4]));
        assert!(matches!(m[0].stage, CupStage::Quarterfinal));
    }

    #[test]
    fn qualifier_bracket_view_keeps_the_playoff_in_its_own_column() {
        let (cup, s) = qualifier_cup();
        let v = cup.bracket_view(&qualifier_rounds(&s));
        // The bracket columns are the usual three; the play-off is beside them.
        assert_eq!(v.rounds.len(), 3);
        let q = v
            .qualification
            .expect("the qualifier format has a play-off");
        assert_eq!(q.len(), 4);
        assert_eq!((q[0].player1, q[0].player2), (Some(s[4]), Some(s[11])));
        assert_eq!(q[0].winner, Some(s[4]));
        assert!(matches!(q[0].stage, CupStage::Qualification));
        // Qualification match `j` feeds the second slot of `rounds[0][3 - j]`.
        assert_eq!(v.rounds[0][3].player2, q[0].winner);
        assert_eq!(v.rounds[0][0].player2, q[3].winner);
    }

    #[test]
    fn qualifier_podium_reads_the_final_a_round_later() {
        let (cup, s) = qualifier_cup();
        let podium = cup.podium(&qualifier_rounds(&s)).expect("final is decided");
        assert_eq!(podium.champion, Some(s[0]));
        assert_eq!(podium.runner_up, Some(s[1]));
        // The small final in the fixture is won by s[2].
        assert_eq!(podium.third, Some(s[2]));
        assert_eq!(podium.fourth, Some(s[3]));
    }

    #[test]
    fn a_direct_cup_has_no_qualification_column() {
        let cup = Cup {
            format: CupFormat::Direct,
            size: 8,
            seed_order: ids(8),
        };
        assert!(cup.bracket_view(&[]).qualification.is_none());
        assert_eq!(cup.field_size(), 8);
        assert_eq!(cup.cup_rounds(), 3);
    }

    #[test]
    fn a_double_no_show_in_the_playoff_gives_its_bracket_opponent_a_bye() {
        let (cup, s) = qualifier_cup();
        // Seeds 8 and 9 both fail to show: their play-off advances nobody, so the
        // seed drawn against that slot — the top seed — walks into round 2.
        let mut qualification = cup_round(
            1,
            &[
                (s[4], s[11], CupStage::Qualification),
                (s[5], s[10], CupStage::Qualification),
                (s[6], s[9], CupStage::Qualification),
                (s[7], s[8], CupStage::Qualification),
            ],
        );
        let dead = &mut qualification.boards[3];
        dead.outcome = Outcome::Forfeit {
            absent: Forfeit::Both(AbsenceKind::NoShow, AbsenceKind::NoShow),
        };

        let pairings = cup.matches_for_round(&[qualification], 2).unwrap();
        assert_eq!(pairings.byes, vec![(s[0], CupStage::Quarterfinal)]);
        assert_eq!(pairings.matches.len(), 3);
        assert_eq!(
            (pairings.matches[0].player1, pairings.matches[0].player2),
            (s[1], s[6])
        );
    }
}
