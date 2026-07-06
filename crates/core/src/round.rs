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

/// A piece-odds handicap, smallest to largest. The even game is the
/// absence of a handicap, so there is no `None` variant here. Serialized as the
/// short code used in the results table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handicap {
    #[serde(rename = "s")]
    Sente,
    #[serde(rename = "l")]
    Lance,
    #[serde(rename = "b")]
    Bishop,
    #[serde(rename = "r")]
    Rook,
    #[serde(rename = "rl")]
    RookLance,
    #[serde(rename = "2p")]
    TwoPiece,
    #[serde(rename = "4p")]
    FourPiece,
    #[serde(rename = "5p")]
    FivePiece,
    #[serde(rename = "6p")]
    SixPiece,
}

impl Handicap {
    /// All handicaps in increasing size — the order shown in the picker.
    pub const ALL: [Handicap; 9] = [
        Handicap::Sente,
        Handicap::Lance,
        Handicap::Bishop,
        Handicap::Rook,
        Handicap::RookLance,
        Handicap::TwoPiece,
        Handicap::FourPiece,
        Handicap::FivePiece,
        Handicap::SixPiece,
    ];

    /// Short code shown in the results table (matches the serialized form).
    pub fn code(self) -> &'static str {
        match self {
            Handicap::Sente => "s",
            Handicap::Lance => "l",
            Handicap::Bishop => "b",
            Handicap::Rook => "r",
            Handicap::RookLance => "rl",
            Handicap::TwoPiece => "2p",
            Handicap::FourPiece => "4p",
            Handicap::FivePiece => "5p",
            Handicap::SixPiece => "6p",
        }
    }

    /// Human-readable label shown in the handicap picker.
    pub fn label(self) -> &'static str {
        match self {
            Handicap::Sente => "Sente",
            Handicap::Lance => "Lance",
            Handicap::Bishop => "Bishop",
            Handicap::Rook => "Rook",
            Handicap::RookLance => "Rook+Lance",
            Handicap::TwoPiece => "2 pieces",
            Handicap::FourPiece => "4 pieces",
            Handicap::FivePiece => "5 pieces",
            Handicap::SixPiece => "6 pieces",
        }
    }
}

/// A handicap attached to a board. The `giver` (the higher-rated player) is
/// frozen when the handicap is set, so a later rating edit can't retroactively
/// flip who conceded the odds. A handicap game always counts as a win for the
/// giver in the standings, whatever the actual result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandicapGame {
    pub handicap: Handicap,
    pub giver: Winner,
}

/// `skip_serializing_if` helper — a plain `false` is the default and omitted.
fn is_false(b: &bool) -> bool {
    !*b
}

/// A single board (game) in a round: two paired players and, once played, a
/// result. `result` is `None` while the game hasn't been played yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub player1: Uuid,
    pub player2: Uuid,
    /// The *actual* winner of the game, used for end-of-tournament ELO and for
    /// the sign shown in the results cell. `None` until the game is decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Winner>,
    /// At least one draw (sennichite, repetition) occurred before the decisive game.
    /// The game is replayed to a win/loss for pairing, but the draw is recorded
    /// because it matters for ELO.
    #[serde(default, skip_serializing_if = "is_false")]
    pub drawn: bool,
    /// The handicap conceded on this board, if any (see [`HandicapGame`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handicap: Option<HandicapGame>,
    /// The float for this board: `points(player1) − points(player2)` at the time
    /// the round was paired. Frozen here so the float history stays correct even
    /// if MacMahon thresholds change later or an earlier result is edited — the
    /// score standings are recomputed live, but *who floated* is a fact of the
    /// pairing. `None` for boards from the naive pairer or from saves predating
    /// this field (the scorer then falls back to the live points difference).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points_diff: Option<i32>,
}

impl Board {
    /// The winner that counts for standings and pairing. For a handicap game
    /// that is always the giver (once the game is decided), regardless of who
    /// actually won; otherwise it is the actual result.
    pub fn effective_winner(&self) -> Option<Winner> {
        match &self.handicap {
            Some(h) => self.result.map(|_| h.giver),
            None => self.result,
        }
    }
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
