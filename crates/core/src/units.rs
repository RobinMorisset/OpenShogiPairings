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
//! deepest of those, SOSOS ([`crate::standings::Tiebreaks::sososm`] /
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

/// A count of games won, in **half-win units** (×2): a win is `2`, half a win
/// `1` — the same doubling [`HalfPoints`] uses, and for the same reason.
///
/// Halves are not decoration: a `0=` sit-out is worth half a point *and* half a
/// win (the EGF "number of wins" convention), so a whole count would have to
/// round it, and the Wins column would disagree with the Points column beside
/// it — which is exactly what it used to do. Everything summed *from* wins
/// (SOSW, SODOSW, SOSOSW, CUSSW and the dropped variants) inherits the halves,
/// so the arithmetic stays exact all the way up.
///
/// Still a type of its own rather than a second [`HalfPoints`]: a win count and
/// a point score are different quantities that happen to share a scale, and
/// mixing them up is precisely the mistake worth making impossible. Convert
/// deliberately through [`From<Wins>`], which is the one place the (now
/// one-to-one) relation is written.
///
/// The inner value is private — construct with [`from_whole`] / [`from_halves`]
/// and read the raw units with [`halves`], so the ×2 convention lives here and
/// nowhere else.
///
/// [`from_whole`]: Wins::from_whole
/// [`from_halves`]: Wins::from_halves
/// [`halves`]: Wins::halves
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
pub struct Wins(u32);

impl Wins {
    pub const ZERO: Wins = Wins(0);

    /// `whole` whole wins (doubled into half-win units).
    pub fn from_whole(whole: u32) -> Self {
        Wins(whole * 2)
    }

    /// `halves` raw half-win units (already ×2).
    pub fn from_halves(halves: u32) -> Self {
        Wins(halves)
    }

    /// The count in raw half-win units (×2) — for the wire and for ordinal
    /// ranking. Divide by 2 for a whole-win display.
    pub fn halves(self) -> u32 {
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

/// A win is worth a point, and half a win half a point — the single place that
/// conversion is written. Both are doubled, so this is a change of *quantity*,
/// not of scale.
impl From<Wins> for HalfPoints {
    fn from(w: Wins) -> Self {
        HalfPoints::from_halves(w.halves())
    }
}

/// A player's dense tournament number — the 1-based key the scoring tables are
/// indexed by (assigned at finalization). Distinct from a [`Uuid`](uuid::Uuid)
/// (the stable identity) and from any score quantity, so a raw number can't be
/// mistaken for one of those.
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

/// A team's dense number — the 1-based key the team score tables are indexed by,
/// assigned at finalization by descending average pairing rating, exactly as
/// [`TournamentId`] is for players. Distinct from the team's `Uuid` (its stable
/// identity, which survives a rename or a roster edit).
///
/// Like [`TournamentId`] it serializes as a bare number, exports to TypeScript as
/// a transparent `number`, and bridges to `usize` so the team tables can be
/// [`TiVec`](typed_index_collections::TiVec)s indexed by it directly.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display, Serialize, Deserialize, TS,
)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct TeamId(pub u32);

/// `usize` → `TeamId`, for `TiVec` key generation.
impl From<usize> for TeamId {
    fn from(i: usize) -> Self {
        TeamId(i as u32)
    }
}

/// `TeamId` → `usize`, what makes a `TiVec<TeamId, _>` indexable by a bare id.
impl From<TeamId> for usize {
    fn from(t: TeamId) -> Self {
        t.0 as usize
    }
}

/// The dense key of one **pairable unit** in the matching engine — a player in an
/// individual tournament, a team in a team tournament. The engine never reads a
/// [`Player`] or a [`Team`]: it is handed a [`PairingUnit`] table indexed by this
/// key and returns matched keys, so one implementation serves both modes.
///
/// Real units are numbered from 1, mirroring [`TournamentId`] / `TeamId` (the
/// wrappers pass those numbers straight through), which leaves `0` free for
/// [`PHANTOM`](Self::PHANTOM), the sentinel vertex a bye is matched to.
///
/// Serializes as a bare number like [`TournamentId`], because it is what the
/// pairing *explanations* ([`BoardLedger`], [`AffectedCycle`]) reference: a
/// client reads those keys as player numbers in individual mode and team numbers
/// in team mode, exactly as it reads the boards themselves.
///
/// [`Player`]: crate::player::Player
/// [`Team`]: crate::team::Team
/// [`PairingUnit`]: crate::pairing::PairingUnit
/// [`BoardLedger`]: crate::pairing::BoardLedger
/// [`AffectedCycle`]: crate::pairing::AffectedCycle
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display, Serialize, Deserialize, TS,
)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct UnitKey(pub u32);

impl UnitKey {
    /// The sentinel vertex standing in for the bye in a matching. Real units are
    /// numbered from 1, so it can never collide with one.
    pub const PHANTOM: UnitKey = UnitKey(0);
}

/// `usize` → `UnitKey`, for `TiVec` key generation.
impl From<usize> for UnitKey {
    fn from(i: usize) -> Self {
        UnitKey(i as u32)
    }
}

/// `UnitKey` → `usize`, what makes a `TiVec<UnitKey, _>` indexable by a bare key.
impl From<UnitKey> for usize {
    fn from(k: UnitKey) -> Self {
        k.0 as usize
    }
}

/// A player's number *is* its unit key in individual mode — the one place that
/// identification is written.
impl From<TournamentId> for UnitKey {
    fn from(t: TournamentId) -> Self {
        UnitKey(t.0)
    }
}

/// The inverse, for turning the engine's answer back into players. Only valid on
/// a key the player wrapper produced — [`UnitKey::PHANTOM`] maps to
/// `TournamentId(0)`, which is the same "no player" sentinel the counterfactual
/// API already uses on the wire.
impl From<UnitKey> for TournamentId {
    fn from(k: UnitKey) -> Self {
        TournamentId(k.0)
    }
}

/// A team's number *is* its unit key in team mode, the counterpart of the
/// [`TournamentId`] identification above.
impl From<TeamId> for UnitKey {
    fn from(t: TeamId) -> Self {
        UnitKey(t.0)
    }
}

/// The inverse, for turning the engine's answer back into teams. Only valid on a
/// key the team wrapper produced.
impl From<UnitKey> for TeamId {
    fn from(k: UnitKey) -> Self {
        TeamId(k.0)
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
/// raw unit — **half-units for both** [`Wins`] and [`HalfPoints`], so a bare `2`
/// is one win or one point. Assertions read `standing.points == 4` rather
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
