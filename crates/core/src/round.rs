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
    /// Players marked absent for this round (excluded from pairing). Recorded so
    /// the next round's draft can default to the same absentees, and so a
    /// deliberate absence is distinguishable from a late joiner.
    #[serde(default)]
    pub absent: Vec<Uuid>,
    /// Whether the round has been completed (all games played and locked in).
    /// A new round must not be started until the current one is completed.
    #[serde(default)]
    pub completed: bool,
}

/// A round being set up but not yet started (the `RoundDraft` state).
///
/// The referee customizes it — mark players absent, force specific pairings,
/// force the bye — and then confirms, which generates the pairings for the
/// remaining players and turns it into a real [`Round`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundDraft {
    /// The number the round will have once confirmed.
    pub number: u32,
    /// Players marked absent (excluded from pairing).
    #[serde(default)]
    pub absent: Vec<Uuid>,
    /// Pairings the referee has fixed by hand (the `result` field is unused
    /// here). Remaining present players are paired automatically.
    #[serde(default)]
    pub forced_boards: Vec<Board>,
    /// A player forced to take the bye (only valid with an odd present count).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forced_bye: Option<Uuid>,
}
