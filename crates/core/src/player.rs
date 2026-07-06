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
            last_name: new.last_name.trim().to_string(),
            first_name: new.first_name.trim().to_string(),
            rating: new.rating,
            nationality: non_empty(new.nationality.unwrap_or_default()).map(|c| c.to_uppercase()),
            club: non_empty(new.club.unwrap_or_default()),
        }
    }
}
