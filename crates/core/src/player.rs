//! Players and player registration.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A player registered in a tournament.
///
/// Only `name` is required; `rating` and `club` are optional metadata that the
/// pairing engine will eventually use (e.g. rating for initial seeding, club to
/// avoid pairing team-mates in early rounds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    /// Stable unique identifier, assigned by the server on registration.
    pub id: Uuid,
    /// Display name. Guaranteed non-empty (trimmed) once registered.
    pub name: String,
    /// Optional playing strength / rating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<u32>,
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
    pub name: String,
    #[serde(default)]
    pub rating: Option<u32>,
    #[serde(default)]
    pub club: Option<String>,
}

impl Player {
    /// Create a player with a new random id from registration data.
    ///
    /// The caller is responsible for validating `new.name` (see
    /// [`crate::Tournament::add_player`]); this constructor trims surrounding
    /// whitespace and normalizes an all-whitespace/empty club to `None`.
    pub(crate) fn from_new(new: NewPlayer) -> Self {
        let club = new
            .club
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty());
        Self {
            id: Uuid::new_v4(),
            name: new.name.trim().to_string(),
            rating: new.rating,
            club,
        }
    }
}
