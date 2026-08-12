//! Rounds and their pairings.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::units::{HalfPoints, TeamId, TournamentId, Wins};

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

/// Why a missing side missed the board.
///
/// The distinction only exists because a team match is played whether or not
/// every member turns up: a member who fell ill still has a board, and stamping
/// it with the unjustified `0#` would put the wrong thing in the record. In an
/// individual tournament a justified absence never reaches a board — the player
/// is excluded from the pairing and gets a sit-out instead — which is why
/// [`Justified`](Self::Justified) is rejected at load time outside team mode.
///
/// Neither kind scores the missing player anything, and neither ever feeds the
/// ELO estimate; they differ only in the exported cell and the honest record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum AbsenceKind {
    /// Failed to appear, unjustified — the `0#` cell. The default, and the only
    /// kind an individual tournament can produce.
    #[default]
    NoShow,
    /// Absent for a valid reason (illness, a departure) — the `0-` cell.
    Justified,
}

impl AbsenceKind {
    /// The cross-table / American Grid cell for a side that missed the board
    /// this way.
    pub fn cell(self) -> &'static str {
        match self {
            AbsenceKind::NoShow => "0#",
            AbsenceKind::Justified => "0-",
        }
    }
}

/// Which side(s) of a board missed it, and why each of them did.
///
/// A single missing side is a forfeit — the opponent takes the point exactly
/// like a bye — while `Both` means neither player appeared, so no winner can be
/// determined (both take a zero loss, and a cup game with this outcome advances
/// nobody). Every state is meaningful by construction: at least one side is
/// missing, and each missing side has exactly one reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum Forfeit {
    Player1(AbsenceKind),
    Player2(AbsenceKind),
    Both(AbsenceKind, AbsenceKind),
}

impl Forfeit {
    /// The forfeit that has `side` missing for `kind` and the other side
    /// present — the common single-sided case.
    pub fn one(side: Winner, kind: AbsenceKind) -> Forfeit {
        match side {
            Winner::Player1 => Forfeit::Player1(kind),
            Winner::Player2 => Forfeit::Player2(kind),
        }
    }

    /// The forfeit described by a reason per side, or `None` when both sides
    /// turned up (which is not a forfeit at all). The single place the two
    /// per-side controls of the round view are folded into one value.
    pub fn of(player1: Option<AbsenceKind>, player2: Option<AbsenceKind>) -> Option<Forfeit> {
        match (player1, player2) {
            (Some(a), Some(b)) => Some(Forfeit::Both(a, b)),
            (Some(a), None) => Some(Forfeit::Player1(a)),
            (None, Some(b)) => Some(Forfeit::Player2(b)),
            (None, None) => None,
        }
    }

    /// Why `side` missed the board, or `None` if they turned up.
    pub fn kind(self, side: Winner) -> Option<AbsenceKind> {
        match (self, side) {
            (Forfeit::Player1(k), Winner::Player1) => Some(k),
            (Forfeit::Player2(k), Winner::Player2) => Some(k),
            (Forfeit::Both(k, _), Winner::Player1) => Some(k),
            (Forfeit::Both(_, k), Winner::Player2) => Some(k),
            _ => None,
        }
    }

    /// Whether `side` missed the board.
    pub fn absent(self, side: Winner) -> bool {
        self.kind(side).is_some()
    }

    /// The side that *did* turn up, when exactly one did — the one credited the
    /// free point. `None` under `Both`, where nobody is.
    pub fn present(self) -> Option<Winner> {
        match self {
            Forfeit::Player1(_) => Some(Winner::Player2),
            Forfeit::Player2(_) => Some(Winner::Player1),
            Forfeit::Both(..) => None,
        }
    }

    /// Whether any side missed for a justified reason — the load-time check that
    /// keeps the kind out of individual tournaments, where it cannot arise.
    pub fn has_justified(self) -> bool {
        [Winner::Player1, Winner::Player2]
            .into_iter()
            .any(|s| self.kind(s) == Some(AbsenceKind::Justified))
    }
}

/// What happened on a board — the single field replacing the former
/// `result` × `no_show` × `drawn` product.
///
/// As three sibling fields, states that cannot occur were representable and only
/// excluded by convention (a result recorded on a forfeited board, a draw on a
/// board nobody turned up for), which the ELO reader in particular got wrong.
/// The sum type makes them unrepresentable instead.
///
/// `drawn` lives only in the variants where play happened: the American grid
/// cannot express "forfeit after draws", so neither does the type.
///
/// Serialized internally-tagged, like [`PairingSource`], e.g. `{"kind":"won",
/// "winner":"player1"}`; the whole field is omitted from JSON while the board is
/// an ordinary pending one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// No decision yet. `drawn` records at least one draw (sennichite) before
    /// the decisive replay still in progress — it matters for ELO.
    Pending {
        #[serde(default, skip_serializing_if = "is_false")]
        drawn: bool,
    },
    /// Played to a decision, possibly after one or more draws.
    Won {
        winner: Winner,
        #[serde(default, skip_serializing_if = "is_false")]
        drawn: bool,
    },
    /// Side(s) failed to appear, so no game was played. The present side (if
    /// exactly one) is credited the point exactly like a bye; never feeds ELO.
    /// The payload records *why* each missing side missed (see [`Forfeit`]).
    Forfeit { absent: Forfeit },
}

impl Default for Outcome {
    /// A fresh board: nothing played, no draw.
    fn default() -> Self {
        Outcome::PENDING
    }
}

impl Outcome {
    /// An untouched board — what [`Board::pending`] starts from.
    pub const PENDING: Outcome = Outcome::Pending { drawn: false };

    /// A board played straight to a decision, with no draw along the way.
    pub const fn won(winner: Winner) -> Outcome {
        Outcome::Won {
            winner,
            drawn: false,
        }
    }

    /// The *actual* winner of the game (ignoring the Wiel rule, which is
    /// [`Board::effective_winner`]'s business). `None` unless the board was
    /// played to a decision — a forfeit has no winner here, only a beneficiary
    /// (see [`Board::no_show_opponent`]).
    pub fn winner(self) -> Option<Winner> {
        match self {
            Outcome::Won { winner, .. } => Some(winner),
            Outcome::Pending { .. } | Outcome::Forfeit { .. } => None,
        }
    }

    /// Whether at least one draw occurred before the decisive replay. Always
    /// false on a forfeit, where no game was played at all.
    pub fn drawn(self) -> bool {
        match self {
            Outcome::Pending { drawn } | Outcome::Won { drawn, .. } => drawn,
            Outcome::Forfeit { .. } => false,
        }
    }

    /// Which side(s) failed to appear, if this board was forfeited.
    pub fn forfeit(self) -> Option<Forfeit> {
        match self {
            Outcome::Forfeit { absent } => Some(absent),
            Outcome::Pending { .. } | Outcome::Won { .. } => None,
        }
    }

    /// Whether the outcome is settled: played to a decision, or forfeited.
    pub fn is_decided(self) -> bool {
        !matches!(self, Outcome::Pending { .. })
    }

    /// `skip_serializing_if` helper — an untouched board is the default and
    /// omitted from JSON.
    fn is_pending_undrawn(&self) -> bool {
        *self == Outcome::PENDING
    }
}

/// A single board (game) in a round: two paired players and, once played, a
/// result. See [`Outcome`] for the states a board can be in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Board {
    /// The two players, by **tournament number** (`Player::tournament_id`), the
    /// dense per-tournament key — not the registration `Uuid`. Rounds are only
    /// created after finalization, when every player has a number, so scoring and
    /// pairing index players directly without a `Uuid → number` lookup.
    pub player1: TournamentId,
    pub player2: TournamentId,
    /// What happened on this board: still to play, played to a decision, or
    /// forfeited by one or both sides. Omitted from JSON while pending.
    #[serde(default, skip_serializing_if = "Outcome::is_pending_undrawn")]
    pub outcome: Outcome,
    /// The handicap conceded on this board, if any (see [`HandicapGame`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handicap: Option<HandicapGame>,
    /// The float for this board: `points(player1) − points(player2)` at the time
    /// the round was paired, **in half-points** — the unit [`HalfPoints`] keeps
    /// scores in, so an ordinary one-point float is `±2` here. Only the sign
    /// matters to the float history; anything displaying the size has to halve
    /// it. Frozen here so the float history stays correct even
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
            outcome: Outcome::PENDING,
            handicap: None,
            points_diff,
            source,
            long: false,
        }
    }

    /// Whether this board's outcome is settled: either a game was played (a
    /// winner is recorded) or a player was marked as a no-show. Drives the
    /// round's `completed` flag, so a no-show counts toward closing the round
    /// even though no game was played.
    pub fn is_decided(&self) -> bool {
        self.outcome.is_decided()
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
    /// side under a single forfeit, and for both sides under
    /// [`Forfeit::Both`]).
    pub fn no_show_absent(&self, side: Winner) -> bool {
        self.outcome.forfeit().is_some_and(|f| f.absent(side))
    }

    /// Why `side` missed this board, if they did — what the exported cell reads
    /// (`0#` for an unjustified no-show, `0-` for a justified absence).
    pub fn absence_kind(&self, side: Winner) -> Option<AbsenceKind> {
        self.outcome.forfeit().and_then(|f| f.kind(side))
    }

    /// The id of the player who *did* show up — the one credited the free point,
    /// exactly like a bye — when exactly one side was a no-show. `None` when both
    /// showed up or [`Forfeit::Both`] (neither did, so nobody is credited).
    pub fn no_show_opponent(&self) -> Option<TournamentId> {
        self.outcome.forfeit()?.present().map(|side| match side {
            Winner::Player1 => self.player1,
            Winner::Player2 => self.player2,
        })
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
            Some(h) if wiel_rule => self.outcome.winner().map(|_| h.giver),
            _ => self.outcome.winner(),
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

    /// What this counts as in the Wins column and the victory-based tie-breaks
    /// (SOSW, SODOSW, SOSOSW, CUSSW).
    ///
    /// Deliberately the same shape as [`points`](Self::points): a `0=` is worth
    /// half a point *and* half a win, following the EGF "number of wins"
    /// convention. It used to be worth half a point and no win at all, which
    /// left the Wins column reading `0` beside a Points column reading `1½` —
    /// the two describing the same round and disagreeing about it.
    pub fn wins(self) -> Wins {
        match self {
            SitoutValue::Zero => Wins::ZERO,
            SitoutValue::Half => Wins::from_halves(1),
            SitoutValue::Full => Wins::from_whole(1),
        }
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

/// A team match the referee has fixed by hand, for a team round's draft — the
/// team-level counterpart of a forced [`Board`].
///
/// The two teams are named by number, and the match expands to its `size`
/// boards when the round is confirmed, exactly as an engine-chosen one does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct ForcedMatch {
    pub team1: TeamId,
    pub team2: TeamId,
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
    /// Team matches the referee has fixed by hand — **team mode only**, where
    /// teams are what get paired, so a forced pairing names two teams rather
    /// than two players. Empty (and absent from JSON) otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forced_matches: Vec<ForcedMatch>,
    /// Teams forced to take a bye — team mode only, for the same reason. The
    /// engine still byes one more team if what's left over is odd.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forced_team_byes: Vec<TeamId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire shape of each [`Outcome`] variant, and that every one survives a
    /// round trip. The board's whole `outcome` key disappears while pending, so a
    /// fresh board's JSON is exactly what it was before the sum type.
    #[test]
    fn outcome_round_trips_through_json() {
        let cases = [
            (Outcome::PENDING, r#"{"kind":"pending"}"#),
            (
                Outcome::Pending { drawn: true },
                r#"{"kind":"pending","drawn":true}"#,
            ),
            (
                Outcome::won(Winner::Player2),
                r#"{"kind":"won","winner":"player2"}"#,
            ),
            (
                Outcome::Won {
                    winner: Winner::Player1,
                    drawn: true,
                },
                r#"{"kind":"won","winner":"player1","drawn":true}"#,
            ),
            (
                Outcome::Forfeit {
                    absent: Forfeit::Both(AbsenceKind::NoShow, AbsenceKind::NoShow),
                },
                r#"{"kind":"forfeit","absent":{"both":["no_show","no_show"]}}"#,
            ),
            // A single missing side, absent for a reason — the team-mode cell.
            (
                Outcome::Forfeit {
                    absent: Forfeit::Player1(AbsenceKind::Justified),
                },
                r#"{"kind":"forfeit","absent":{"player1":"justified"}}"#,
            ),
        ];
        for (outcome, json) in cases {
            assert_eq!(serde_json::to_string(&outcome).unwrap(), json);
            assert_eq!(serde_json::from_str::<Outcome>(json).unwrap(), outcome);
        }
    }

    /// A pending board serializes without an `outcome` key at all, and reads back
    /// as pending — the `skip_serializing_if` / `default` pair that keeps saves
    /// from growing an entry per unplayed board.
    #[test]
    fn a_pending_board_omits_its_outcome() {
        let board = Board::pending(TournamentId(1), TournamentId(2), 0, PairingSource::Swiss);
        let json = serde_json::to_string(&board).unwrap();
        assert_eq!(json, r#"{"player1":1,"player2":2}"#);
        assert_eq!(serde_json::from_str::<Board>(&json).unwrap(), board);
    }
}
