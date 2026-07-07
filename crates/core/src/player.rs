//! Players and player registration.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A player registered in a tournament.
///
/// Names follow the FESA convention: separate last and first names (last name is
/// the primary identifier). Only `last_name` is required; the rest is optional
/// metadata the pairing engine will eventually use (rating for seeding, club to
/// avoid pairing team-mates early). `nationality` is a country code (e.g. `JP`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    /// Stable unique identifier, assigned by the server on registration.
    pub id: Uuid,
    /// Human-facing tournament number, assigned when registration is finalized
    /// (or on registration if added afterwards). `None` until then. Used to
    /// reference opponents in the results table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tournament_id: Option<u32>,
    /// Family name. Guaranteed non-empty (trimmed) once registered.
    pub last_name: String,
    /// Given name. May be empty.
    #[serde(default)]
    pub first_name: String,
    /// Optional playing strength / rating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<u32>,
    /// Optional country code (uppercase, e.g. `JP`, `FR`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nationality: Option<String>,
    /// Optional club or federation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewPlayer {
    pub last_name: String,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub rating: Option<u32>,
    #[serde(default)]
    pub nationality: Option<String>,
    #[serde(default)]
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
            first_name: new.first_name.trim().to_string(),
            rating: new.rating,
            nationality: non_empty(new.nationality.unwrap_or_default()).map(|c| c.to_uppercase()),
            club: non_empty(new.club.unwrap_or_default()),
            eligible: false,
            adjustments: Vec::new(),
        }
    }
}
