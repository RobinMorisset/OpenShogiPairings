//! Rounds and their pairings.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::units::{HalfPoints, TournamentId};

/// Which player won a board. Colour (sente/gote) is chosen at random per game
/// and isn't tracked, so the result is simply which of the two players won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum Winner {
    Player1,
    Player2,
}

/// A piece-odds handicap, smallest to largest. The even game is the
/// absence of a handicap, so there is no `None` variant here. Serialized as the
/// short code used in the results table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
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

    /// Parse a handicap from its short [`code`](Self::code), e.g. when reading a
    /// results cell back in. Returns `None` for an unknown code.
    pub fn from_code(code: &str) -> Option<Handicap> {
        Handicap::ALL.into_iter().find(|h| h.code() == code)
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
/// flip who conceded the odds. When the "Wiel" rule is on (off by default — see
/// [`crate::settings::TournamentSettings::handicap_wiel_rule`]), a handicap
/// game always counts as a win for the giver in the standings, whatever the
/// actual result; when it's off (the default), the actual result counts as
/// normal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct HandicapGame {
    pub handicap: Handicap,
    pub giver: Winner,
}

/// `skip_serializing_if` helper — a plain `false` is the default and omitted.
fn is_false(b: &bool) -> bool {
    !*b
}

/// `skip_serializing_if` helper — a `0` float (the not-applicable default) is
/// omitted.
fn is_zero(n: &i32) -> bool {
    *n == 0
}

/// Which stage of the direct-elimination cup a board belongs to. `RoundOf(n)`
/// covers the early bracket rounds (round of 64/32/16); the last three rounds are
/// named explicitly. Used to label cup games in the pairings view.
///
/// `Qualification` is the play-off round that precedes the bracket in the
/// qualifier format ([`CupFormat::Qualifier`]) — it needs its own name because it
/// has the same number of players as bracket round 1 and would otherwise be
/// labelled with the same `RoundOf(n)`.
///
/// [`CupFormat::Qualifier`]: crate::cup::CupFormat::Qualifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum CupStage {
    Qualification,
    RoundOf(u32),
    Quarterfinal,
    Semifinal,
    Final,
    SmallFinal,
}

/// How a board's pairing was decided, surfaced to clients so the pairings view can
/// flag each game. Serialized internally-tagged, e.g. `{"kind":"swiss"}` or
/// `{"kind":"cup","stage":{"round_of":32}}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PairingSource {
    /// Produced by the Swiss / MacMahon matching engine.
    #[default]
    Swiss,
    /// Fixed by the referee (a forced pairing).
    Forced,
    /// Dictated by the direct-elimination cup bracket (bypasses the engine).
    Cup { stage: CupStage },
}

impl PairingSource {
    /// `skip_serializing_if` helper — Swiss is the default and omitted from JSON.
    fn is_swiss(&self) -> bool {
        matches!(self, PairingSource::Swiss)
    }
}

/// Which side(s) of a board failed to show up. A single side is a forfeit — the
/// opponent takes the point exactly like a bye — while `Both` means neither
/// player appeared, so no winner can be determined (both take a zero loss, and a
/// cup game with this outcome advances nobody). Serialized snake_case, so the
/// single-side variants stay wire-compatible with the earlier `Winner`-typed
/// field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum NoShow {
    Player1,
    Player2,
    Both,
}

/// A single board (game) in a round: two paired players and, once played, a
/// result. `result` is `None` while the game hasn't been played yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Board {
    /// The two players, by **tournament number** (`Player::tournament_id`), the
    /// dense per-tournament key — not the registration `Uuid`. Rounds are only
    /// created after finalization, when every player has a number, so scoring and
    /// pairing index players directly without a `Uuid → number` lookup.
    pub player1: TournamentId,
    pub player2: TournamentId,
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
    /// pairing. Set for every scored board — Swiss, forced and cup alike — so a
    /// bracket or referee pairing that crosses score groups still shapes float
    /// history. `0` only on referee draft placeholders, which are re-created with
    /// the real value when the round is paired.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub points_diff: i32,
    /// How this pairing was decided (Swiss / referee-forced / cup). Defaults to
    /// Swiss for saves predating this field.
    #[serde(default, skip_serializing_if = "PairingSource::is_swiss")]
    pub source: PairingSource,
    /// A player (or both) failed to show up. A single side is a forfeit — the
    /// opponent is credited the point exactly as for a bye — while `Both` means
    /// neither appeared, so no winner exists. Kept separate from
    /// [`result`](Self::result) (which stays `None` — no game was actually
    /// played) so the cross-table and American grid can show it distinctly (`0#`
    /// for an absentee, `0+` for a player who showed up), and so it never feeds
    /// the ELO estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_show: Option<NoShow>,
    /// This board is a "long game": double time control, lasting two rounds, and
    /// its winner scores two points instead of one. Off by default and omitted
    /// from JSON when false. The two players sit out the next round's pairing
    /// while an undecided long board is in flight. See `docs/two-round-boards.md`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub long: bool,
}

impl Board {
    /// A not-yet-played board between two players, tagged with how it was paired
    /// and (for engine pairings) its frozen float.
    pub fn pending(
        player1: TournamentId,
        player2: TournamentId,
        points_diff: i32,
        source: PairingSource,
    ) -> Self {
        Board {
            player1,
            player2,
            result: None,
            drawn: false,
            handicap: None,
            points_diff,
            source,
            no_show: None,
            long: false,
        }
    }

    /// Whether this board's outcome is settled: either a game was played (a
    /// `result` is recorded) or a player was marked as a no-show. Drives the
    /// round's `completed` flag, so a no-show counts toward closing the round
    /// even though no game was played.
    pub fn is_decided(&self) -> bool {
        self.result.is_some() || self.no_show.is_some()
    }

    /// A long board whose game hasn't finished yet — the state that makes its two
    /// players sit out the next round's pairing and shows the `0-` placeholder in
    /// the cross-table. A long board that is already decided (finished early, or
    /// resolved by forfeit) is not pending, so its players are free again.
    pub fn long_pending(&self) -> bool {
        self.long && !self.is_decided()
    }

    /// Whether this board counts as settled for the purpose of closing its round.
    /// A pending long board does **not** hold the round open — the rest of the
    /// field plays on while the long game (which spans this round and the next)
    /// finishes — so it counts as complete here even without a result.
    pub fn complete_for_round(&self) -> bool {
        self.is_decided() || self.long
    }

    /// Whether the given `side` failed to show up on this board (true for that
    /// side under a single no-show, and for both sides under [`NoShow::Both`]).
    pub fn no_show_absent(&self, side: Winner) -> bool {
        match self.no_show {
            Some(NoShow::Player1) => side == Winner::Player1,
            Some(NoShow::Player2) => side == Winner::Player2,
            Some(NoShow::Both) => true,
            None => false,
        }
    }

    /// The id of the player who *did* show up — the one credited the free point,
    /// exactly like a bye — when exactly one side was a no-show. `None` when both
    /// showed up or [`NoShow::Both`] (neither did, so nobody is credited).
    pub fn no_show_opponent(&self) -> Option<TournamentId> {
        match self.no_show {
            Some(NoShow::Player1) => Some(self.player2),
            Some(NoShow::Player2) => Some(self.player1),
            _ => None,
        }
    }

    /// The winner that counts for standings and pairing. For a handicap game,
    /// when the "Wiel" rule ([`TournamentSettings::handicap_wiel_rule`]) is on,
    /// that is always the giver (once the game is decided), regardless of who
    /// actually won; otherwise — and for any non-handicap game — it is the
    /// actual result.
    ///
    /// [`TournamentSettings::handicap_wiel_rule`]: crate::settings::TournamentSettings::handicap_wiel_rule
    pub fn effective_winner(&self, wiel_rule: bool) -> Option<Winner> {
        match &self.handicap {
            Some(h) if wiel_rule => self.result.map(|_| h.giver),
            _ => self.result,
        }
    }

    /// The loser (effective) of a decided board, if any — the side that isn't the
    /// effective winner. `None` while the board is unplayed.
    pub fn effective_loser(&self, wiel_rule: bool) -> Option<TournamentId> {
        self.effective_winner(wiel_rule).map(|w| match w {
            Winner::Player1 => self.player2,
            Winner::Player2 => self.player1,
        })
    }

    /// The player id of the effective winner, if the board is decided.
    pub fn winner_id(&self, wiel_rule: bool) -> Option<TournamentId> {
        self.effective_winner(wiel_rule).map(|w| match w {
            Winner::Player1 => self.player1,
            Winner::Player2 => self.player2,
        })
    }
}

/// What a round is worth to a player who played no board — the `0+` / `0=` /
/// `0−` shown in their cross-table cell. A bye starts at [`Full`](Self::Full)
/// and an absence at the tournament's
/// [`half_point_absences`](crate::settings::TournamentSettings::half_point_absences)
/// default, but the referee can set any of the three per round and per player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum SitoutValue {
    /// `0−`: nothing.
    Zero,
    /// `0=`: half a point, but not a victory.
    Half,
    /// `0+`: a full point, counted as a victory (like a played win).
    Full,
}

impl SitoutValue {
    /// The score this is worth.
    pub fn points(self) -> HalfPoints {
        match self {
            SitoutValue::Zero => HalfPoints::ZERO,
            SitoutValue::Half => HalfPoints::from_halves(1),
            SitoutValue::Full => HalfPoints::from_whole(1),
        }
    }

    /// Whether this counts as a victory. Only a full point does — a `0=` is
    /// score without a win, so it never lifts the Wins column or the
    /// victory-based tie-breaks (SOSW, SODOSW, CUSSW).
    pub fn is_victory(self) -> bool {
        matches!(self, SitoutValue::Full)
    }

    /// The cross-table / American Grid cell for this value.
    pub fn cell(self) -> &'static str {
        match self {
            SitoutValue::Zero => "0-",
            SitoutValue::Half => "0=",
            SitoutValue::Full => "0+",
        }
    }
}

/// Why a player has no board in a round. Distinct from [`SitoutValue`]: the kind
/// records what happened, the value what it scores, and the referee can edit the
/// latter without rewriting the former. Only the kind feeds the pairing engine
/// (a bye is a downfloat that shouldn't repeat), so re-scoring a past cell can
/// never reshape a later round's pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum SitoutKind {
    /// Marked absent by the referee, so excluded from the pairing entirely.
    Absent,
    /// Present but left over: the engine gave them the bye (an odd field).
    Bye,
    /// The referee fixed the bye on this player.
    ForcedBye,
    /// Advanced through the cup bracket unopposed — the rare case where the
    /// player they would have faced never materialized (both players of the
    /// feeding match were no-shows).
    CupBye { stage: CupStage },
}

impl SitoutKind {
    /// Whether this is a bye of any sort (as opposed to an absence): the player
    /// was in the tournament that round but had nobody to play. A bye is a
    /// downfloat and shouldn't be handed out twice, whatever it ended up
    /// scoring.
    pub fn is_bye(self) -> bool {
        !matches!(self, SitoutKind::Absent)
    }
}

/// A player with no board in a round: why they sat out, and what it scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Sitout {
    pub player: TournamentId,
    pub kind: SitoutKind,
    pub value: SitoutValue,
}

/// One round of the tournament: the boards, plus everyone who played no board —
/// the byes and the absentees ([`Sitout`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Round {
    /// 1-based round number.
    pub number: u32,
    pub boards: Vec<Board>,
    /// Everyone who played no board this round: the bye (when the field is odd),
    /// any referee-forced or cup byes, and the players marked absent. Each
    /// carries what the round scored them, frozen when the round was confirmed
    /// and editable afterwards from the standings.
    ///
    /// A player marked absent who nonetheless has a board — the cup bracket
    /// pairs its players regardless of the absent set, so the referee can record
    /// the forfeit — is *not* listed here: they played a board, and it is the
    /// board that scores them.
    #[serde(default)]
    pub sitouts: Vec<Sitout>,
    /// Whether the round has been completed (all games played and locked in).
    /// A new round must not be started until the current one is completed.
    #[serde(default)]
    pub completed: bool,
}

impl Round {
    /// Whether every board of this round is settled enough to close the round: a
    /// board is either decided or a long board still running (which spans into the
    /// next round and so must not hold this one open). This is the single source
    /// of truth for the [`completed`](Self::completed) flag — note it means a
    /// completed round may still contain an undecided (long) board.
    pub fn is_complete(&self) -> bool {
        self.boards.iter().all(|b| b.complete_for_round())
    }

    /// This player's sit-out, if they played no board this round.
    pub fn sitout(&self, player: TournamentId) -> Option<&Sitout> {
        self.sitouts.iter().find(|s| s.player == player)
    }

    /// The players marked absent this round — the set the next round's draft
    /// defaults to, and what tells a deliberate absence from a late joiner.
    pub fn absentees(&self) -> impl Iterator<Item = TournamentId> + '_ {
        self.sitouts
            .iter()
            .filter(|s| !s.kind.is_bye())
            .map(|s| s.player)
    }

    /// Everyone who took a bye of any kind this round.
    pub fn byes(&self) -> impl Iterator<Item = TournamentId> + '_ {
        self.sitouts
            .iter()
            .filter(|s| s.kind.is_bye())
            .map(|s| s.player)
    }

    /// The bye the *engine* chose, if any — the phantom edge of the round's
    /// matching, and so the only bye the pairing explanations can reason about.
    /// There is at most one by construction (a matching has one phantom).
    pub fn swiss_bye(&self) -> Option<TournamentId> {
        self.sitouts
            .iter()
            .find(|s| s.kind == SitoutKind::Bye)
            .map(|s| s.player)
    }

    /// The byes the referee fixed by hand — the ones re-pairing the round must
    /// carry over (an engine-chosen bye goes back up for grabs).
    pub fn forced_byes(&self) -> impl Iterator<Item = TournamentId> + '_ {
        self.sitouts
            .iter()
            .filter(|s| s.kind == SitoutKind::ForcedBye)
            .map(|s| s.player)
    }
}

/// A round being set up but not yet started (the `RoundDraft` state).
///
/// The referee customizes it — mark players absent, force specific pairings,
/// force byes — and then confirms, which generates the pairings for the
/// remaining players and turns it into a real [`Round`].
///
/// These are the pairing *inputs* only: what each sit-out ends up scoring is
/// decided when the round is confirmed (see [`Sitout`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct RoundDraft {
    /// The number the round will have once confirmed.
    pub number: u32,
    /// Players marked absent (excluded from pairing).
    #[serde(default)]
    pub absent: Vec<TournamentId>,
    /// Pairings the referee has fixed by hand (the `result` field is unused
    /// here). Remaining present players are paired automatically.
    #[serde(default)]
    pub forced_boards: Vec<Board>,
    /// Players forced to take a bye. Usually empty (the engine picks the bye
    /// when the field is odd) or a single player; several are allowed, and the
    /// engine still adds its own bye if what's left over is odd.
    #[serde(default)]
    pub forced_byes: Vec<TournamentId>,
}
