//! Players and player registration.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::units::TournamentId;

/// The two families of playing grade: dan (stronger, unbounded above) and kyu
/// (weaker, unbounded below).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum GradeKind {
    Dan,
    Kyu,
}

/// A dan/kyu playing grade (e.g. `3 dan`, `15 kyu`), used for grade-based
/// MacMahon thresholds alongside (or instead of) ELO-based ones — see
/// [`crate::settings::ThresholdCriterion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Grade {
    pub kind: GradeKind,
    /// 1-based level within the kind (e.g. `3` for "3 dan").
    pub level: u32,
}

impl Grade {
    pub fn dan(level: u32) -> Self {
        Grade {
            kind: GradeKind::Dan,
            level,
        }
    }

    pub fn kyu(level: u32) -> Self {
        Grade {
            kind: GradeKind::Kyu,
            level,
        }
    }

    /// A contiguous strength scale for comparison: `n` dan is `n`; `n` kyu is
    /// `1 - n`, so `1 kyu` (0) sits directly below `1 dan` (1) with no gap
    /// across the dan/kyu boundary, and a bigger value always means stronger.
    pub fn rank(self) -> i64 {
        match self.kind {
            GradeKind::Dan => self.level as i64,
            GradeKind::Kyu => 1 - self.level as i64,
        }
    }

    /// Parse the compact free-text grade entry used in registration and CSV
    /// import — `"3d"`, `"3 dan"`, `"5k"`, `"5 kyu"` (case-insensitive, optional
    /// space) — into a [`Grade`], or `None` if it doesn't match. The level must
    /// be a positive integer. Mirrors the frontend's `parseGrade`.
    pub fn parse(raw: &str) -> Option<Grade> {
        let s = raw.trim().to_lowercase();
        // Split the leading digits from the dan/kyu suffix.
        let digits_end = s.find(|c: char| !c.is_ascii_digit())?;
        if digits_end == 0 {
            return None; // must start with a level
        }
        let level: u32 = s[..digits_end].parse().ok()?;
        if level < 1 {
            return None;
        }
        let kind = match s[digits_end..].trim_start() {
            "d" | "dan" => GradeKind::Dan,
            "k" | "kyu" => GradeKind::Kyu,
            _ => return None,
        };
        Some(Grade { kind, level })
    }
}

/// Ordered by playing strength (see [`Grade::rank`]), not by field order —
/// `1 dan` is stronger than `1 kyu`, which the derived field-order comparison
/// (`Dan < Kyu`) would get backwards.
impl PartialOrd for Grade {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Grade {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

/// A player registered in a tournament.
///
/// Names follow the FESA convention: separate last and first names (last name is
/// the primary identifier). Only `last_name` is required; the rest is optional
/// metadata the pairing engine will eventually use (rating for seeding, club to
/// avoid pairing team-mates early). `nationality` is a country code (e.g. `JP`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Player {
    /// Stable unique identifier, assigned by the server on registration.
    pub id: Uuid,
    /// Human-facing tournament number, assigned when registration is finalized
    /// (or on registration if added afterwards). `None` until then. Used to
    /// reference opponents in the results table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tournament_id: Option<TournamentId>,
    /// Family name. Guaranteed non-empty (trimmed) once registered.
    pub last_name: String,
    /// Given name. May be empty.
    #[serde(default)]
    pub first_name: String,
    /// Optional playing strength / rating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rating: Option<u32>,
    /// Optional dan/kyu playing grade, independent of `rating` — used for
    /// grade-based MacMahon thresholds (see
    /// [`crate::settings::ThresholdCriterion`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub grade: Option<Grade>,
    /// Number of rated games behind `rating` in the FESA list, when the rating was
    /// picked from it. `None` means the rating was entered by hand or the player
    /// isn't in the list — in either case the ELO estimator treats the rating as
    /// provisional (see [`crate::estimate_elos`]). A `Some` below the reliability
    /// threshold is likewise provisional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fesa_games: Option<u32>,
    /// Optional country code (uppercase, e.g. `JP`, `FR`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub nationality: Option<String>,
    /// Optional club or federation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub club: Option<String>,
    /// Whether the referee has marked this player eligible for the direct-
    /// elimination cup (only meaningful when the cup is enabled). Set during
    /// registration; frozen into the bracket at finalization.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub eligible: bool,
    /// Manual point bonuses/maluses a referee has applied to this player (e.g. a
    /// fair-play bonus, or a correction). Each entry's `delta` is folded into the
    /// player's points alongside MacMahon starting points and victories (see
    /// [`crate::scoring::compute_scores`]), so it affects both standings and
    /// future pairing weight from the moment it is added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adjustments: Vec<PointAdjustment>,
}

/// A single manual point bonus (positive `delta`) or malus (negative `delta`)
/// applied to a player by a referee, with a mandatory human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct PointAdjustment {
    /// Stable id, so a specific entry can be removed later.
    pub id: Uuid,
    /// Points added (positive) or removed (negative). Never zero.
    pub delta: i32,
    /// Why the adjustment was made; shown to referees, required and non-blank.
    pub reason: String,
}

/// Data supplied when registering a player, before an id exists.
///
/// This is the request shape clients send; the server turns it into a [`Player`]
/// with a freshly minted [`Uuid`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct NewPlayer {
    pub last_name: String,
    #[serde(default)]
    #[ts(optional)]
    pub first_name: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub rating: Option<u32>,
    #[serde(default)]
    #[ts(optional)]
    pub grade: Option<Grade>,
    /// FESA #games behind the rating, when it was picked from the list (see
    /// [`Player::fesa_games`]).
    #[serde(default)]
    #[ts(optional)]
    pub fesa_games: Option<u32>,
    #[serde(default)]
    #[ts(optional)]
    pub nationality: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub club: Option<String>,
}

/// Trim a string; map empty to `None`.
fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

impl Player {
    /// Create a player with a new random id from registration data.
    ///
    /// The caller is responsible for validating `new.last_name` (see
    /// [`crate::Tournament::add_player`]); this constructor trims whitespace,
    /// uppercases the nationality code, and normalizes empty optionals to `None`.
    pub(crate) fn from_new(new: NewPlayer) -> Self {
        Self {
            id: Uuid::new_v4(),
            tournament_id: None,
            last_name: new.last_name.trim().to_string(),
            first_name: new.first_name.unwrap_or_default().trim().to_string(),
            rating: new.rating,
            grade: new.grade,
            // Only meaningful alongside a rating; drop it if the rating was cleared.
            fesa_games: new.rating.and(new.fesa_games),
            nationality: non_empty(new.nationality.unwrap_or_default()).map(|c| c.to_uppercase()),
            club: non_empty(new.club.unwrap_or_default()),
            eligible: false,
            adjustments: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grade_orders_across_the_dan_kyu_boundary_with_no_gap() {
        // Ascending strength: weak kyu → 1 kyu → 1 dan → strong dan.
        assert!(Grade::kyu(20) < Grade::kyu(1));
        assert!(Grade::kyu(1) < Grade::dan(1));
        assert!(Grade::dan(1) < Grade::dan(5));
        // Contiguous: 1 kyu sits directly below 1 dan.
        assert_eq!(Grade::kyu(1).rank() + 1, Grade::dan(1).rank());
    }

    #[test]
    fn parse_grade_accepts_the_compact_and_spelled_out_forms() {
        assert_eq!(Grade::parse("3d"), Some(Grade::dan(3)));
        assert_eq!(Grade::parse("3 dan"), Some(Grade::dan(3)));
        assert_eq!(Grade::parse("  5K "), Some(Grade::kyu(5)));
        assert_eq!(Grade::parse("5 kyu"), Some(Grade::kyu(5)));
        assert_eq!(Grade::parse("12DAN"), Some(Grade::dan(12)));
    }

    #[test]
    fn parse_grade_rejects_junk() {
        assert_eq!(Grade::parse(""), None);
        assert_eq!(Grade::parse("dan"), None); // no level
        assert_eq!(Grade::parse("5"), None); // no kind
        assert_eq!(Grade::parse("0d"), None); // level must be ≥ 1
        assert_eq!(Grade::parse("3 danger"), None); // suffix must be exact
        assert_eq!(Grade::parse("3 d an"), None);
    }
}
