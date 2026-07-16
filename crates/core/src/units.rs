//! Strongly-typed score quantities.
//!
//! The results tables juggle two kinds of integer that must never be mixed: a
//! **win count** ([`Wins`]) and a score in **half-point units** ([`HalfPoints`],
//! stored ×2 so a half-point stays an exact integer). Giving each its own type
//! makes the compiler reject adding wins to half-points, and makes the ×2
//! half-point convention impossible to cross by accident — [`HalfPoints`] hides
//! its inner value, so the only ways in and out are its named constructors and
//! [`halves`](HalfPoints::halves).
//!
//! Both serialize as a bare JSON number and export to TypeScript as `number`, so
//! they stay wire- and TS-compatible with the plain `u32` they replace — no
//! save-format or frontend change. This falls out of serde's default *newtype
//! struct* handling (a one-field tuple struct forwards to its inner value), so
//! **do not add `#[serde(transparent)]`**: it is redundant here and only makes
//! ts-rs emit a "failed to parse serde attribute" warning. The
//! `serializes_as_a_bare_number` test pins this representation.
//!
//! # Why `u32` and not `u16`
//!
//! `u16` looks tempting — a player's own score maxes at roughly `2·rounds`
//! half-points, tiny. But these units also hold the tie-break sums, and the
//! deepest of those, SOSOS ([`crate::standings::Standing::sososm`] /
//! `sososw`), is a *sum of opponents' SOS*, i.e. a third-order sum that grows as
//! ~`rounds³`:
//!
//! ```text
//! points/player ≈ 2·R          (win every round; R = rounds)
//! sos           ≈ R · 2R  = 2R²    (Σ over opponents of their points)
//! sosos         ≈ R · 2R² = 2R³    (Σ over opponents of their SOS)
//! ```
//!
//! `2R³` passes `u16::MAX` (65 535) at only **R ≈ 33 rounds**, and sooner in
//! practice: MacMahon starting points raise everyone's base score, and a long
//! board counts an opponent twice *and* pays double, so both the summand and the
//! multiplier inflate. Overflow in a release build **wraps silently** (only debug
//! panics), which would mis-rank a tournament rather than crash — the worst
//! failure mode for a pairing tool. `u32` pushes that ceiling out past ~1300
//! rounds, and buys nothing to shrink: on the wire these are JSON numbers
//! regardless of width, and in memory the difference is a few KB across a whole
//! field. So: keep `u32`.

use derive_more::{Add, AddAssign, Display, Sum};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A whole count of games won. Adds and sums with other [`Wins`]; it has no
/// cross-type arithmetic with [`HalfPoints`] on purpose, so a win count can't be
/// silently treated as a (doubled) point score.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Add,
    AddAssign,
    Sum,
    Display,
    Serialize,
    Deserialize,
    TS,
)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Wins(pub u32);

impl Wins {
    pub const ZERO: Wins = Wins(0);

    /// The underlying whole win count.
    pub fn get(self) -> u32 {
        self.0
    }
}

/// A score in **half-point units** (×2): a whole point is `2`, a half-point `1`.
/// Kept doubled so a half-point (a `0=` absence, or half a bye) stays an exact
/// integer. The inner value is private — construct with [`from_whole`] /
/// [`from_halves`] and read the raw units with [`halves`], so the ×2 convention
/// lives here and nowhere else.
///
/// [`from_whole`]: HalfPoints::from_whole
/// [`from_halves`]: HalfPoints::from_halves
/// [`halves`]: HalfPoints::halves
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Add,
    AddAssign,
    Sum,
    Serialize,
    Deserialize,
    TS,
)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct HalfPoints(u32);

impl HalfPoints {
    pub const ZERO: HalfPoints = HalfPoints(0);

    /// A score of `whole` whole points (doubled into half-point units).
    pub fn from_whole(whole: u32) -> Self {
        HalfPoints(whole * 2)
    }

    /// A score of `halves` raw half-point units (already ×2).
    pub fn from_halves(halves: u32) -> Self {
        HalfPoints(halves)
    }

    /// The score in raw half-point units (×2) — for the wire, the cross-table
    /// (`Pts` column) and ordinal ranking. Divide by 2 for a whole-point display.
    pub fn halves(self) -> u32 {
        self.0
    }
}

/// A win is worth two half-points — the single place that conversion is written.
impl From<Wins> for HalfPoints {
    fn from(w: Wins) -> Self {
        HalfPoints::from_whole(w.0)
    }
}

/// A player's dense tournament number — the 1-based key the scoring tables are
/// indexed by (assigned at finalization). Distinct from a [`Uuid`] (the stable
/// identity) and from any score quantity, so a raw number can't be mistaken for
/// one of those.
///
/// This is the type the per-number score tables ([`crate::scoring::Scores`], and
/// the opponent/defeated lists) are keyed by. The [`From<usize>`] / [`Into`]
/// `usize` bridge below is what lets those tables be
/// [`TiVec`](typed_index_collections::TiVec)s indexed directly by a
/// `TournamentId` — so `table[tid]` needs no `as usize` at the call site; the one
/// cast lives here.
///
/// It is also the player reference carried across the wire — on [`crate::Board`],
/// [`crate::Round`], [`crate::Player::tournament_id`], … — so it derives serde and
/// [`TS`]. Like [`Wins`] / [`HalfPoints`], it serializes as a **bare number** (a
/// one-field tuple struct forwards to its inner value, so **no**
/// `#[serde(transparent)]` — that only trips a ts-rs warning) and exports to
/// TypeScript as a transparent `number` alias; the `serializes_as_a_bare_number`
/// test pins this. [`Display`] prints the plain number for the cross-table.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display, Serialize, Deserialize, TS,
)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct TournamentId(pub u32);

/// `usize` → `TournamentId`, for `TiVec` key generation (e.g. `push`).
impl From<usize> for TournamentId {
    fn from(i: usize) -> Self {
        TournamentId(i as u32)
    }
}

/// `TournamentId` → `usize`, the single place the index cast is written — this is
/// what makes a `TiVec<TournamentId, _>` indexable by a bare `TournamentId`.
impl From<TournamentId> for usize {
    fn from(t: TournamentId) -> Self {
        t.0 as usize
    }
}

/// Test-only convenience: compare a [`TournamentId`] against a bare `u32` literal,
/// so the scoring tests keep reading `opponents == vec![btid, btid]` (with `btid`
/// a plain `u32` tournament number) after the lists became `Vec<TournamentId>`.
/// `Vec<A>: PartialEq<Vec<B>>` holds whenever `A: PartialEq<B>`, so this covers
/// the list assertions too. Gated on `test`; production code stays strict.
#[cfg(test)]
impl PartialEq<u32> for TournamentId {
    fn eq(&self, other: &u32) -> bool {
        self.0 == *other
    }
}

/// Test-only convenience: compare a score against a bare integer literal in its
/// natural unit ([`Wins`] a whole count, [`HalfPoints`] raw half-units), so the
/// assertions in the scoring/standings tests read `standing.points == 4` rather
/// than `standing.points.halves() == 4`. Gated on `test` so production code
/// keeps the strict typing — you still can't compare the two units to each other
/// or to a stray `u32` outside tests.
#[cfg(test)]
impl PartialEq<u32> for Wins {
    fn eq(&self, other: &u32) -> bool {
        self.0 == *other
    }
}

#[cfg(test)]
impl PartialEq<u32> for HalfPoints {
    fn eq(&self, other: &u32) -> bool {
        self.0 == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frontend (and saved tournaments) rely on these being bare numbers on
    /// the wire, not `{"0": 5}`. This is serde's default newtype-struct
    /// behaviour, achieved *without* `#[serde(transparent)]` — guard it so a
    /// refactor can't silently wrap them in an object.
    #[test]
    fn serializes_as_a_bare_number() {
        assert_eq!(serde_json::to_string(&Wins(5)).unwrap(), "5");
        assert_eq!(
            serde_json::to_string(&HalfPoints::from_whole(2)).unwrap(),
            "4"
        );
        assert_eq!(serde_json::to_string(&TournamentId(7)).unwrap(), "7");
        // ...and round-trips back.
        assert_eq!(serde_json::from_str::<Wins>("5").unwrap(), Wins(5));
        assert_eq!(
            serde_json::from_str::<HalfPoints>("4").unwrap(),
            HalfPoints::from_halves(4)
        );
        assert_eq!(
            serde_json::from_str::<TournamentId>("7").unwrap(),
            TournamentId(7)
        );
    }
}
