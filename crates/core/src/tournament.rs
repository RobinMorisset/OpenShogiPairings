//! The tournament aggregate.
//!
//! For now a tournament is just a named list of players. Rounds, pairings and
//! results will be added here later; keeping the mutation logic in this crate
//! (rather than in the server) means the server, a future CLI, and the Tauri app
//! all share exactly one implementation.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pairing::pair_round;
use crate::player::{NewPlayer, Player};
use crate::round::Round;

/// On-disk / on-the-wire format version for a serialized [`Tournament`].
///
/// Bumped whenever the saved shape changes incompatibly, so that loading an old
/// file can be detected (and, later, migrated) instead of silently mis-parsed.
///
/// v2: players carry `last_name` + `first_name` + `nationality` instead of a
/// single `name`.
/// v3: tournaments carry a list of `rounds`.
pub const TOURNAMENT_FORMAT_VERSION: u32 = 3;

fn default_format_version() -> u32 {
    TOURNAMENT_FORMAT_VERSION
}

/// Minimum number of players required to start a round.
pub const MIN_PLAYERS_PER_ROUND: usize = 2;

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
    /// Rounds played so far, in order.
    #[serde(default)]
    pub rounds: Vec<Round>,
}

/// Errors that can arise while mutating a [`Tournament`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TournamentError {
    /// The tournament name was empty or whitespace-only.
    #[error("tournament name must not be empty")]
    EmptyTournamentName,
    /// A player's last name was empty or whitespace-only.
    #[error("player last name must not be empty")]
    EmptyPlayerName,
    /// No player with the given id exists in this tournament.
    #[error("no player with id {0}")]
    PlayerNotFound(Uuid),
    /// Too few players to start a round.
    #[error("need at least {needed} players to start a round (have {have})")]
    NotEnoughPlayers { needed: usize, have: usize },
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
            rounds: Vec::new(),
        })
    }

    /// Register a new player and return a reference to the stored [`Player`].
    ///
    /// Returns [`TournamentError::EmptyPlayerName`] if the last name is blank
    /// (the last name is the required identifier; the first name is optional).
    pub fn add_player(&mut self, new: NewPlayer) -> Result<&Player, TournamentError> {
        if new.last_name.trim().is_empty() {
            return Err(TournamentError::EmptyPlayerName);
        }
        self.players.push(Player::from_new(new));
        // `push` never reallocates away the just-added last element.
        Ok(self.players.last().expect("just pushed a player"))
    }

    /// Replace the editable fields of an existing player, keeping its id and
    /// position. Used for in-place cell editing.
    ///
    /// Returns [`TournamentError::EmptyPlayerName`] if the new last name is blank
    /// or [`TournamentError::PlayerNotFound`] if no player has that id.
    pub fn edit_player(
        &mut self,
        id: Uuid,
        new: NewPlayer,
    ) -> Result<&Player, TournamentError> {
        if new.last_name.trim().is_empty() {
            return Err(TournamentError::EmptyPlayerName);
        }
        let player = self
            .players
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or(TournamentError::PlayerNotFound(id))?;
        // Reuse the normalization in `from_new`, but keep the existing id.
        let normalized = Player::from_new(new);
        player.last_name = normalized.last_name;
        player.first_name = normalized.first_name;
        player.rating = normalized.rating;
        player.nationality = normalized.nationality;
        player.club = normalized.club;
        Ok(player)
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

    /// Start the next round: compute its pairings from the current players and
    /// append it. Returns the new [`Round`].
    ///
    /// Requires at least [`MIN_PLAYERS_PER_ROUND`] players.
    pub fn start_round(&mut self) -> Result<&Round, TournamentError> {
        if self.players.len() < MIN_PLAYERS_PER_ROUND {
            return Err(TournamentError::NotEnoughPlayers {
                needed: MIN_PLAYERS_PER_ROUND,
                have: self.players.len(),
            });
        }
        let number = self.rounds.len() as u32 + 1;
        let ids: Vec<Uuid> = self.players.iter().map(|p| p.id).collect();
        self.rounds.push(pair_round(number, &ids));
        Ok(self.rounds.last().expect("just pushed a round"))
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

    fn named(last_name: &str) -> NewPlayer {
        NewPlayer {
            last_name: last_name.to_string(),
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
        assert_eq!(player.last_name, "Alice");
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
                last_name: "Bob".into(),
                rating: Some(1500),
                club: Some("   ".into()),
                nationality: Some("fr".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(p.club, None);
        assert_eq!(p.rating, Some(1500));
        assert_eq!(p.nationality.as_deref(), Some("FR")); // trimmed + uppercased
    }

    #[test]
    fn edit_player_updates_fields_and_keeps_id() {
        let mut t = Tournament::new("Paris Open").unwrap();
        let id = t.add_player(named("Alice")).unwrap().id;

        let edited = t
            .edit_player(
                id,
                NewPlayer {
                    last_name: "Alice".into(),
                    first_name: "Anne".into(),
                    rating: Some(1600),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(edited.id, id); // id preserved
        assert_eq!(edited.first_name, "Anne");
        assert_eq!(edited.rating, Some(1600));
    }

    #[test]
    fn edit_player_rejects_blank_name_and_missing_id() {
        let mut t = Tournament::new("Paris Open").unwrap();
        let id = t.add_player(named("Alice")).unwrap().id;
        assert_eq!(
            t.edit_player(id, named("  ")),
            Err(TournamentError::EmptyPlayerName)
        );
        let missing = uuid::Uuid::new_v4();
        assert_eq!(
            t.edit_player(missing, named("Bob")),
            Err(TournamentError::PlayerNotFound(missing))
        );
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
    fn start_round_pairs_players_and_numbers_rounds() {
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B", "C"] {
            t.add_player(named(name)).unwrap();
        }
        let round = t.start_round().unwrap();
        assert_eq!(round.number, 1);
        assert_eq!(round.boards.len(), 1); // 3 players → 1 board + 1 bye
        assert!(round.bye.is_some());

        let round2 = t.start_round().unwrap();
        assert_eq!(round2.number, 2);
        assert_eq!(t.rounds.len(), 2);
    }

    #[test]
    fn start_round_needs_enough_players() {
        let mut t = Tournament::new("Cup").unwrap();
        t.add_player(named("Solo")).unwrap();
        assert_eq!(
            t.start_round(),
            Err(TournamentError::NotEnoughPlayers { needed: 2, have: 1 })
        );
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
