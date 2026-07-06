//! The tournament aggregate.
//!
//! For now a tournament is just a named list of players. Rounds, pairings and
//! results will be added here later; keeping the mutation logic in this crate
//! (rather than in the server) means the server, a future CLI, and the Tauri app
//! all share exactly one implementation.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::player::{NewPlayer, Player};

/// On-disk / on-the-wire format version for a serialized [`Tournament`].
///
/// Bumped whenever the saved shape changes incompatibly, so that loading an old
/// file can be detected (and, later, migrated) instead of silently mis-parsed.
pub const TOURNAMENT_FORMAT_VERSION: u32 = 1;

fn default_format_version() -> u32 {
    TOURNAMENT_FORMAT_VERSION
}

/// A tournament: a name and its registered players.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tournament {
    /// Format version of this record (see [`TOURNAMENT_FORMAT_VERSION`]).
    #[serde(default = "default_format_version")]
    pub format_version: u32,
    /// Stable unique identifier for the tournament.
    pub id: Uuid,
    /// Human-readable tournament name.
    pub name: String,
    /// Registered players, in registration order.
    #[serde(default)]
    pub players: Vec<Player>,
}

/// Errors that can arise while mutating a [`Tournament`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TournamentError {
    /// The tournament name was empty or whitespace-only.
    #[error("tournament name must not be empty")]
    EmptyTournamentName,
    /// A player name was empty or whitespace-only.
    #[error("player name must not be empty")]
    EmptyPlayerName,
    /// No player with the given id exists in this tournament.
    #[error("no player with id {0}")]
    PlayerNotFound(Uuid),
    /// The serialized record uses a format version this build cannot read.
    #[error("unsupported tournament format version {found} (this build supports {supported})")]
    UnsupportedFormatVersion { found: u32, supported: u32 },
}

impl Tournament {
    /// Create a new, empty tournament with the given name.
    ///
    /// Returns [`TournamentError::EmptyTournamentName`] if `name` is blank.
    pub fn new(name: &str) -> Result<Self, TournamentError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(TournamentError::EmptyTournamentName);
        }
        Ok(Self {
            format_version: TOURNAMENT_FORMAT_VERSION,
            id: Uuid::new_v4(),
            name: name.to_string(),
            players: Vec::new(),
        })
    }

    /// Register a new player and return a reference to the stored [`Player`].
    ///
    /// Returns [`TournamentError::EmptyPlayerName`] if the name is blank.
    pub fn add_player(&mut self, new: NewPlayer) -> Result<&Player, TournamentError> {
        if new.name.trim().is_empty() {
            return Err(TournamentError::EmptyPlayerName);
        }
        self.players.push(Player::from_new(new));
        // `push` never reallocates away the just-added last element.
        Ok(self.players.last().expect("just pushed a player"))
    }

    /// Remove the player with the given id.
    ///
    /// Returns [`TournamentError::PlayerNotFound`] if no such player exists.
    pub fn remove_player(&mut self, id: Uuid) -> Result<(), TournamentError> {
        let before = self.players.len();
        self.players.retain(|p| p.id != id);
        if self.players.len() == before {
            return Err(TournamentError::PlayerNotFound(id));
        }
        Ok(())
    }

    /// Validate a tournament that was deserialized from an untrusted source
    /// (an uploaded save file). Currently this only checks the format version.
    pub fn validate_loaded(&self) -> Result<(), TournamentError> {
        if self.format_version != TOURNAMENT_FORMAT_VERSION {
            return Err(TournamentError::UnsupportedFormatVersion {
                found: self.format_version,
                supported: TOURNAMENT_FORMAT_VERSION,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> NewPlayer {
        NewPlayer {
            name: name.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn new_rejects_blank_name() {
        assert_eq!(
            Tournament::new("   "),
            Err(TournamentError::EmptyTournamentName)
        );
    }

    #[test]
    fn add_player_trims_and_assigns_id() {
        let mut t = Tournament::new("Paris Open").unwrap();
        let player = t.add_player(named("  Alice  ")).unwrap().clone();
        assert_eq!(player.name, "Alice");
        assert_eq!(t.players.len(), 1);
        assert_eq!(t.players[0].id, player.id);
    }

    #[test]
    fn add_player_rejects_blank_name() {
        let mut t = Tournament::new("Paris Open").unwrap();
        assert_eq!(t.add_player(named("  ")), Err(TournamentError::EmptyPlayerName));
        assert!(t.players.is_empty());
    }

    #[test]
    fn empty_club_becomes_none() {
        let mut t = Tournament::new("Paris Open").unwrap();
        let p = t
            .add_player(NewPlayer {
                name: "Bob".into(),
                rating: Some(1500),
                club: Some("   ".into()),
            })
            .unwrap();
        assert_eq!(p.club, None);
        assert_eq!(p.rating, Some(1500));
    }

    #[test]
    fn remove_player_works_and_reports_missing() {
        let mut t = Tournament::new("Paris Open").unwrap();
        let id = t.add_player(named("Alice")).unwrap().id;
        assert!(t.remove_player(id).is_ok());
        assert!(t.players.is_empty());
        assert_eq!(t.remove_player(id), Err(TournamentError::PlayerNotFound(id)));
    }

    #[test]
    fn json_round_trip_is_stable() {
        let mut t = Tournament::new("Paris Open").unwrap();
        t.add_player(named("Alice")).unwrap();
        t.add_player(named("Bob")).unwrap();
        let json = serde_json::to_string(&t).unwrap();
        let back: Tournament = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
        back.validate_loaded().unwrap();
    }

    #[test]
    fn validate_loaded_rejects_future_version() {
        let mut t = Tournament::new("Paris Open").unwrap();
        t.format_version = 999;
        assert_eq!(
            t.validate_loaded(),
            Err(TournamentError::UnsupportedFormatVersion {
                found: 999,
                supported: TOURNAMENT_FORMAT_VERSION,
            })
        );
    }
}
