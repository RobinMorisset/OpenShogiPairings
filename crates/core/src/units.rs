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
//! Both serialize transparently (as a bare JSON number) and export to TypeScript
//! as `number`, so they stay wire- and TS-compatible with the plain `u32` they
//! replace — no save-format or frontend change.

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
#[serde(transparent)]
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
#[serde(transparent)]
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
