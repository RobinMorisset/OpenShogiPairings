//! Rounds and their pairings.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Which player won a board. Colour (sente/gote) is chosen at random per game
/// and isn't tracked, so the result is simply which of the two players won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Winner {
    Player1,
    Player2,
}

/// A single board (game) in a round: two paired players and, once played, a
/// result. `result` is `None` while the game hasn't been played yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub player1: Uuid,
    pub player2: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Winner>,
}

/// One round of the tournament: the boards, plus the player sitting out (a bye)
/// when there is an odd number of players.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Round {
    /// 1-based round number.
    pub number: u32,
    pub boards: Vec<Board>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bye: Option<Uuid>,
    /// Whether the round has been completed (all games played and locked in).
    /// A new round must not be started until the current one is completed.
    #[serde(default)]
    pub completed: bool,
}
