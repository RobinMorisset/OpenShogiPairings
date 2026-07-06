//! Rounds and their pairings.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single board (game) in a round: two paired players.
///
/// The two slots are just "player 1" and "player 2" for now — colour
/// (sente/gote) assignment comes with smarter pairings later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub player1: Uuid,
    pub player2: Uuid,
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
}
