//! The tournament aggregate.
//!
//! A tournament is its players and teams, its settings, and the rounds played so
//! far, with every mutation that moves it forward. Keeping that logic in this
//! crate (rather than in the server) means the server, the simulator and the
//! Tauri app all share exactly one implementation.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::cup::{cup_field_size, Cup, CupBracketView, CupPairings, CupPodium, CUP_SIZES};
use crate::pairing::{
    counterfactual_forbid, counterfactual_force, explain_pairing, pair_round_weighted,
    player_units, Counterfactual, CounterfactualMode, PairingUnit, RoundExplanation, ScopeReason,
    PHANTOM,
};
use crate::player::{NewPlayer, Player, PointAdjustment};
use crate::round::{
    Board, ForcedMatch, Forfeit, GameRecord, Handicap, HandicapGame, Outcome, PairingSource, Round,
    RoundDraft, Sitout, SitoutKind, SitoutValue, Winner,
};
use crate::scoring::compute_scores;
use crate::settings::{TeamModeConflict, TournamentSettings, TEAM_SIZES};
use crate::standings::{compute_standings, Standing};
use crate::team::Team;
use crate::team_scoring::{
    compute_team_standings, matches_in_round, swiss_bye_team, TeamSlots, TeamStanding,
};
use crate::units::{TeamId, TournamentId, UnitKey};
use typed_index_collections::TiVec;

/// On-disk / on-the-wire format version for a serialized [`Tournament`].
///
/// Bumped whenever the saved shape changes incompatibly, so that loading an old
/// file can be detected (and, later, migrated) instead of silently mis-parsed.
///
/// v2: players carry `last_name` + `first_name` + `nationality` instead of a
/// single `name`.
/// v3: tournaments carry a list of `rounds`.
/// v4: players carry a list of manual point `adjustments`.
/// v5: rounds carry one `sitouts` list (each with what it scores) in place of
/// the separate `bye`, `cup_byes` and `absent` fields.
/// v6: boards carry one `outcome` sum in place of the separate `result`,
/// `drawn` and `no_show` fields.
/// v7: team tournaments — a `teams` roster list and `settings.teams`.
/// v8: a board's forfeit records *why* each missing side missed it.
/// v9: teams carry manual point `adjustments`.
/// v10: rounds carry the `explanation` of their pairing, frozen at confirmation,
/// and the tournament an `explanations_faithful_through` watermark.
/// v11: boards carry one `record` sum (`short` / `long_start` / `long_carried` /
/// `long_end`, each but the third holding the outcome) in place of the separate
/// `outcome` field and `long` flag.
///
/// A save is normally only readable at the exact version this build writes. The
/// one exception is v5 — what v1.1.0, v1.2.0 and v1.3.0 all wrote — whose
/// **not-yet-started** tournaments the server upgrades on load; see
/// `UPGRADABLE_FROM` in `crates/server/src/save.rs` for the window and why it
/// stops at the first round. A tournament that has not started has no board, so
/// the v11 board shape is not part of what that upgrade has to translate.
pub const TOURNAMENT_FORMAT_VERSION: u32 = 11;

/// Minimum number of players required to start a round.
pub(crate) const MIN_PLAYERS_PER_ROUND: usize = 2;

/// Minimum number of present teams required to start a team round — the team
/// reading of [`MIN_PLAYERS_PER_ROUND`], since teams are what get paired.
/// Crate-internal: only `team.rs`'s own guard reads it.
pub(crate) const MIN_TEAMS_PER_ROUND: usize = 2;

/// A tournament: a name and its registered players.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(deny_unknown_fields)]
pub struct Tournament {
    /// Format version of this record (see [`TOURNAMENT_FORMAT_VERSION`]).
    ///
    /// Required, deliberately. It used to default to "whatever this build
    /// writes", which is the one answer a file with no version cannot support:
    /// the field exists to say which shape the rest of the bytes are in, so
    /// assuming the current one turns "I don't know" into a confident misread.
    pub format_version: u32,
    /// Stable unique identifier for the tournament.
    pub id: Uuid,
    /// Human-readable tournament name.
    pub name: String,
    /// Tournament-wide settings (MacMahon groups, …). Defaulted so older saves
    /// that predate it load with no MacMahon.
    #[serde(default)]
    pub settings: TournamentSettings,
    /// Registered players, in registration order.
    #[serde(default)]
    pub players: Vec<Player>,
    /// Whether registration has been finalized (a prerequisite for round 1).
    #[serde(default)]
    pub registration_finalized: bool,
    /// The round currently being set up but not yet started, if any. Its
    /// presence is the `RoundDraft` state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<RoundDraft>,
    /// Rounds played so far, in order.
    #[serde(default)]
    pub rounds: Vec<Round>,
    /// The direct-elimination cup, if this is a hybrid tournament. Fixed at
    /// finalization (see [`Cup`]); `None` when there is no cup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cup: Option<Cup>,
    /// The registered teams, in creation order — the only team state that is
    /// *stored*, everything else about a team being replayed from the boards.
    /// Empty (and absent from JSON) outside team mode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<Team>,
    /// How far the frozen [`Round::explanation`]s still match the tournament a
    /// reader is looking at: rounds `1..=n` are faithful, every round above `n`
    /// is to be shown with a "the data behind this has changed since" warning.
    /// `0` means none are.
    ///
    /// The explanations themselves stay permanently faithful to the pairing they
    /// describe (that is the point of freezing them), but the *present* moves:
    /// the stored round-3 ledger may cite a score that a later correction to
    /// round 2 has since changed. Round `r` is paired from `rounds[..r-1]`, so an
    /// edit inside round `k` can only disturb rounds *after* `k` — which makes
    /// the faithful rounds a prefix, and this watermark decreasing-only:
    /// [`min(mark, k)`](Self::explanations_stale_after) on a per-round edit and
    /// [`0`](Self::explanations_all_stale) on a player or settings edit, which
    /// are global. Round 1 is never disturbed by anything.
    ///
    /// Lives here rather than in server session state because, unlike the
    /// undo/redo `version`, it must survive save/load, travel with a mailed save
    /// file, and be restored by undo and by a backup restore alongside the state
    /// it describes.
    ///
    /// Not defaulted on load, for the same reason [`Round::explanation`] is not:
    /// a save that lacks it predates explanations entirely, and guessing a
    /// watermark for it would vouch for ledgers that aren't there. The one older
    /// save this build reads (v5, see [`TOURNAMENT_FORMAT_VERSION`]) has no
    /// rounds by construction, so the upgrade supplies the only honest value,
    /// `0` — it does not default it, which would just as happily paper over a
    /// hand-edited current save that dropped the field.
    pub explanations_faithful_through: u32,
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

    #[error("no category with id {0}")]
    CategoryNotFound(Uuid),
    /// Registration has already been finalized.
    #[error("registration is already finalized")]
    RegistrationAlreadyFinalized,
    /// Registration must be finalized before starting rounds.
    #[error("registration must be finalized first")]
    RegistrationNotFinalized,
    /// A new round cannot start until the current one is completed.
    #[error("the current round must be completed first")]
    PreviousRoundNotComplete,
    /// A round is already being drafted.
    #[error("a round is already being prepared")]
    DraftAlreadyExists,
    /// An operation needs a draft round in progress, but there is none.
    #[error("no round is being prepared")]
    NoDraft,
    /// Too few present (non-absent) players to start a round.
    #[error("need at least {needed} present players (have {have})")]
    NotEnoughPresentPlayers { needed: usize, have: usize },
    /// The draft's constraints are inconsistent (see the message).
    #[error("invalid round setup: {0}")]
    InvalidDraft(String),
    /// There is no round (or draft) to cancel.
    #[error("no round to cancel")]
    NoRoundToCancel,
    /// There is no current (in-progress) round to act on (e.g. to re-pair).
    #[error("there is no current round")]
    NoCurrentRound,
    /// A pairing can't be forced onto a round that already has recorded results.
    #[error("cannot re-pair a round that already has recorded results")]
    RoundHasResults,
    /// The clicked board already has a result, so it cannot be made long: the
    /// flag doubles what a game is worth, and that game has been played.
    #[error("this game already has a result, so it can no longer be made long")]
    LongFlagAfterResult,
    /// One of the *other* boards the flag moves with already has a result. A cup
    /// round is long or short as a whole — including, in a qualifier cup's first
    /// round, the pre-qualified players' games — so the referee clicked an empty
    /// board and was still refused, which needs saying rather than implying.
    #[error("another game in this cup round already has a result")]
    LongFlagAfterCoupledResult,
    /// A board's "long game" flag can only be changed on the current round.
    #[error("a long game can only be set on the current round")]
    NotCurrentRound,
    /// A long game must be resolved before the record that depends on it can be
    /// used: the American Grid renders its result in the column of the round it
    /// was finished in, and there is nothing to put there yet.
    #[error("the long game from round {round} must be resolved first")]
    UnresolvedLongGame { round: u32 },
    /// A board was addressed in the round its long game *started*, where the
    /// record is inert. The game is finished in `round`, and everything about it
    /// — result, handicap — belongs to the record there.
    #[error("this long game is finished in round {round}; record it there")]
    CarriedLongGame { round: u32 },
    /// A long game's length was decided in the round it started, so it cannot be
    /// changed in the round it is finished in. The two records only mean anything
    /// as a pair, and demoting one of them would orphan the other.
    #[error("this long game was started in round {round}; its length is fixed")]
    LongGameStartedEarlier { round: u32 },
    /// A loaded file has half of a carried long game: a starting record with no
    /// live one after it, a live one with no starting record before it, or a
    /// start that was never carried although a later round exists. See
    /// `docs/long-boards-v2.md`.
    #[error("round {round} holds half of a carried long game")]
    OrphanedLongGame { round: u32 },
    /// No round with the given number exists.
    #[error("no round number {0}")]
    RoundNotFound(u32),
    /// No board with the given index exists in the round.
    #[error("no board {board} in round {round}")]
    BoardNotFound { round: u32, board: usize },
    /// A draw was recorded on a board that was forfeited — nobody played, so
    /// nobody drew. The UI disables the control, so this is a client bug.
    #[error("board {board} of round {round} was forfeited, so it cannot be a draw")]
    DrawnOnForfeitedBoard { round: u32, board: usize },
    /// A sit-out's value was set for a player who played a board that round (or
    /// wasn't in it at all), so there is no sit-out to score.
    #[error("player {player} did not sit out round {round}")]
    PlayerNotSittingOut { round: u32, player: TournamentId },
    /// A handicap was requested for two players whose ratings are equal (or both
    /// unrated), so there is no unambiguous handicap giver.
    #[error("a handicap game needs two players with different ratings")]
    HandicapNeedsRatingDifference,
    /// A handicap was requested for a cup (direct-elimination) board — cup games
    /// are always played even.
    #[error("cup games cannot have a handicap")]
    HandicapNotAllowedForCup,
    /// A handicap was set on a board that was forfeited — nobody played, so
    /// nobody conceded odds. The UI disables the control, so this is a client
    /// bug.
    #[error("board {board} of round {round} was forfeited, so it cannot have a handicap")]
    HandicapOnForfeitedBoard { round: u32, board: usize },
    /// The serialized record uses a format version this build cannot read.
    #[error("unsupported tournament format version {found} (this build supports {supported})")]
    UnsupportedFormatVersion { found: u32, supported: u32 },
    /// The save is from an older format this build *can* upgrade, but only for a
    /// tournament that hasn't started — and this one had rounds played, or a
    /// round in preparation. See `UPGRADABLE_FROM` in the server's `save` module
    /// for why the upgrade stops at the first round.
    #[error(
        "this save is from an older version (format {found}) and its tournament was already \
         under way; only one that has not started a round can be upgraded to format {supported}"
    )]
    OldSaveAlreadyStarted { found: u32, supported: u32 },
    /// The bytes couldn't be parsed as a tournament save at all (not even far
    /// enough to read its format version).
    #[error("malformed tournament save: {0}")]
    MalformedSave(String),
    /// Two players in the file share a registration id.
    #[error("player id {player} appears more than once")]
    DuplicatePlayerId { player: Uuid },
    /// Two players in the file share a tournament number — the key every score
    /// is stored by, so they would share one score.
    #[error("tournament number {number} is used by more than one player")]
    DuplicateTournamentNumber { number: TournamentId },
    /// Registration is finalized but a player has no tournament number, which
    /// everything downstream assumes they have.
    #[error("player {player} has no tournament number, but registration is finalized")]
    UnnumberedPlayer { player: Uuid },
    /// The rounds in the file are not numbered `1..=n` in order.
    #[error("expected round {expected} at this position, found round {found}")]
    MisnumberedRound { expected: u32, found: u32 },
    /// A board or sit-out names a tournament number no player in the file has.
    #[error("round {round} names tournament number {player}, who is not in this tournament")]
    UnknownRoundPlayer { round: u32, player: TournamentId },
    /// A board pairs a player with themselves.
    #[error("round {round} pairs player {player} against themselves")]
    BoardAgainstSelf { round: u32, player: TournamentId },
    /// A player takes part in one round more than once — on two boards, in two
    /// sit-outs, or on a board *and* in a sit-out.
    ///
    /// A round is one thing per player: one game, or one reason there was no
    /// game. Every count downstream assumes it — a player listed absent twice is
    /// paid the absence twice (invisible while an absence is worth nothing,
    /// which is the default, and free points the moment `half_point_absences` is
    /// on), a player on two boards plays two games in one round, and the
    /// cross-table grows a second cell for a round that has one.
    #[error("round {round} has player {player} taking part more than once")]
    PlayerTwiceInRound { round: u32, player: TournamentId },
    /// The ELO estimate is enabled but nothing anchors its scale: every player is
    /// on a flat prior and none is pinned, so the estimate has no absolute
    /// reference (see [`crate::elo::has_scale_anchor`]).
    #[error(
        "the ELO estimate has no scale anchor: add a rated player, pin ratings \
         (K multiplier 0), or use a non-flat unrated prior"
    )]
    EloEstimateUnanchored,
    /// The cup is enabled but no size was chosen at finalization.
    #[error("choose a cup size to finalize (the cup is enabled)")]
    CupSizeRequired,
    /// The chosen cup size is not one of the supported powers of two.
    #[error("invalid cup size {size} (must be one of 8, 16, 32, 64)")]
    InvalidCupSize { size: u32 },
    /// Fewer eligible players than the chosen cup size.
    #[error("need at least {needed} eligible players for the cup (have {have})")]
    NotEnoughEligiblePlayers { needed: u32, have: usize },
    /// A loaded cup's seed list is the wrong length for its bracket size — the
    /// two are frozen together at finalization, and the bracket replay folds one
    /// against the other without re-checking.
    #[error("a cup bracket of {size} needs {expected} seeds (found {found})")]
    CupSeedCountMismatch {
        size: u32,
        expected: u32,
        found: usize,
    },
    /// A loaded cup seeds the same player into two bracket slots.
    #[error("cup seed {seed} appears more than once")]
    DuplicateCupSeed { seed: TournamentId },
    /// A loaded cup seeds a tournament number no player in the file carries.
    #[error("cup seed {seed} is not a player of this tournament")]
    UnknownCupSeed { seed: TournamentId },
    /// A player who is seeded in the cup bracket cannot be removed.
    #[error("cannot remove a player seeded in the cup")]
    CannotRemoveCupPlayer,
    /// A player who has already played a game (appears on a board of a started
    /// round) cannot be removed — their results are referenced by every opponent's
    /// score record, so erasing them would corrupt those tie-breaks. Mark them
    /// absent for future rounds instead.
    #[error("cannot remove a player who has already played a game")]
    CannotRemovePlayedPlayer,
    /// In team mode the pairing unit is the team, not the player. Once
    /// registration is finalized every team holds exactly `team_size` members
    /// and every player is in one, so removing a player on their own could only
    /// leave a short roster — which would silently play a short match. Remove
    /// the whole team, or mark the player absent.
    #[error("cannot remove a player individually from a team tournament")]
    CannotRemoveTeamPlayer,
    /// A team that has been paired into a match cannot be removed: every
    /// opponent's score record references it, exactly as for a player who has
    /// played. This is [`Self::CannotRemovePlayedPlayer`] one layer up.
    #[error("cannot remove a team that has already played a match")]
    CannotRemoveMatchedTeam,
    /// The cup bracket referenced an earlier result that is missing (internal
    /// inconsistency — should not happen for a properly gated round).
    #[error("the cup bracket is missing an earlier result")]
    CupBracketInconsistent,
    /// A manual point adjustment was requested with a blank reason.
    #[error("a point adjustment needs a reason")]
    EmptyAdjustmentReason,
    /// A manual point adjustment of zero was requested (it would have no effect).
    #[error("a point adjustment must not be zero")]
    ZeroPointAdjustment,
    /// No point adjustment with the given id exists for that player.
    #[error("no point adjustment {adjustment} for player {player}")]
    AdjustmentNotFound { player: Uuid, adjustment: Uuid },

    // --- Team tournaments ---
    /// Team mode was enabled alongside a feature it cannot support (or such a
    /// feature was turned on while team mode was already active). Neither side is
    /// silently disabled: the referee is told which two settings disagree and
    /// picks.
    #[error("team mode does not support {} — turn one of the two off", .0.describe())]
    TeamModeConflict(TeamModeConflict),
    /// The configured team size is outside [`TEAM_SIZES`].
    #[error("invalid team size {size} (must be between 2 and 9)")]
    InvalidTeamSize { size: u32 },
    /// Team mode, or the team size, was changed after registration was
    /// finalized — both reshape every roster and every future match.
    #[error("team mode and team size are fixed once registration is finalized")]
    TeamSettingsLocked,
    /// A team operation was attempted on an individual tournament.
    #[error("this is not a team tournament")]
    NotATeamTournament,
    /// No team with the given id exists in this tournament.
    #[error("no team with id {0}")]
    TeamNotFound(Uuid),
    /// A team's name was empty or whitespace-only.
    #[error("team name must not be empty")]
    EmptyTeamName,
    /// Another team already has that name (compared ignoring case).
    #[error("a team named {0} already exists")]
    DuplicateTeamName(String),
    /// A player was assigned to a team while already a member of another — a
    /// player belongs to exactly one team.
    #[error("player {0} is already in another team")]
    PlayerAlreadyInATeam(Uuid),
    /// A player was assigned to a team that already has its full roster.
    #[error("that team already has its {size} members")]
    TeamIsFull { size: u32 },
    /// A player was removed from, or reordered within, a team they aren't in.
    #[error("player {player} is not a member of team {team}")]
    NotATeamMember { team: Uuid, player: Uuid },
    /// A board-order reorder didn't name exactly the team's current members.
    #[error("a board order must list exactly the team's current members")]
    InvalidBoardOrder,
    /// Finalization found a player belonging to no team.
    #[error("every player must be in a team before finalizing ({count} are not)")]
    PlayersWithoutTeam { count: usize },
    /// Finalization found a team whose roster isn't the configured size.
    #[error("team {name} has {have} of {need} members")]
    IncompleteTeam {
        name: String,
        have: usize,
        need: u32,
    },
    /// Finalization found a team member with neither a rating nor a referee-set
    /// pairing rating, while MacMahon starting points are in use — so that member
    /// would contribute nothing to the team average the thresholds read.
    #[error(
        "MacMahon starting points need a pairing ELO for every unrated team \
         member ({count} are missing one)"
    )]
    MembersWithoutPairingRating { count: usize },
    /// Fewer than two teams at finalization.
    #[error("need at least 2 teams (have {have})")]
    NotEnoughTeams { have: usize },
    /// A pairing rating was set outside the one configuration it means anything
    /// in: team mode with MacMahon starting points.
    #[error("a pairing ELO is only meaningful in team mode with MacMahon starting points")]
    PairingRatingNotApplicable,
    /// Registration of any kind after finalization, in team mode (see
    /// `docs/archive/team-tournaments.md`: a late individual would be teamless, and a
    /// late team would need teamless players first).
    #[error("a team tournament cannot take late registrations")]
    NoLateRegistrationInTeamMode,
    /// Too few present teams to start a team round.
    #[error("need at least {needed} present teams (have {have})")]
    NotEnoughPresentTeams { needed: usize, have: usize },
    /// A player-level forced pairing or forced bye was submitted for a team
    /// round, where teams are what get paired.
    #[error("a team round is paired by team, so it takes no player-level forced pairing or bye")]
    PlayerLevelDraftInTeamMode,
    /// A manual point adjustment was applied to a player in a team tournament,
    /// where the ranking is by team so a per-player delta moves nothing visible.
    /// Team-level adjustments are the answer, and are still to come.
    #[error("a team tournament ranks by team, so a per-player point adjustment has no effect")]
    PlayerAdjustmentInTeamMode,
    /// A loaded round carries an explanation that names a different round, so the
    /// rationale and the pairings it is shown against do not belong together.
    #[error("round {round} carries the explanation of round {explains}")]
    MisplacedExplanation { round: u32, explains: u32 },
    /// A loaded tournament vouches for the explanations of rounds it does not
    /// have (see [`Tournament::explanations_faithful_through`]).
    #[error(
        "the explanation watermark is {mark}, past the last of the {rounds} \
         rounds this tournament has"
    )]
    WatermarkPastLastRound { mark: u32, rounds: usize },
    /// A justified absence was recorded on a board outside team mode, where it
    /// cannot arise: an absent player is excluded from the pairing before any
    /// board exists, so the only forfeit an individual tournament can produce is
    /// an unjustified no-show.
    #[error("a justified absence on a board only exists in a team tournament")]
    JustifiedAbsenceOutsideTeamMode,
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
            settings: TournamentSettings::default(),
            players: Vec::new(),
            registration_finalized: false,
            draft: None,
            rounds: Vec::new(),
            cup: None,
            teams: Vec::new(),
            explanations_faithful_through: 0,
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
        // Team mode takes no late registration at all: a player registered now
        // would be teamless, breaking the frozen "everyone is in exactly one
        // team" invariant, and a whole new team would need its players registered
        // teamless first. See `docs/archive/team-tournaments.md`.
        if self.registration_finalized && self.settings.team_mode() {
            return Err(TournamentError::NoLateRegistrationInTeamMode);
        }
        let mut player = Player::from_new(new);
        // A player registered after finalization gets the next free number
        // immediately (independent of rating); before finalization, numbers are
        // assigned in bulk by `finalize_registration`.
        if self.registration_finalized {
            player.tournament_id = Some(TournamentId(self.next_tournament_id()));
        }
        self.players.push(player);
        // `push` never reallocates away the just-added last element.
        Ok(self.players.last().expect("just pushed a player"))
    }

    /// The next unused tournament number (max assigned + 1). Numbers are never
    /// reused, so results referencing a number stay unambiguous.
    fn next_tournament_id(&self) -> u32 {
        self.players
            .iter()
            .filter_map(|p| p.tournament_id)
            .max()
            .map_or(0, |t| t.0)
            + 1
    }

    /// Replace the editable fields of an existing player, keeping its id and
    /// position. Used for in-place cell editing.
    ///
    /// Returns [`TournamentError::EmptyPlayerName`] if the new last name is blank
    /// or [`TournamentError::PlayerNotFound`] if no player has that id.
    pub fn edit_player(&mut self, id: Uuid, new: NewPlayer) -> Result<&Player, TournamentError> {
        if new.last_name.trim().is_empty() {
            return Err(TournamentError::EmptyPlayerName);
        }
        let moved = {
            let player = self.player_mut(id)?;
            // Reuse the normalization in `from_new`, but keep the existing id.
            let normalized = Player::from_new(new);
            // Rating, club and nationality are pairing input. Compared rather
            // than assumed changed: the client re-sends the whole player for a
            // one-cell edit, and a warning that fires on a no-op is a warning
            // referees learn to ignore.
            let moved = (&player.rating, &player.nationality, &player.club)
                != (
                    &normalized.rating,
                    &normalized.nationality,
                    &normalized.club,
                );
            player.last_name = normalized.last_name;
            player.first_name = normalized.first_name;
            player.rating = normalized.rating;
            player.grade = normalized.grade;
            player.nationality = normalized.nationality;
            player.club = normalized.club;
            moved
        };
        if moved {
            // A player's pairing data sits under every round's model at once.
            self.explanations_all_stale();
        }
        self.player_mut(id).map(|p| &*p)
    }

    /// Remove the player with the given id.
    ///
    /// Returns [`TournamentError::PlayerNotFound`] if no such player exists,
    /// [`TournamentError::CannotRemoveCupPlayer`] if the player is seeded in the
    /// cup bracket (removing them would corrupt it),
    /// [`TournamentError::CannotRemovePlayedPlayer`] if the player has already been
    /// paired into a game (their results are referenced by every opponent's score
    /// record — mark them absent for future rounds instead), or
    /// [`TournamentError::CannotRemoveTeamPlayer`] in a finalized team
    /// tournament, where the team is the pairing unit and rosters are fixed at
    /// `team_size` — remove the team with [`remove_team`](Self::remove_team).
    pub fn remove_player(&mut self, id: Uuid) -> Result<(), TournamentError> {
        // Individually, that is: `remove_team` takes its members out through the
        // same path below once the tournament is under way, because after
        // finalization there is no unassigned pool to leave them in.
        if self.settings.team_mode() && self.registration_finalized {
            return Err(TournamentError::CannotRemoveTeamPlayer);
        }
        self.remove_player_inner(id)
    }

    /// [`remove_player`](Self::remove_player) without the team-mode guard: the
    /// removal itself, and everything that has to go with it.
    ///
    /// Split out so team removal can reach it. A team that has played is refused
    /// at *its* level, so by the time this runs for a member the same thing has
    /// been established for them — nobody is on a board.
    pub(crate) fn remove_player_inner(&mut self, id: Uuid) -> Result<(), TournamentError> {
        // The player's tournament number, if finalized. A not-yet-numbered player is
        // on no board and in no cup seed (both are keyed by number).
        let tid = self
            .players
            .iter()
            .find(|p| p.id == id)
            .and_then(|p| p.tournament_id);
        if let (Some(t), Some(cup)) = (tid, &self.cup) {
            if cup.seed_order.contains(&t) {
                return Err(TournamentError::CannotRemoveCupPlayer);
            }
        }
        // A started round's boards are what other players' opponent/defeated lists
        // are built from; once a player appears on one, removing them would dangle
        // those references (which the score tables index densely), so forbid it.
        let has_played = tid.is_some_and(|t| {
            self.rounds
                .iter()
                .flat_map(|r| &r.boards)
                .any(|b| b.player1 == t || b.player2 == t)
        });
        if has_played {
            return Err(TournamentError::CannotRemovePlayedPlayer);
        }
        let before = self.players.len();
        self.players.retain(|p| p.id != id);
        if self.players.len() == before {
            return Err(TournamentError::PlayerNotFound(id));
        }
        // A removed player leaves their team too, so no roster can reference a
        // player who is no longer registered. (Only reachable before
        // finalization: after it, team rosters are frozen and every player has
        // been paired.)
        for team in &mut self.teams {
            team.members.retain(|&m| m != id);
        }
        if let Some(t) = tid {
            // Their sit-outs go with them. This is the point of allowing the
            // removal at all: someone who said they would miss round 1 and then
            // never came should leave no trace — no standings row, no column in
            // the american grid, not listed absent in a round they were never
            // really part of.
            //
            // It is also what keeps the score tables sound. Numbers are never
            // reused or reassigned (that would dangle every other player's board
            // references), so removing a player leaves a hole, which
            // `compute_scores` is built to tolerate — but only *below* the
            // highest number in play, since that is what sizes the table. A
            // left-behind sit-out naming the highest number, freed by this very
            // removal, indexes past the end.
            //
            // An *engine* bye is named in that round's frozen explanation, so
            // dropping it leaves the ledger citing a player who is gone. A
            // referee-forced bye and a cup bye are not the engine's choices and
            // appear in no ledger (see `explain_pairing`), so they leave every
            // explanation as faithful as it was.
            let had_engine_bye = self
                .rounds
                .iter()
                .flat_map(|r| &r.sitouts)
                .any(|s| s.player == t && s.kind == SitoutKind::Bye);
            for round in &mut self.rounds {
                round.sitouts.retain(|s| s.player != t);
            }
            // The open draft names them too, and confirming it would pair a
            // number no player answers to.
            if let Some(draft) = &mut self.draft {
                draft.absent.retain(|&a| a != t);
                draft.forced_byes.retain(|&b| b != t);
                draft
                    .forced_boards
                    .retain(|b| b.player1 != t && b.player2 != t);
            }
            if had_engine_bye {
                self.explanations_all_stale();
            }
        }
        Ok(())
    }

    /// Replace the tournament settings (MacMahon groups, degressive schedule, …),
    /// stored in canonical form (see [`TournamentSettings::normalized`]).
    ///
    /// Allowed at any point; the caller (UI) warns when registration is already
    /// finalized, since changing the groups shifts everyone's points and future
    /// pairings.
    pub fn update_settings(
        &mut self,
        settings: TournamentSettings,
    ) -> Result<&TournamentSettings, TournamentError> {
        // Team mode and the features it can't support are rejected as a pair,
        // naming the conflict — neither side is silently switched off. Checked
        // *before* normalization, which would otherwise quietly drop some of them
        // (the estimated-ELO tie-break can't rank in team mode, so `normalized`
        // removes it) and turn a conflict the referee should see into a silent
        // edit of what they asked for.
        if let Some(conflict) = settings.team_mode_conflict() {
            return Err(TournamentError::TeamModeConflict(conflict));
        }
        let settings = settings.normalized();
        // Team mode is structural: whether teams exist at all, and how many
        // players a match takes, shape every roster and every board. Both are
        // fixed at finalization, like the cup bracket.
        if self.registration_finalized
            && (settings.team_mode() != self.settings.team_mode()
                || settings.team_size() != self.settings.team_size())
        {
            return Err(TournamentError::TeamSettingsLocked);
        }
        if let Some(teams) = &settings.teams {
            if !TEAM_SIZES.contains(&teams.size) {
                return Err(TournamentError::InvalidTeamSize { size: teams.size });
            }
        }
        // Once the tournament has started, a settings change is the last gate
        // before the new config takes effect (there is no re-finalization), so the
        // ELO scale anchor is validated here too. Before finalization the field may
        // still be incomplete, so that case is left to
        // [`finalize_registration_with`], which validates against the final field.
        if self.registration_finalized {
            Self::validate_elo_scale_anchor(&settings, &self.players)?;
        }
        // A pairing ELO only means anything under MacMahon starting points; if
        // this update turns those off, drop the values rather than leaving them
        // stored where nothing reads them and no UI can reach them.
        if !(settings.team_mode() && settings.macmahon_in_use()) {
            for p in &mut self.players {
                p.pairing_rating = None;
            }
        }
        // Prune any player memberships in categories this update deleted, so a
        // stale id can never linger (and later collide with a re-created one).
        let valid_categories = settings.category_ids();
        for p in &mut self.players {
            p.categories.retain(|c| valid_categories.contains(c));
        }
        // The settings sit under every round's pairing model. Coarse — a change
        // to the tournament's city moves the watermark too — but compared rather
        // than assumed, since the client PUTs the whole settings object for any
        // one field.
        if settings != self.settings {
            self.explanations_all_stale();
        }
        self.settings = settings;
        Ok(&self.settings)
    }

    /// If the ELO estimate would be live under `settings`, require the field to
    /// anchor its scale — otherwise the estimate (and anything it drives: pairing,
    /// estimate-based MacMahon) has no absolute reference. See
    /// [`crate::elo::has_scale_anchor`].
    fn validate_elo_scale_anchor(
        settings: &TournamentSettings,
        players: &[Player],
    ) -> Result<(), TournamentError> {
        if settings.elo_estimate_live() && !crate::elo::has_scale_anchor(players, settings) {
            return Err(TournamentError::EloEstimateUnanchored);
        }
        Ok(())
    }

    /// The ranked **team** standings, or `None` outside team mode — the team
    /// twin of [`Self::standings`], with the same safety floor (nothing to rank
    /// before finalization assigns the numbers everything is keyed by).
    pub fn team_standings(&self) -> Option<Vec<TeamStanding>> {
        if !self.settings.team_mode() {
            return None;
        }
        if !self.registration_finalized {
            return Some(Vec::new());
        }
        Some(compute_team_standings(
            &self.teams,
            &self.players,
            &self.settings,
            &self.rounds,
        ))
    }

    /// The ranked standings (points and tie-breaks) from the completed rounds.
    ///
    /// This is the canonical ordering — used by the Results tab and by the
    /// American grid — so scoring lives in one place rather than being
    /// re-derived by each client.
    pub fn standings(&self) -> Vec<Standing> {
        // Scoring keys players by their tournament number, which is only assigned
        // at finalization, so there is nothing safe (or meaningful) to compute
        // before then — a client showing standings during registration gets an
        // empty list. (The standings *tab* proper is hidden until a round is
        // played; this is just the safety floor.)
        if !self.registration_finalized {
            return Vec::new();
        }
        compute_standings(&self.players, &self.settings, &self.rounds)
    }

    /// Finalize registration with no cup (the common path). See
    /// [`finalize_registration_with`](Self::finalize_registration_with).
    pub fn finalize_registration(&mut self) -> Result<(), TournamentError> {
        self.finalize_registration_with(None)
    }

    /// Finalize registration, optionally seeding the direct-elimination cup.
    ///
    /// Assigns tournament numbers (highest ELO first, unrated last, ties by
    /// registration order) and gates round creation behind this explicit step.
    /// When the cup is enabled in the settings, `cup_size` must be a supported
    /// bracket size (8/16/32/64); the top eligible players (by tournament number)
    /// are frozen into the cup in seed order. How many that takes depends on the
    /// configured [`CupFormat`](crate::CupFormat): `cup_size` for a direct
    /// bracket, half as many again for the qualifier format, whose qualification
    /// round feeds half the bracket (see [`cup_field_size`]). The cup is validated *before* any
    /// mutation, so a rejected cup leaves registration open for the referee to fix.
    pub fn finalize_registration_with(
        &mut self,
        cup_size: Option<u32>,
    ) -> Result<(), TournamentError> {
        if self.registration_finalized {
            return Err(TournamentError::RegistrationAlreadyFinalized);
        }

        // Pre-validate (before any mutation) that an enabled ELO estimate has a
        // scale anchor in the final field, so we don't finalize into an estimate
        // whose absolute scale is undefined.
        Self::validate_elo_scale_anchor(&self.settings, &self.players)?;

        // Pre-validate the cup so a bad request doesn't half-finalize.
        let cup_shape = if self.settings.cup_enabled {
            let size = cup_size.ok_or(TournamentError::CupSizeRequired)?;
            if !CUP_SIZES.contains(&size) {
                return Err(TournamentError::InvalidCupSize { size });
            }
            // The qualifier format takes half as many players again as the
            // bracket holds, so the eligibility floor is the *field* size.
            let format = self.settings.cup_format;
            let needed = cup_field_size(size, format);
            let eligible = self.players.iter().filter(|p| p.eligible).count();
            if eligible < needed as usize {
                return Err(TournamentError::NotEnoughEligiblePlayers {
                    needed,
                    have: eligible,
                });
            }
            Some((size, format, needed))
        } else {
            None
        };

        // In team mode: validate the rosters and number the teams. Placed after
        // every other pre-validation so nothing can fail once it has started
        // assigning numbers (team mode rejects the cup, so `cup_shape` is `None`
        // here anyway — but the ordering is what keeps that a fact rather than a
        // coincidence).
        if self.settings.team_mode() {
            self.finalize_teams()?;
        }

        // Assign tournament numbers 1..N in the sorted-table order: highest ELO
        // first, unrated players last, ties broken by registration order.
        let mut order: Vec<usize> = (0..self.players.len()).collect();
        order.sort_by(|&a, &b| {
            let (ra, rb) = (self.players[a].rating, self.players[b].rating);
            let by_rating = match (ra, rb) {
                (Some(x), Some(y)) => y.cmp(&x),   // descending
                (Some(_), None) => Ordering::Less, // rated before unrated
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            by_rating.then(a.cmp(&b)) // stable: registration order breaks ties
        });
        for (rank, &idx) in order.iter().enumerate() {
            self.players[idx].tournament_id = Some(TournamentId(rank as u32 + 1));
        }

        // Seed the cup from the top eligible players by tournament number.
        if let Some((size, format, field)) = cup_shape {
            let mut eligible: Vec<&Player> = self.players.iter().filter(|p| p.eligible).collect();
            eligible.sort_by_key(|p| p.tournament_id.unwrap_or(TournamentId(u32::MAX)));
            let seed_order = eligible
                .into_iter()
                .take(field as usize)
                .map(|p| p.tournament_id.expect("finalized players have a number"))
                .collect();
            self.cup = Some(Cup {
                size,
                format,
                seed_order,
            });
        }

        self.registration_finalized = true;
        Ok(())
    }

    /// Set whether a player is eligible for the cup. Only meaningful before
    /// finalization (the bracket is frozen then); allowed at any time so the
    /// column stays editable.
    pub fn set_player_eligible(
        &mut self,
        id: Uuid,
        eligible: bool,
    ) -> Result<&Player, TournamentError> {
        let player = self.player_mut(id)?;
        player.eligible = eligible;
        Ok(player)
    }

    /// Add (`member`) or remove a player's membership in a referee-defined
    /// category. The category must currently exist in the settings, else
    /// [`TournamentError::CategoryNotFound`]. Allowed at any time — categories are
    /// descriptive, so (unlike cup eligibility) they never freeze at
    /// finalization. The stored list is kept sorted and de-duplicated.
    pub fn set_player_category(
        &mut self,
        id: Uuid,
        category_id: Uuid,
        member: bool,
    ) -> Result<&Player, TournamentError> {
        if !self.settings.categories.iter().any(|c| c.id == category_id) {
            return Err(TournamentError::CategoryNotFound(category_id));
        }
        let player = self.player_mut(id)?;
        if member {
            if !player.categories.contains(&category_id) {
                player.categories.push(category_id);
                player.categories.sort();
            }
        } else {
            player.categories.retain(|&c| c != category_id);
        }
        Ok(player)
    }

    /// Apply a manual point bonus (positive `delta`) or malus (negative `delta`)
    /// to a player, with a mandatory `reason` shown to referees.
    ///
    /// Returns [`TournamentError::PlayerNotFound`] if no such player exists,
    /// [`TournamentError::EmptyAdjustmentReason`] if `reason` is blank, or
    /// [`TournamentError::ZeroPointAdjustment`] if `delta` is zero.
    pub fn add_point_adjustment(
        &mut self,
        player_id: Uuid,
        delta: i32,
        reason: String,
    ) -> Result<&Player, TournamentError> {
        // In a team tournament the ranking is by team, so a delta on one player
        // would move nothing a referee can see. Adjustments belong to teams there
        // — not implemented yet, so this refuses rather than storing a bonus that
        // silently does nothing.
        if self.settings.team_mode() {
            return Err(TournamentError::PlayerAdjustmentInTeamMode);
        }
        let reason = reason.trim().to_string();
        if reason.is_empty() {
            return Err(TournamentError::EmptyAdjustmentReason);
        }
        if delta == 0 {
            return Err(TournamentError::ZeroPointAdjustment);
        }
        self.player_mut(player_id)?
            .adjustments
            .push(PointAdjustment {
                id: Uuid::new_v4(),
                delta,
                reason,
            });
        // An adjustment moves points, and points are what every round was paired
        // on — it carries no round of its own, so it lands under all of them.
        self.explanations_all_stale();
        self.player_mut(player_id).map(|p| &*p)
    }

    /// Remove a previously applied point adjustment.
    ///
    /// Returns [`TournamentError::PlayerNotFound`] if no such player exists, or
    /// [`TournamentError::AdjustmentNotFound`] if the player has no adjustment
    /// with that id.
    pub fn remove_point_adjustment(
        &mut self,
        player_id: Uuid,
        adjustment_id: Uuid,
    ) -> Result<&Player, TournamentError> {
        let player = self.player_mut(player_id)?;
        let before = player.adjustments.len();
        player.adjustments.retain(|a| a.id != adjustment_id);
        if player.adjustments.len() == before {
            return Err(TournamentError::AdjustmentNotFound {
                player: player_id,
                adjustment: adjustment_id,
            });
        }
        self.explanations_all_stale();
        self.player_mut(player_id).map(|p| &*p)
    }

    /// The cup podium (champion, runner-up, third, fourth), once the final round is
    /// decided; `None` when there is no cup or the final isn't finished.
    pub fn cup_podium(&self) -> Option<CupPodium> {
        self.cup.as_ref()?.podium(&self.rounds)
    }

    /// The full cup bracket, derived for the client (structure + results); `None`
    /// when there is no cup. Drawn from the frozen seeding, so it exists as soon
    /// as registration is finalized and fills in as results land.
    pub fn cup_bracket(&self) -> Option<CupBracketView> {
        Some(self.cup.as_ref()?.bracket_view(&self.rounds))
    }

    /// The pre-qualified cup players of round `r` — empty unless `r` is a
    /// qualifier cup's qualification round (see [`Cup::prequalified_for_round`]).
    /// They are in the Swiss pool that round but must not be paired with each
    /// other, which is the pairing engine's business, so every call into it gets
    /// this list.
    fn prequalified_in_round(&self, r: u32) -> &[TournamentId] {
        match &self.cup {
            Some(cup) => cup.prequalified_for_round(&self.rounds, r),
            None => &[],
        }
    }

    /// The pairing engine's input for round `r`, replayed from `completed` — one
    /// [`PairingUnit`] per player, keyed by tournament number.
    ///
    /// The engine knows nothing about [`Player`]: it pairs opaque units, so that
    /// one implementation serves individual and team tournaments alike. This is
    /// the individual side of that seam, and the only place the per-round cup
    /// context reaches it.
    ///
    /// `completed` is passed rather than read off `self` because the explanation
    /// paths replay a *past* round, from the rounds before it.
    fn pairing_units(&self, r: u32, completed: &[Round]) -> TiVec<UnitKey, PairingUnit> {
        player_units(
            &self.players,
            &self.settings,
            completed,
            self.prequalified_in_round(r),
        )
    }

    /// The players the cup bracket will pair in the round currently being drafted
    /// (empty when there is no draft, no cup, or the draft round is past the cup).
    /// Lets clients keep those players out of the Swiss customization UI.
    pub fn draft_cup_players(&self) -> Vec<TournamentId> {
        let (Some(draft), Some(cup)) = (&self.draft, &self.cup) else {
            return Vec::new();
        };
        cup.matches_for_round(&self.rounds, draft.number)
            .map(|p| {
                p.matches
                    .iter()
                    .flat_map(|m| [m.player1, m.player2])
                    .chain(p.byes.iter().map(|&(player, _)| player))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Cancel the most recent round, stepping the tournament back one stage.
    ///
    /// Peels off exactly one step *as the referee took it*: if a round is
    /// currently being drafted, the draft is discarded; otherwise the last round
    /// is removed. Either way, once nothing is left before round 1 this also
    /// undoes `finalize_registration` (tournament numbers and any cup bracket
    /// are cleared) — back to open registration, which is where the click that
    /// prepared round 1 started. Earlier rounds keep their results, so a removed
    /// round N>1 simply lands back on round N-1, which stays complete (its games
    /// are all still recorded) and ready to re-prepare the next round.
    ///
    /// This makes it easy to re-pair and replay a round in simulations, and lets
    /// a referee undo a round in the rare cases that call for it. It is undoable
    /// like any other mutation.
    ///
    /// Returns [`TournamentError::NoRoundToCancel`] when there is neither a draft
    /// nor any round to remove.
    pub fn cancel_last_round(&mut self) -> Result<(), TournamentError> {
        if self.draft.take().is_some() {
            // Discarding the *first* draft reopens registration too. Preparing
            // round 1 is one referee action that takes two steps — it finalizes
            // registration and then opens the draft — so peeling only the draft
            // off would leave the tournament a step ahead of where that click
            // found it, with registration closed and nothing to show for it and
            // no way back but undo. In team mode that state refuses late
            // registration outright, so a referee who discards a draft to add
            // someone cannot.
            //
            // With a round already played, finalization belongs to that round
            // rather than to this draft, and stays.
            if self.rounds.is_empty() {
                self.reopen_registration();
            }
            return Ok(());
        }
        if self.pop_round_uncarrying().is_none() {
            return Err(TournamentError::NoRoundToCancel);
        }
        // The watermark counts rounds, so it must not survive past the end of the
        // list it indexes into.
        self.explanations_stale_after(self.rounds.len() as u32);
        // Removing the very first round reopens registration; later rounds leave
        // the preceding one untouched (and thus still complete).
        if self.rounds.is_empty() {
            self.reopen_registration();
        }
        Ok(())
    }

    /// Undo [`finalize_registration`](Self::finalize_registration): tournament
    /// numbers and any frozen cup bracket are cleared, and the roster is open to
    /// edit again.
    ///
    /// Only meaningful with no rounds left — a played round references players
    /// by the numbers this clears.
    fn reopen_registration(&mut self) {
        debug_assert!(
            self.rounds.is_empty(),
            "reopening registration would strip the numbers the remaining rounds reference",
        );
        self.registration_finalized = false;
        self.cup = None;
        for player in &mut self.players {
            player.tournament_id = None;
        }
    }

    /// Remove the last round, moving any long game it was finishing back onto
    /// the record it came from — which reverts to [`GameRecord::LongStart`].
    ///
    /// The exact inverse of the carry in [`confirm_round_inner`], and the only
    /// supported way to drop a round. Popping without this leaves the previous
    /// round holding an inert `LongCarried` record with nothing to make it live
    /// again: re-confirming finds no `LongStart` to carry, and the game — result
    /// and all — silently disappears.
    ///
    /// [`confirm_round_inner`]: Self::confirm_round_inner
    fn pop_round_uncarrying(&mut self) -> Option<Round> {
        let cancelled = self.rounds.pop()?;
        for board in &cancelled.boards {
            let GameRecord::LongEnd(outcome) = board.record else {
                continue;
            };
            let restored = self
                .rounds
                .last_mut()
                .and_then(|previous| {
                    previous.boards.iter_mut().find(|b| {
                        b.record == GameRecord::LongCarried
                            && b.player1 == board.player1
                            && b.player2 == board.player2
                    })
                })
                .map(|b| {
                    b.record = GameRecord::LongStart(outcome);
                    b.handicap = board.handicap;
                });
            debug_assert!(
                restored.is_some(),
                "a carried long game has its starting record in the previous round"
            );
        }
        Some(cancelled)
    }

    /// Begin preparing the next round (the `RoundDraft` state).
    ///
    /// Requires registration finalized and the previous round (if any)
    /// completed. The draft's absent set defaults to the previous round's
    /// absentees (restricted to players who still exist), so recurring absences
    /// carry over while late joiners are not pre-marked absent.
    /// Why the American Grid export would refuse right now, or `None` if it
    /// would produce a document.
    ///
    /// Same contract as [`next_round_blocker`](Self::next_round_blocker):
    /// `american_grid::to_grid` is defined in terms of this, so the button's
    /// reason and the export's refusal are the same sentence, computed once.
    pub fn grid_export_blocker(&self) -> Option<TournamentError> {
        self.rounds
            .iter()
            .find(|r| r.boards.iter().any(|b| b.long_pending()))
            .map(|round| TournamentError::UnresolvedLongGame {
                round: round.number,
            })
    }

    /// Why [`prepare_round`](Self::prepare_round) would refuse right now, or
    /// `None` if it would go ahead.
    ///
    /// Exists so a client can disable the button *and say why* without
    /// re-deriving the rule: `prepare_round` is defined in terms of this, so the
    /// two cannot answer differently. Every previous attempt to mirror a rule
    /// like this in the frontend has drifted from the original — a predicate
    /// copied into TypeScript is a predicate that will one day disagree.
    pub fn next_round_blocker(&self) -> Option<TournamentError> {
        if !self.registration_finalized {
            return Some(TournamentError::RegistrationNotFinalized);
        }
        if self.draft.is_some() {
            return Some(TournamentError::DraftAlreadyExists);
        }
        if self.rounds.last().is_some_and(|last| !last.completed) {
            return Some(TournamentError::PreviousRoundNotComplete);
        }
        None
    }

    pub fn prepare_round(&mut self) -> Result<&RoundDraft, TournamentError> {
        if let Some(blocked) = self.next_round_blocker() {
            return Err(blocked);
        }

        let number = self.rounds.len() as u32 + 1;
        // No guard for an overrunning long game is needed here any more. A game
        // carried into round R+1 has its live `LongEnd` record *in* R+1, where it
        // is an ordinary board, so R+1 is not complete until the result is in —
        // and the `PreviousRoundNotComplete` check above already refuses R+2 on
        // exactly that basis. The old `UnresolvedLongGame` guard existed only
        // because the game was invisible to R+1's completion; it would now be
        // unreachable. (The error variant survives: the American Grid export
        // still refuses while any long game is unresolved.)
        let existing: HashSet<TournamentId> = self
            .players
            .iter()
            .filter_map(|p| p.tournament_id)
            .collect();
        // Default the new draft's absentees to the previous round's absentees,
        // plus anyone who was a no-show there — a no-show reads as "didn't turn
        // up", so it should carry over the same way an explicit absence does (the
        // referee can still uncheck it).
        let default_absent: Vec<TournamentId> = self
            .rounds
            .last()
            .map(|r| {
                let mut absent: Vec<TournamentId> = r.absentees().collect();
                for board in &r.boards {
                    for (side, id) in [
                        (Winner::Player1, board.player1),
                        (Winner::Player2, board.player2),
                    ] {
                        if board.no_show_absent(side) && !absent.contains(&id) {
                            absent.push(id);
                        }
                    }
                }
                absent.retain(|id| existing.contains(id));
                // Never propose a player who is finishing a long game. Their game
                // is about to become a board of this round, so they are playing,
                // not absent — and `confirm_round` refuses the combination. A
                // no-show *on* the long board is the case that reaches here: it
                // resolves the game, but the carry still happens (a long game
                // takes both its rounds however it ends), so they are still busy.
                absent.retain(|id| {
                    !r.boards
                        .iter()
                        .any(|b| b.is_long() && (b.player1 == *id || b.player2 == *id))
                });
                absent
            })
            .unwrap_or_default();

        self.draft = Some(RoundDraft {
            number,
            absent: default_absent,
            forced_boards: Vec::new(),
            forced_byes: Vec::new(),
            forced_matches: Vec::new(),
            forced_team_byes: Vec::new(),
        });
        Ok(self.draft.as_ref().expect("just set the draft"))
    }

    /// Replace the current draft's customization (absent set, forced pairings,
    /// forced byes). Structural consistency is validated when the round is
    /// confirmed; here we only check that every referenced player exists.
    pub fn update_draft(
        &mut self,
        absent: Vec<TournamentId>,
        forced_boards: Vec<Board>,
        forced_byes: Vec<TournamentId>,
    ) -> Result<&RoundDraft, TournamentError> {
        if self.draft.is_none() {
            return Err(TournamentError::NoDraft);
        }
        let known: HashSet<TournamentId> = self
            .players
            .iter()
            .filter_map(|p| p.tournament_id)
            .collect();
        let referenced = absent
            .iter()
            .chain(forced_boards.iter().flat_map(|b| [&b.player1, &b.player2]))
            .chain(forced_byes.iter());
        for id in referenced {
            if !known.contains(id) {
                return Err(TournamentError::InvalidDraft(format!(
                    "references unknown player {id}"
                )));
            }
        }

        // Player-level forcing has no meaning when teams are what get paired.
        // The absent set is still per player there — a member can be absent
        // without their team being — so only the forced halves are rejected.
        if self.settings.team_mode() && !(forced_boards.is_empty() && forced_byes.is_empty()) {
            return Err(TournamentError::PlayerLevelDraftInTeamMode);
        }

        let draft = self.draft.as_mut().expect("draft present");
        draft.absent = absent;
        draft.forced_boards = forced_boards
            .into_iter()
            .map(|b| Board::pending(b.player1, b.player2, 0, PairingSource::Forced))
            .collect();
        draft.forced_byes = forced_byes;
        draft.forced_matches = Vec::new();
        draft.forced_team_byes = Vec::new();
        Ok(self.draft.as_ref().expect("draft present"))
    }

    /// Replace the current draft's customization in **team mode**: the absent
    /// players (still per player — a member can be absent without their team
    /// being), plus the matches and byes the referee has fixed by hand.
    ///
    /// The team twin of [`Self::update_draft`], because a forced pairing names
    /// two *teams* here. Structural consistency is validated when the round is
    /// confirmed; this only checks that every referenced team and player exists.
    pub fn update_team_draft(
        &mut self,
        absent: Vec<TournamentId>,
        forced_matches: Vec<ForcedMatch>,
        forced_team_byes: Vec<TeamId>,
    ) -> Result<&RoundDraft, TournamentError> {
        if !self.settings.team_mode() {
            return Err(TournamentError::NotATeamTournament);
        }
        if self.draft.is_none() {
            return Err(TournamentError::NoDraft);
        }
        let known_players: HashSet<TournamentId> = self
            .players
            .iter()
            .filter_map(|p| p.tournament_id)
            .collect();
        for id in &absent {
            if !known_players.contains(id) {
                return Err(TournamentError::InvalidDraft(format!(
                    "references unknown player {id}"
                )));
            }
        }
        let known_teams: HashSet<TeamId> =
            self.teams.iter().filter_map(|t| t.tournament_id).collect();
        let referenced = forced_matches
            .iter()
            .flat_map(|m| [m.team1, m.team2])
            .chain(forced_team_byes.iter().copied());
        for id in referenced {
            if !known_teams.contains(&id) {
                return Err(TournamentError::InvalidDraft(format!(
                    "references unknown team {id}"
                )));
            }
        }

        let draft = self.draft.as_mut().expect("draft present");
        draft.absent = absent;
        draft.forced_boards = Vec::new();
        draft.forced_byes = Vec::new();
        draft.forced_matches = forced_matches;
        draft.forced_team_byes = forced_team_byes;
        Ok(self.draft.as_ref().expect("draft present"))
    }

    /// Confirm the draft: generate the pairings for the present players
    /// (honoring the forced pairings/bye) and append the resulting [`Round`].
    ///
    /// Validates that there are enough present players and that the forced
    /// constraints are consistent.
    pub fn confirm_round(&mut self) -> Result<&Round, TournamentError> {
        self.confirm_round_inner(true)
    }

    /// Like [`Self::confirm_round`], but skips ordering the round's boards by
    /// standings rank — a display-only nicety (the "Board N" numbering) that costs
    /// a full standings computation, over again, every round. The simulator
    /// auto-fills every board by index and never shows them, so it uses this to
    /// avoid recomputing the tie-breaks mid-tournament; the paired boards, results,
    /// and final standings are identical either way.
    pub fn confirm_round_unordered(&mut self) -> Result<&Round, TournamentError> {
        self.confirm_round_inner(false)
    }

    fn confirm_round_inner(
        &mut self,
        order_boards_for_display: bool,
    ) -> Result<&Round, TournamentError> {
        if self.settings.team_mode() {
            return self.confirm_team_round();
        }
        let draft = self.draft.clone().ok_or(TournamentError::NoDraft)?;

        let absent: HashSet<TournamentId> = draft.absent.iter().copied().collect();
        // Present players, by tournament number (rounds are tid-native). Every
        // player has a number here — a round is only confirmed post-finalization.
        let present: Vec<TournamentId> = self
            .players
            .iter()
            .filter_map(|p| p.tournament_id)
            .filter(|t| !absent.contains(t))
            .collect();
        if present.len() < MIN_PLAYERS_PER_ROUND {
            return Err(TournamentError::NotEnoughPresentPlayers {
                needed: MIN_PLAYERS_PER_ROUND,
                have: present.len(),
            });
        }

        // Cup boards for this round (if it is a cup round). They bypass the engine
        // and are generated from the bracket regardless of the absent set — an
        // absent cup player still gets a board and the referee records the forfeit.
        // A cup *bye* arises only when a feeding match was a double no-show, so the
        // player who would have faced its (non-existent) winner advances unopposed.
        let cup_pairings = match &self.cup {
            Some(cup) => cup
                .matches_for_round(&self.rounds, draft.number)
                .ok_or(TournamentError::CupBracketInconsistent)?,
            None => CupPairings::default(),
        };
        // Freeze each cup board's float — points going into this round, exactly
        // like the Swiss boards get from the engine — so a bracket pairing across
        // score groups counts toward float history and curbs re-floating those
        // players in later rounds. Only pay for the score replay on an actual cup
        // round; a plain Swiss round has no cup matches.
        let cup_boards: Vec<Board> = if cup_pairings.matches.is_empty() {
            Vec::new()
        } else {
            let cup_scores = compute_scores(&self.players, &self.settings, &self.rounds);
            let cup_diff = |p1: TournamentId, p2: TournamentId| {
                cup_scores.get_tid(p1).points().halves() as i32
                    - cup_scores.get_tid(p2).points().halves() as i32
            };
            cup_pairings
                .matches
                .iter()
                .map(|m| {
                    Board::pending(
                        m.player1,
                        m.player2,
                        cup_diff(m.player1, m.player2),
                        PairingSource::Cup { stage: m.stage },
                    )
                })
                .collect()
        };
        let cup_byes = cup_pairings.byes;
        let cup_players: HashSet<TournamentId> = cup_boards
            .iter()
            .flat_map(|b| [b.player1, b.player2])
            .chain(cup_byes.iter().map(|&(p, _)| p))
            .collect();

        // Players on a long game started in the previous round. They sit out this
        // round's pairing entirely — like cup players — because a long game *is*
        // one game played across two rounds, which is exactly why its winner
        // scores two points.
        //
        // Deliberately keyed on `long`, not `long_pending`: a long game that
        // finished early (or resolved by a no-show) still took both rounds. It
        // used to free its players the moment it was decided, which let them take
        // three wins out of two rounds — two from the long board, one from the
        // round they should not have been in. The referee who wants them paired
        // here unticks the box before the round advances, demoting the board to an
        // ordinary one-point game; that is the only escape hatch.
        //
        // Only the immediately previous round is scanned, since that is the whole
        // reach of a long game: `prepare_round` refuses to advance past R+1 while
        // one is still pending, so a long board can never be two rounds behind and
        // unresolved. Scanning every round (as this once did) would be wrong now
        // that the predicate no longer clears itself when the game is decided —
        // the players would be excluded from every subsequent round forever.
        let busy_long: HashSet<TournamentId> = self
            .rounds
            .iter()
            .filter(|r| r.number + 1 == draft.number)
            .flat_map(|r| &r.boards)
            .filter(|b| b.is_long())
            .flat_map(|b| [b.player1, b.player2])
            .collect();

        // The Swiss pool: present players not taken by the cup and not mid-long-game.
        let swiss_present: Vec<TournamentId> = present
            .iter()
            .copied()
            .filter(|id| !cup_players.contains(id) && !busy_long.contains(id))
            .collect();
        let swiss_set: HashSet<TournamentId> = swiss_present.iter().copied().collect();

        // A player is absent at most once. The forced halves get their own
        // "once each" checks below (via `placed`), and being absent already
        // excludes a player from those, so this is the one way a player could
        // still reach the round twice — as two sit-out rows, each scoring
        // separately.
        if let Some(player) = first_repeat(draft.absent.iter().copied()) {
            return Err(TournamentError::PlayerTwiceInRound {
                round: draft.number,
                player,
            });
        }

        // A player finishing a long game is playing, not sitting out, and the
        // round is about to receive their board. Marking them absent gave them a
        // sit-out *and* that board: the sit-out then took precedence in the
        // cross-table (hiding the game from the export) and added its own value
        // on top of the game's score. They should not be offered in the draft at
        // all — this is the backstop for a client that offers them anyway.
        if draft.absent.iter().any(|id| busy_long.contains(id)) {
            return Err(TournamentError::InvalidDraft(
                "an absent player is still in a long game".into(),
            ));
        }

        // Validate the referee's forced boards/bye against the Swiss pool.
        let mut placed: HashSet<TournamentId> = HashSet::new();
        for board in &draft.forced_boards {
            if board.player1 == board.player2 {
                return Err(TournamentError::InvalidDraft(
                    "a forced pairing has the same player twice".into(),
                ));
            }
            for player in [board.player1, board.player2] {
                if cup_players.contains(&player) {
                    return Err(TournamentError::InvalidDraft(
                        "a forced pairing includes a cup player".into(),
                    ));
                }
                if busy_long.contains(&player) {
                    return Err(TournamentError::InvalidDraft(
                        "a forced pairing includes a player still in a long game".into(),
                    ));
                }
                if !swiss_set.contains(&player) {
                    return Err(TournamentError::InvalidDraft(
                        "a forced pairing includes an absent player".into(),
                    ));
                }
                if !placed.insert(player) {
                    return Err(TournamentError::InvalidDraft(
                        "a player is in more than one forced pairing".into(),
                    ));
                }
            }
        }
        for &bye in &draft.forced_byes {
            if cup_players.contains(&bye) {
                return Err(TournamentError::InvalidDraft(
                    "a forced bye is a cup player".into(),
                ));
            }
            if busy_long.contains(&bye) {
                return Err(TournamentError::InvalidDraft(
                    "a forced bye is a player still in a long game".into(),
                ));
            }
            if !swiss_set.contains(&bye) {
                return Err(TournamentError::InvalidDraft(
                    "a forced bye is an absent player".into(),
                ));
            }
            if !placed.insert(bye) {
                return Err(TournamentError::InvalidDraft(
                    "a forced bye is also in a forced pairing, or forced twice".into(),
                ));
            }
        }
        // No parity check on the forced byes: whatever they leave over, the
        // engine byes one more player if the count is odd.

        // Pair the Swiss pool with the engine, then prepend the cup boards. The
        // engine speaks in units; in individual mode a unit is a player, so each
        // matched pair is exactly one board and the bye exactly one sit-out.
        let units = self.pairing_units(draft.number, &self.rounds);
        let forced_pairs: Vec<(UnitKey, UnitKey)> = draft
            .forced_boards
            .iter()
            .map(|b| (UnitKey::from(b.player1), UnitKey::from(b.player2)))
            .collect();
        let forced_bye_keys: Vec<UnitKey> = draft
            .forced_byes
            .iter()
            .copied()
            .map(UnitKey::from)
            .collect();
        let swiss = pair_round_weighted(
            draft.number,
            &self.settings,
            &units,
            &swiss_present
                .iter()
                .copied()
                .map(UnitKey::from)
                .collect::<Vec<_>>(),
            &forced_pairs,
            &forced_bye_keys,
        );
        // Freeze the explanation here, against the very `units` the engine was
        // just handed — this is the one moment the model is provably the one that
        // paired the round. See [`Round::explanation`].
        let explanation = explain_pairing(
            draft.number,
            &self.settings,
            &units,
            &swiss
                .pairs
                .iter()
                .filter(|p| matches!(p.source, PairingSource::Swiss))
                .map(|p| (p.a, p.b))
                .collect::<Vec<_>>(),
            swiss.swiss_bye,
        );

        let mut boards = cup_boards;
        boards.extend(swiss.pairs.iter().map(|p| {
            Board::pending(
                TournamentId::from(p.a),
                TournamentId::from(p.b),
                p.points_diff,
                p.source,
            )
        }));

        // Display order: cup boards first, then by the rank (from the standings
        // entering this round) of the board's best-placed player — the same
        // criteria the pairings view sorts by, so the "Board" numbers it shows
        // match this stored order instead of drifting from a client-only sort.
        // Skipped by the simulator (see `confirm_round_unordered`): it never shows
        // the boards, and a full standings computation per round is the cost.
        if order_boards_for_display {
            // Standings face callers by id; boards store numbers. Map through each
            // player's number (this path is display-only, off the hot sim loop).
            let tid_of: HashMap<Uuid, TournamentId> = self
                .players
                .iter()
                .filter_map(|p| p.tournament_id.map(|t| (p.id, t)))
                .collect();
            let rank_of: HashMap<TournamentId, usize> = self
                .standings()
                .into_iter()
                .enumerate()
                .filter_map(|(rank, s)| tid_of.get(&s.player_id).map(|&t| (t, rank)))
                .collect();
            boards.sort_by_key(|b| {
                let is_cup = matches!(b.source, PairingSource::Cup { .. });
                let best_rank = rank_of
                    .get(&b.player1)
                    .copied()
                    .unwrap_or(usize::MAX)
                    .min(rank_of.get(&b.player2).copied().unwrap_or(usize::MAX));
                (if is_cup { 0 } else { 1 }, best_rank)
            });
        }

        // Everyone with no board: the byes the engine picked or the referee forced
        // (both already built by the engine), the cup byes, and the absentees. What
        // each scores is frozen here — a bye is a full point, an absence follows the
        // tournament default — and the referee can re-value any of them afterwards
        // from the standings.
        // The forced byes score a full point, like any bye; so does the engine's
        // own, when the leftover field was odd.
        let mut sitouts: Vec<Sitout> = draft
            .forced_byes
            .iter()
            .map(|&player| Sitout {
                player,
                kind: SitoutKind::ForcedBye,
                value: SitoutValue::Full,
            })
            .collect();
        sitouts.extend(swiss.swiss_bye.map(|key| Sitout {
            player: TournamentId::from(key),
            kind: SitoutKind::Bye,
            value: SitoutValue::Full,
        }));
        sitouts.extend(cup_byes.into_iter().map(|(player, stage)| Sitout {
            player,
            kind: SitoutKind::CupBye { stage },
            value: SitoutValue::Full,
        }));
        let absent_value = if self.settings.half_point_absences {
            SitoutValue::Half
        } else {
            SitoutValue::Zero
        };
        // An absent player the cup paired anyway (the bracket ignores the absent
        // set, so the referee can record the forfeit) has a board: it is the
        // board that scores them, so they get no sit-out.
        let has_board: HashSet<TournamentId> =
            boards.iter().flat_map(|b| [b.player1, b.player2]).collect();
        // A player already given a sit-out this round (a cup bye, or a bye/absence
        // the engine placed) must not also get an Absent entry: scoring sums every
        // sit-out a player has, so a second one would double-count them. This can
        // happen when a current cup-bye player is also in the referee-set absent
        // list.
        let already_sitting: HashSet<TournamentId> = sitouts.iter().map(|s| s.player).collect();
        sitouts.extend(
            draft
                .absent
                .iter()
                .filter(|id| !has_board.contains(id) && !already_sitting.contains(id))
                .map(|&player| Sitout {
                    player,
                    kind: SitoutKind::Absent,
                    value: absent_value,
                }),
        );

        // Carry forward every long game the previous round started: its outcome
        // *moves* onto a `LongEnd` board here, and the record it came from
        // becomes inert. Derived from the previous round rather than from the
        // draft, so any re-confirmation of this round (`force_pairing` pops it
        // and rebuilds) reproduces the same boards instead of dropping them.
        //
        // The two players were kept out of this round's pairing by `busy_long`
        // above, so these boards cannot collide with one they were paired into.
        if let Some(previous) = self
            .rounds
            .iter_mut()
            .find(|r| r.number + 1 == draft.number)
        {
            for board in previous.boards.iter_mut() {
                if !matches!(board.record, GameRecord::LongStart(_)) {
                    continue;
                }
                // Everything about the *game* moves onto the live record: the
                // outcome, and the handicap conceded in it. What stays behind is
                // only what the round decided — who was paired, and their float.
                // Leaving the handicap here would duplicate it, and a duplicate
                // is a thing that can disagree.
                let carried = Board {
                    record: GameRecord::LongEnd(board.outcome()),
                    source: PairingSource::Carried,
                    ..*board
                };
                board.record = GameRecord::LongCarried;
                board.handicap = None;
                boards.push(carried);
            }
        }

        let mut round = Round {
            number: draft.number,
            boards,
            sitouts,
            completed: false,
            explanation,
        };
        // Almost always `false` — a fresh round has results to record. But a
        // round can be born complete: with every player byed, absent, or already
        // playing a long board carried from the previous round, it has no board
        // at all, and `all()` over nothing is true. The flag is otherwise only
        // recomputed when a board changes, so leaving it `false` here left such a
        // round permanently unfinishable and every later round unpreparable.
        round.completed = round.is_complete();
        self.rounds.push(round);
        self.draft = None;
        self.explanations_extend_through(draft.number);
        Ok(self.rounds.last().expect("just pushed a round"))
    }

    /// Record that round `number` has just been confirmed with a freshly frozen
    /// explanation, advancing [`explanations_faithful_through`] over it — but
    /// only when the prefix below it is intact. A confirmation never *rescues* an
    /// earlier round whose data has since moved; it only extends an unbroken run.
    ///
    /// `>=` rather than `==` because re-pairing (`force_pairing`) pops the round
    /// and re-confirms it, so the mark can already be at `number`.
    ///
    /// [`explanations_faithful_through`]: Self::explanations_faithful_through
    pub(crate) fn explanations_extend_through(&mut self, number: u32) {
        if self.explanations_faithful_through + 1 >= number {
            self.explanations_faithful_through = number;
        }
    }

    /// An edit *inside* round `k` (a result, a sit-out value, a long-board flag):
    /// every later round was paired from a model that read it, so their frozen
    /// explanations may now cite data that has moved. Round `k`'s own explanation
    /// is untouched — it was paired before this round was played.
    pub(crate) fn explanations_stale_after(&mut self, k: u32) {
        self.explanations_faithful_through = self.explanations_faithful_through.min(k);
    }

    /// A global edit — a player's rating, club or nationality, a point
    /// adjustment, a pairing ELO, or the settings. These sit under *every*
    /// round's model, so no round's explanation is known to still match the
    /// present.
    ///
    /// Deliberately coarse: changing one player's club drops the mark even when
    /// no board's ledger would move. The exact version (recompute each round and
    /// compare) is noted as a follow-on in `docs/archive/public-access.md`; a warning
    /// that over-fires is recoverable, one that under-fires is not.
    ///
    /// What is *not* here is as deliberate: a category membership and a cup
    /// eligibility flag are read by neither `player_units` nor `compute_scores`,
    /// so no ledger can depend on them and warning would be pure noise.
    pub(crate) fn explanations_all_stale(&mut self) {
        self.explanations_faithful_through = 0;
    }

    /// Explain the Swiss pairings of the round numbered `round_number`: for each
    /// engine-paired board (and the bye), which rules were relaxed and by how
    /// much, plus a per-rule round report. Forced and cup boards are omitted —
    /// they were not chosen by the engine.
    ///
    /// A lookup, not a computation: the ledger was scored against the model that
    /// actually paired the round and frozen onto it at confirmation (see
    /// [`Round::explanation`]). Whether it still matches the tournament as it
    /// stands now is a separate question, answered by
    /// [`explanations_faithful_through`](Self::explanations_faithful_through).
    pub fn explain_round(&self, round_number: u32) -> Result<RoundExplanation, TournamentError> {
        self.rounds
            .iter()
            .find(|r| r.number == round_number)
            .map(|r| r.explanation.clone())
            .ok_or(TournamentError::RoundNotFound(round_number))
    }

    /// Explain a counterfactual pairing in round `round_number`, relative to the
    /// round's confirmed Swiss pairing: which boards would change, the rings of
    /// affected players, and the net per-rule cost. [`CounterfactualMode::Force`]
    /// asks "why aren't A and B paired?"; [`CounterfactualMode::Forbid`] asks
    /// "why did you pair A and B?".
    ///
    /// If either unit isn't an engine-paired one for that round (it was forced,
    /// is a cup player, or sat out), the result is `scoped_out` with the reason
    /// and no diff — the engine didn't choose its board.
    ///
    /// `a` and `b` are [`UnitKey`]s: player numbers in an individual tournament,
    /// team numbers in a team one, because the engine paired whichever of those
    /// the tournament is run on. [`UnitKey::PHANTOM`] means the bye.
    pub fn explain_counterfactual(
        &self,
        round_number: u32,
        a: UnitKey,
        b: UnitKey,
        mode: CounterfactualMode,
    ) -> Result<Counterfactual, TournamentError> {
        if self.settings.team_mode() {
            return self.explain_team_counterfactual(round_number, a, b, mode);
        }
        let (a, b) = (TournamentId::from(a), TournamentId::from(b));
        let idx = self
            .rounds
            .iter()
            .position(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        let round = &self.rounds[idx];
        let completed = &self.rounds[..idx];
        let swiss_boards: Vec<(UnitKey, UnitKey)> = round
            .boards
            .iter()
            .filter(|bd| matches!(bd.source, PairingSource::Swiss))
            .map(|bd| (UnitKey::from(bd.player1), UnitKey::from(bd.player2)))
            .collect();

        // Both probed players must be engine-paired (in a Swiss board or the
        // engine's own bye — a forced or cup bye wasn't the engine's choice, so
        // there is nothing to explain about it). `PHANTOM` stands for the bye
        // itself (the nil UUID, never a real player id) — it's in scope exactly
        // when this round actually has an engine-chosen bye to negotiate.
        let swiss_bye = round.swiss_bye();
        let in_swiss = |id: TournamentId| {
            let key = UnitKey::from(id);
            swiss_bye == Some(id) || swiss_boards.iter().any(|&(x, y)| x == key || y == key)
        };
        for id in [a, b] {
            if id == PHANTOM {
                if swiss_bye.is_none() {
                    return Ok(Counterfactual {
                        scoped_out: Some(ScopeReason::Absent),
                        cost_delta: Vec::new(),
                        cycles: Vec::new(),
                        changed: Vec::new(),
                    });
                }
                continue;
            }
            if !in_swiss(id) {
                return Ok(Counterfactual {
                    scoped_out: Some(self.scope_reason(round, id)?),
                    cost_delta: Vec::new(),
                    cycles: Vec::new(),
                    changed: Vec::new(),
                });
            }
        }

        let solve = match mode {
            CounterfactualMode::Force => counterfactual_force,
            CounterfactualMode::Forbid => counterfactual_forbid,
        };
        let units = self.pairing_units(round.number, completed);
        Ok(solve(
            round.number,
            &self.settings,
            &units,
            &swiss_boards,
            swiss_bye.map(UnitKey::from),
            UnitKey::from(a),
            UnitKey::from(b),
        ))
    }

    /// The team reading of [`Self::explain_counterfactual`]: the probe names two
    /// *teams*, and the diff is over the round's team matches.
    fn explain_team_counterfactual(
        &self,
        round_number: u32,
        a: UnitKey,
        b: UnitKey,
        mode: CounterfactualMode,
    ) -> Result<Counterfactual, TournamentError> {
        let idx = self
            .rounds
            .iter()
            .position(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        let round = &self.rounds[idx];
        let completed = &self.rounds[..idx];
        let slots = self.team_slots();
        let matches = matches_in_round(round, &slots);
        let swiss_boards: Vec<(UnitKey, UnitKey)> = matches
            .iter()
            .filter(|m| {
                m.boards
                    .first()
                    .is_some_and(|&i| matches!(round.boards[i].source, PairingSource::Swiss))
            })
            .map(|m| (UnitKey::from(m.team1), UnitKey::from(m.team2)))
            .collect();
        let swiss_bye = swiss_bye_team(round, &slots).map(UnitKey::from);

        // Both probed teams must be engine-paired: in a Swiss match, or on the
        // engine's own bye. A forced match or bye wasn't the engine's choice, so
        // there is nothing to explain about it.
        let scoped_out = |reason: ScopeReason| {
            Ok(Counterfactual {
                scoped_out: Some(reason),
                cost_delta: Vec::new(),
                cycles: Vec::new(),
                changed: Vec::new(),
            })
        };
        for key in [a, b] {
            if key == UnitKey::PHANTOM {
                if swiss_bye.is_none() {
                    return scoped_out(ScopeReason::Absent);
                }
                continue;
            }
            if swiss_bye != Some(key) && !swiss_boards.iter().any(|&(x, y)| x == key || y == key) {
                return scoped_out(self.team_scope_reason(round, &slots, TeamId::from(key)));
            }
        }

        let solve = match mode {
            CounterfactualMode::Force => counterfactual_force,
            CounterfactualMode::Forbid => counterfactual_forbid,
        };
        let units = self.team_pairing_units(completed, &slots);
        Ok(solve(
            round.number,
            &self.settings,
            &units,
            &swiss_boards,
            swiss_bye,
            a,
            b,
        ))
    }

    /// Why a team is out of the engine's hands for `round`: the referee fixed
    /// its match or its bye, or it sat the round out entirely.
    fn team_scope_reason(&self, round: &Round, slots: &TeamSlots, team: TeamId) -> ScopeReason {
        let members = slots.members_of(team);
        // A whole-team sit-out: a forced bye was the referee's, anything else
        // (an absence, or the engine's own bye, handled by the caller) is not.
        if let Some(sitout) = members.first().and_then(|&m| round.sitout(m)) {
            return match sitout.kind {
                SitoutKind::ForcedBye => ScopeReason::Forced,
                _ => ScopeReason::Absent,
            };
        }
        let forced = members.iter().any(|&m| {
            round
                .boards
                .iter()
                .any(|b| (b.player1 == m || b.player2 == m) && b.source == PairingSource::Forced)
        });
        if forced {
            ScopeReason::Forced
        } else {
            ScopeReason::Absent
        }
    }

    /// Actually force the pairing `a`–`b` onto the current (last, in-progress)
    /// round: re-pair the round with `a`–`b` added as a referee-forced board,
    /// keeping the round's existing forced boards and absentees. Closes the loop
    /// from the counterfactual preview ("why not pair them?") to applying it.
    ///
    /// Either side may instead be [`PHANTOM`] (the bye sentinel), meaning "force
    /// the other player onto the bye" rather than a real pairing — the engine
    /// re-picks everyone else's boards around that fixed bye.
    ///
    /// Refuses if the round is completed or already has recorded results (re-
    /// pairing would discard them). The pair is validated by the re-pairing path
    /// exactly like any referee-forced board/bye (must be a present Swiss
    /// player, neither a cup player nor already forced elsewhere).
    pub fn force_pairing(&mut self, a: UnitKey, b: UnitKey) -> Result<&Round, TournamentError> {
        let round = self.rounds.last().ok_or(TournamentError::NoCurrentRound)?;
        if round.completed {
            return Err(TournamentError::RoundHasResults);
        }
        if round.boards.iter().any(|bd| bd.is_decided()) {
            return Err(TournamentError::RoundHasResults);
        }
        if self.settings.team_mode() {
            return self.force_team_pairing(a, b);
        }
        let (a, b) = (TournamentId::from(a), TournamentId::from(b));

        // Rebuild the draft the round came from: its absentees, its existing
        // forced boards, plus the newly forced pair (or bye). The engine
        // re-picks everything else, including the bye when it isn't fixed here.
        let mut forced_boards: Vec<Board> = round
            .boards
            .iter()
            .filter(|bd| matches!(bd.source, PairingSource::Forced))
            .map(|bd| Board::pending(bd.player1, bd.player2, 0, PairingSource::Forced))
            .collect();
        // The byes the referee had already fixed carry over; the engine's own bye
        // (if this round had one) goes back up for grabs.
        let mut forced_byes: Vec<TournamentId> = round.forced_byes().collect();
        match (a == PHANTOM, b == PHANTOM) {
            // Forcing onto the bye. Already forced there (so the referee is
            // re-asking for what they have) is a no-op, not a double entry.
            (true, false) | (false, true) => {
                let id = if a == PHANTOM { b } else { a };
                if !forced_byes.contains(&id) {
                    forced_byes.push(id);
                }
            }
            _ => forced_boards.push(Board::pending(a, b, 0, PairingSource::Forced)),
        }
        let draft = RoundDraft {
            number: round.number,
            absent: round.absentees().collect(),
            forced_boards,
            forced_byes,
            forced_matches: Vec::new(),
            forced_team_byes: Vec::new(),
        };

        // Drop the round and re-confirm from the reconstructed draft. Earlier
        // rounds stay completed, so the standings entering this round are intact.
        // Un-carrying is what lets the re-confirmation rebuild any long game this
        // round was finishing: it puts the outcome back on a `LongStart` for the
        // carry to pick up again.
        self.pop_round_uncarrying();
        self.draft = Some(draft);
        self.confirm_round()
    }

    /// The team reading of [`Self::force_pairing`]: fix a *match* (or a team's
    /// bye) onto the current round and re-pair everything else around it.
    ///
    /// Rebuilds the draft the round came from — its absentees and the matches
    /// the referee had already fixed — with the new one added, exactly as the
    /// individual path does. The round is validated by the re-pairing itself.
    fn force_team_pairing(&mut self, a: UnitKey, b: UnitKey) -> Result<&Round, TournamentError> {
        let round = self.rounds.last().expect("checked by the caller");
        let slots = self.team_slots();

        // The matches the referee had already fixed carry over; the engine's own
        // choices go back up for grabs.
        let mut forced_matches: Vec<ForcedMatch> = matches_in_round(round, &slots)
            .iter()
            .filter(|m| {
                m.boards
                    .first()
                    .is_some_and(|&i| round.boards[i].source == PairingSource::Forced)
            })
            .map(|m| ForcedMatch {
                team1: m.team1,
                team2: m.team2,
            })
            .collect();
        // ...and so do the byes the referee fixed; the engine's own does not.
        let mut forced_team_byes: Vec<TeamId> = round
            .forced_byes()
            .filter_map(|p| slots.team_of(p))
            .collect();
        forced_team_byes.sort_unstable();
        forced_team_byes.dedup();

        match (a == UnitKey::PHANTOM, b == UnitKey::PHANTOM) {
            // Forcing a team onto the bye. Re-asking for a bye it already has is
            // a no-op, not a double entry.
            (true, false) | (false, true) => {
                let team = TeamId::from(if a == UnitKey::PHANTOM { b } else { a });
                if !forced_team_byes.contains(&team) {
                    forced_team_byes.push(team);
                }
            }
            _ => forced_matches.push(ForcedMatch {
                team1: TeamId::from(a),
                team2: TeamId::from(b),
            }),
        }
        let draft = RoundDraft {
            number: round.number,
            absent: round.absentees().collect(),
            forced_boards: Vec::new(),
            forced_byes: Vec::new(),
            forced_matches,
            forced_team_byes,
        };

        // Drop the round and re-confirm from the reconstructed draft. Earlier
        // rounds stay completed, so the standings entering this round are intact.
        // Un-carrying is what lets the re-confirmation rebuild any long game this
        // round was finishing: it puts the outcome back on a `LongStart` for the
        // carry to pick up again.
        self.pop_round_uncarrying();
        self.draft = Some(draft);
        self.confirm_round()
    }

    /// Why `id` is out of the engine's hands for `round`: forced, a cup player,
    /// or sitting out. Errors only if `id` isn't a player at all.
    fn scope_reason(
        &self,
        round: &Round,
        id: TournamentId,
    ) -> Result<ScopeReason, TournamentError> {
        if let Some(sitout) = round.sitout(id) {
            return Ok(match sitout.kind {
                SitoutKind::ForcedBye => ScopeReason::Forced,
                SitoutKind::CupBye { .. } => ScopeReason::Cup,
                // The engine's own bye would have passed the in_swiss check.
                SitoutKind::Absent | SitoutKind::Bye => ScopeReason::Absent,
            });
        }
        for bd in &round.boards {
            if bd.player1 == id || bd.player2 == id {
                return Ok(match bd.source {
                    PairingSource::Forced => ScopeReason::Forced,
                    PairingSource::Cup { .. } => ScopeReason::Cup,
                    PairingSource::Carried => ScopeReason::MidLongGame,
                    // A Swiss board would have passed the in_swiss check.
                    PairingSource::Swiss => ScopeReason::Absent,
                });
            }
        }
        if self.players.iter().any(|p| p.tournament_id == Some(id)) {
            Ok(ScopeReason::Absent)
        } else {
            Err(TournamentError::PlayerNotFound(Uuid::nil()))
        }
    }

    /// Toggle the winner of a board in response to a player being clicked.
    ///
    /// If `clicked` is already the recorded winner, the result is cleared (back
    /// to "not played"); otherwise `clicked` becomes the winner. This gives the
    /// three states — not played, player 1 won, player 2 won — from clicks alone.
    ///
    /// The round's `completed` flag is kept in sync automatically: a round is
    /// complete exactly when every board has a result, so recording the last
    /// game locks the round in (and clearing a result reopens it) with no
    /// separate "complete round" step.
    pub fn toggle_board_winner(
        &mut self,
        round_number: u32,
        board_index: usize,
        clicked: Winner,
    ) -> Result<&Board, TournamentError> {
        self.refuse_if_carried(round_number, board_index)?;
        let idx = self
            .rounds
            .iter()
            .position(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        let round = &mut self.rounds[idx];
        let board = round
            .boards
            .get_mut(board_index)
            .ok_or(TournamentError::BoardNotFound {
                round: round_number,
                board: board_index,
            })?;
        // Recording an actual result supersedes a no-show — the game was played
        // after all. A forfeited board carries no draw, so the replayed-draw flag
        // starts fresh there; on a played board it survives the toggle, since
        // whether a draw occurred is independent of who eventually won.
        let drawn = board.outcome().drawn();
        board.set_outcome(if board.outcome().winner() == Some(clicked) {
            Outcome::Pending { drawn }
        } else {
            Outcome::Won {
                winner: clicked,
                drawn,
            }
        });
        round.completed = round.is_complete();
        // Every later round was paired from a model that read this result.
        self.explanations_stale_after(round_number);
        Ok(&self.rounds[idx].boards[board_index])
    }

    /// Set what a round is worth to a player who sat it out — the `0+` / `0=` /
    /// `0−` in their cross-table cell.
    ///
    /// The tournament's
    /// [`half_point_absences`](crate::settings::TournamentSettings::half_point_absences)
    /// setting only picks the value an absence *starts* at when the round is
    /// confirmed; this is how a referee overrules it for one player in one round
    /// (an excused absence, a bye they judge shouldn't score, …). Completed
    /// rounds are fair game — re-scoring a past round is the whole point — and
    /// only the score moves: why the player sat out is untouched, so the facts a
    /// *re-pairing* would read from the kind (the bye they can't be given twice,
    /// the float history) are unaffected. The score itself does feed later
    /// rounds' pairing models, which is why this moves the explanation watermark.
    pub fn set_sitout_value(
        &mut self,
        round_number: u32,
        player: TournamentId,
        value: SitoutValue,
    ) -> Result<&Round, TournamentError> {
        let idx = self
            .rounds
            .iter()
            .position(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        let sitout = self.rounds[idx]
            .sitouts
            .iter_mut()
            .find(|s| s.player == player)
            .ok_or(TournamentError::PlayerNotSittingOut {
                round: round_number,
                player,
            })?;
        sitout.value = value;
        self.explanations_stale_after(round_number);
        Ok(&self.rounds[idx])
    }

    /// Re-score a round a whole **team** sat out (`0+` / `0=` / `0−`).
    ///
    /// A team sits out together — a bye or an absence is one entry per member —
    /// and what the round was worth to the team is read from those entries,
    /// which [`team_sitout`](crate::team_scoring) requires to agree. So the
    /// value is written to every member at once: re-scoring one member would
    /// leave a team whose own entries disagree about what it scored.
    ///
    /// Rejects a team that played that round (or has a member who did), naming
    /// the member without a sit-out, rather than half-applying.
    pub fn set_team_sitout_value(
        &mut self,
        round_number: u32,
        team: Uuid,
        value: SitoutValue,
    ) -> Result<&Round, TournamentError> {
        if !self.settings.team_mode() {
            return Err(TournamentError::NotATeamTournament);
        }
        if !self.teams.iter().any(|t| t.id == team) {
            return Err(TournamentError::TeamNotFound(team));
        }
        let members: Vec<TournamentId> = self
            .team_members(team)
            .iter()
            .filter_map(|p| p.tournament_id)
            .collect();
        let idx = self
            .rounds
            .iter()
            .position(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        let round = &mut self.rounds[idx];
        // Check every member first: a team that played has no team-level cell to
        // re-score, and writing the ones that do sit out would be a half-edit.
        if let Some(&player) = members
            .iter()
            .find(|&&m| !round.sitouts.iter().any(|s| s.player == m))
        {
            return Err(TournamentError::PlayerNotSittingOut {
                round: round_number,
                player,
            });
        }
        for sitout in round.sitouts.iter_mut() {
            if members.contains(&sitout.player) {
                sitout.value = value;
            }
        }
        self.explanations_stale_after(round_number);
        Ok(&self.rounds[idx])
    }

    /// Mark a board as forfeited, or clear it.
    ///
    /// `absent` names the side(s) that missed the board and why — see
    /// [`Forfeit`] — or `None` to clear it back to a normal unplayed board. A
    /// single missing side credits the opponent a free point exactly like a bye;
    /// [`Forfeit::Both`] leaves no winner (both take a zero loss). A forfeit
    /// isn't a played game, so recording one clears any actual result, draw flag
    /// and handicap on the board. Like recording a winner, this keeps the round's
    /// `completed` flag in sync — a forfeit counts toward closing the round.
    ///
    /// A [`justified`](crate::round::AbsenceKind::Justified) absence is rejected
    /// mode: an individual tournament excludes an absent player from the pairing
    /// before a board exists, so there is no board to mark, and accepting one
    /// would put a `0-` in the grid that nothing else in the tournament can
    /// explain.
    pub fn set_board_no_show(
        &mut self,
        round_number: u32,
        board_index: usize,
        absent: Option<Forfeit>,
    ) -> Result<&Board, TournamentError> {
        self.refuse_if_carried(round_number, board_index)?;
        if absent.is_some_and(Forfeit::has_justified) && !self.settings.team_mode() {
            return Err(TournamentError::JustifiedAbsenceOutsideTeamMode);
        }
        let idx = self
            .rounds
            .iter()
            .position(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        let round = &mut self.rounds[idx];
        let board = round
            .boards
            .get_mut(board_index)
            .ok_or(TournamentError::BoardNotFound {
                round: round_number,
                board: board_index,
            })?;
        board.set_outcome(match absent {
            // A forfeit isn't a played game, so it drops any recorded result and
            // draw — states the outcome type can't even express together.
            Some(absent) => Outcome::Forfeit { absent },
            // Clearing only ever un-forfeits: on a board that carries a real
            // result there is no forfeit to clear, and the result must survive.
            None if board.outcome().forfeit().is_some() => Outcome::PENDING,
            None => board.outcome(),
        });
        // Nobody played, so nobody conceded odds: a forfeit drops the handicap
        // too, the same way it drops the result and the draw flag. The handicap
        // is a separate field rather than part of the outcome, so it has to be
        // cleared explicitly — leaving it would keep a "X gives 2-piece" hint on
        // a board that was never played, publish it in the cross-table, and
        // resurrect it if the forfeit is later cleared.
        if absent.is_some() {
            board.handicap = None;
        }
        round.completed = round.is_complete();
        self.explanations_stale_after(round_number);
        Ok(&self.rounds[idx].boards[board_index])
    }

    /// Flag (or unflag) a board as a "long game": double time control, lasting two
    /// rounds and scoring two points for the winner (see
    /// `docs/archive/two-round-boards.md`).
    ///
    /// Allowed only on the **current** (last) round, and only when the tournament
    /// enables long boards. Flagging *on* requires the board undecided; flagging
    /// *off* is allowed even after a result, so the referee can demote a long game
    /// that actually finished in a single round (or resolved by forfeit) back to
    /// an ordinary one-point board. Keeps the round's `completed` flag in sync,
    /// since flagging the last-undecided board long can close the round.
    ///
    /// A cup board's flag couples to the whole cup round — including, in a
    /// qualifier cup's qualification round, the pre-qualified players' games.
    pub fn set_board_long(
        &mut self,
        round_number: u32,
        board_index: usize,
        long: bool,
    ) -> Result<&Round, TournamentError> {
        if !self.settings.long_boards_enabled {
            return Err(TournamentError::InvalidDraft(
                "long games are not enabled for this tournament".into(),
            ));
        }
        // Longness is frozen once a round advances, so only the last round's
        // boards can be toggled.
        let last_index = self.rounds.len().checked_sub(1);
        let idx = self
            .rounds
            .iter()
            .position(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        if Some(idx) != last_index {
            return Err(TournamentError::NotCurrentRound);
        }
        // Which boards this toggle moves — one definition, used both to refuse
        // the flip and to apply it. A guard that checks a smaller set than the
        // write is how a result already recorded gets doubled behind the
        // referee's back.
        //
        // A cup bracket round is long or not as a *unit*: that is the invariant
        // the cup<->tournament-round mapping relies on (see `Cup::cup_schedule`).
        // In a qualifier cup's qualification round the unit also takes in the
        // pre-qualified players' games. They are playing the same session of the
        // same cup — merely seeded past the play-off — so running them at a
        // different length would free them a round early and desynchronise them
        // from the bracket the qualifiers resume in. Their opponents that round
        // are ordinary Swiss players, and the game is long for them too.
        //
        // A board outside all of that toggles on its own.
        let prequalified: Vec<TournamentId> = self.prequalified_in_round(round_number).to_vec();
        let in_cup_unit = |b: &Board| {
            matches!(b.source, PairingSource::Cup { .. })
                || prequalified.contains(&b.player1)
                || prequalified.contains(&b.player2)
        };

        let clicked = self.board(round_number, board_index)?;
        let clicked_record = clicked.record;
        let clicked_decided = clicked.is_decided();
        let coupled = in_cup_unit(clicked);

        // The live record of a game carried from the previous round: its length
        // was decided there, together with the record it left behind, and the two
        // only mean anything as a pair. Demoting it here would turn it into an
        // ordinary board and orphan its partner — a file `validate_loaded` would
        // then refuse to load.
        if let GameRecord::LongEnd(_) = clicked_record {
            return Err(TournamentError::LongGameStartedEarlier {
                round: round_number - 1,
            });
        }
        // Making a board long after it is decided is meaningless; turning it off
        // after a result is the intended demote path. Where the flag couples, the
        // question is asked of the whole unit: flipping one cup board on flips
        // them all, so a result on *any* of them would be retroactively doubled.
        if long {
            if coupled {
                if self.rounds[idx]
                    .boards
                    .iter()
                    .any(|b| in_cup_unit(b) && b.is_decided())
                {
                    return Err(TournamentError::LongFlagAfterCoupledResult);
                }
            } else if clicked_decided {
                return Err(TournamentError::LongFlagAfterResult);
            }
        }

        let round = &mut self.rounds[idx];
        if coupled {
            for b in round.boards.iter_mut() {
                if in_cup_unit(b) {
                    b.set_long(long);
                }
            }
        } else {
            round.boards[board_index].set_long(long);
        }
        round.completed = round.is_complete();
        // A no-op as long as only the last round can be toggled (there is no
        // later round to have read the doubled score), but stated rather than
        // relied on: a long board is worth two points, which is pairing input.
        self.explanations_stale_after(round_number);
        Ok(&self.rounds[idx])
    }

    /// Set (or clear) the "a draw occurred" flag on a board. The game is still
    /// replayed to a decisive [`Winner`]; this only records that the draw
    /// happened, which matters for end-of-tournament ELO.
    ///
    /// Nobody played, so nobody drew: a forfeited board is rejected with
    /// [`TournamentError::DrawnOnForfeitedBoard`] rather than silently recording
    /// a draw that would then feed the ELO estimate. Clear the forfeit first.
    pub fn set_board_drawn(
        &mut self,
        round_number: u32,
        board_index: usize,
        drawn: bool,
    ) -> Result<&Board, TournamentError> {
        self.refuse_if_carried(round_number, board_index)?;
        let board = self.board_mut(round_number, board_index)?;
        board.set_outcome(match board.outcome() {
            Outcome::Pending { .. } => Outcome::Pending { drawn },
            Outcome::Won { winner, .. } => Outcome::Won { winner, drawn },
            Outcome::Forfeit { .. } => {
                return Err(TournamentError::DrawnOnForfeitedBoard {
                    round: round_number,
                    board: board_index,
                })
            }
        });
        // A draw feeds the live ELO estimate, which is what an ELO-mode
        // tournament pairs on, so later rounds' models read it.
        self.explanations_stale_after(round_number);
        self.board_mut(round_number, board_index).map(|b| &*b)
    }

    /// Set (`Some`) or clear (`None`) the handicap on a board.
    ///
    /// The giver — the higher-rated player — is computed from the players'
    /// current ratings and frozen onto the board, so a later rating edit can't
    /// change who conceded the odds. Returns
    /// [`TournamentError::HandicapNeedsRatingDifference`] when the two players'
    /// ratings are equal (or both unrated), since then there is no giver, or
    /// [`TournamentError::HandicapNotAllowedForCup`] for a cup board — cup games
    /// are always played even.
    ///
    /// Nobody played, so nobody conceded odds: a forfeited board is rejected with
    /// [`TournamentError::HandicapOnForfeitedBoard`], exactly like the draw flag.
    /// Clear the forfeit first.
    pub fn set_board_handicap(
        &mut self,
        round_number: u32,
        board_index: usize,
        handicap: Option<Handicap>,
    ) -> Result<&Board, TournamentError> {
        self.refuse_if_carried(round_number, board_index)?;
        // Rejected in both directions, like the draw flag: the picker is disabled
        // on a forfeited board, and the forfeit already dropped whatever handicap
        // the board carried, so even a clear means the client is out of sync.
        if self
            .board(round_number, board_index)?
            .outcome()
            .forfeit()
            .is_some()
        {
            return Err(TournamentError::HandicapOnForfeitedBoard {
                round: round_number,
                board: board_index,
            });
        }
        // Resolve the giver up front (immutable borrow of `players`) so the board
        // can then be borrowed mutably without conflict.
        let game = match handicap {
            None => None,
            Some(handicap) => {
                let board = self.board(round_number, board_index)?;
                if matches!(board.source, PairingSource::Cup { .. }) {
                    return Err(TournamentError::HandicapNotAllowedForCup);
                }
                let (p1, p2) = (board.player1, board.player2);
                let giver = Self::rating_giver(self.player_rating(p1), self.player_rating(p2))
                    .ok_or(TournamentError::HandicapNeedsRatingDifference)?;
                Some(HandicapGame { handicap, giver })
            }
        };
        self.board_mut(round_number, board_index)?.handicap = game;
        // Under the Wiel rule the handicap decides who the board scores for, and
        // that score is what later rounds were paired on.
        self.explanations_stale_after(round_number);
        self.board_mut(round_number, board_index).map(|b| &*b)
    }

    /// Set a board's handicap with an **explicit** giver, for an importer that
    /// reads the conceding side from its source rather than deriving it from
    /// ratings — the two can disagree (a cross-table records an unrated player
    /// conceding odds, say), and the source is the authority on what was played.
    pub(crate) fn set_board_handicap_from_source(
        &mut self,
        round_number: u32,
        board_index: usize,
        handicap: Handicap,
        giver: Winner,
    ) -> Result<(), TournamentError> {
        let board = self.board(round_number, board_index)?;
        if matches!(board.source, PairingSource::Cup { .. }) {
            return Err(TournamentError::HandicapNotAllowedForCup);
        }
        // The importer forces every board and resolves it straight away, so it
        // never meets a forfeited one — a cross-table's forfeit wins come in as
        // byes. Stated so a future importer that *does* record forfeits can't
        // quietly build the state `set_board_handicap` refuses.
        debug_assert!(
            board.outcome().forfeit().is_none(),
            "round {round_number} board {board_index} is forfeited, so it cannot take a handicap"
        );
        self.board_mut(round_number, board_index)?.handicap =
            Some(HandicapGame { handicap, giver });
        self.explanations_stale_after(round_number);
        Ok(())
    }

    /// The suggested handicap for a board, from the two players' current
    /// ratings — always `None` for cup boards, whatever their ratings.
    /// Display-only: never affects pairing and is never auto-filled.
    pub fn suggested_handicap_for_board(&self, board: &Board) -> Option<Handicap> {
        if matches!(board.source, PairingSource::Cup { .. }) {
            return None;
        }
        crate::handicap::suggested_handicap(
            self.player_rating(board.player1),
            self.player_rating(board.player2),
        )
    }

    /// Which side gives the handicap, given the two players' ratings: the higher
    /// rating gives, an unrated player counts as the lowest. Returns `None` when
    /// the ratings are equal (including both unrated), leaving no unambiguous
    /// giver — the caller treats that as an error.
    fn rating_giver(p1: Option<u32>, p2: Option<u32>) -> Option<Winner> {
        match (p1, p2) {
            (Some(a), Some(b)) if a > b => Some(Winner::Player1),
            (Some(a), Some(b)) if a < b => Some(Winner::Player2),
            (Some(_), None) => Some(Winner::Player1),
            (None, Some(_)) => Some(Winner::Player2),
            _ => None, // equal ratings or both unrated
        }
    }

    /// Mutable access to a player by id, or [`TournamentError::PlayerNotFound`].
    fn player_mut(&mut self, id: Uuid) -> Result<&mut Player, TournamentError> {
        self.players
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or(TournamentError::PlayerNotFound(id))
    }

    /// A player's registration rating (by tournament number, as boards carry), if
    /// the player exists and is rated.
    fn player_rating(&self, tid: TournamentId) -> Option<u32> {
        self.players
            .iter()
            .find(|p| p.tournament_id == Some(tid))
            .and_then(|p| p.rating)
    }

    /// Immutable access to a board by round number and index.
    fn board(&self, round_number: u32, board_index: usize) -> Result<&Board, TournamentError> {
        let round = self
            .rounds
            .iter()
            .find(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        round
            .boards
            .get(board_index)
            .ok_or(TournamentError::BoardNotFound {
                round: round_number,
                board: board_index,
            })
    }

    /// Mutable access to a board by round number and index.
    /// Refuse a mutation aimed at the inert record of a carried long game,
    /// naming the round its live record is in.
    ///
    /// That record holds nothing about the game — not the result, not the
    /// handicap — because the game is finished one round later and everything
    /// about it lives there. Writing here is how the two records of one game
    /// would come to disagree, so it is refused rather than allowed to become a
    /// silent no-op (a handicap that scores nothing) or a panic
    /// ([`GameRecord::with_outcome`] has nowhere to put an outcome).
    fn refuse_if_carried(
        &self,
        round_number: u32,
        board_index: usize,
    ) -> Result<(), TournamentError> {
        if self.board(round_number, board_index)?.record == GameRecord::LongCarried {
            return Err(TournamentError::CarriedLongGame {
                round: round_number + 1,
            });
        }
        Ok(())
    }

    fn board_mut(
        &mut self,
        round_number: u32,
        board_index: usize,
    ) -> Result<&mut Board, TournamentError> {
        let round = self
            .rounds
            .iter_mut()
            .find(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        round
            .boards
            .get_mut(board_index)
            .ok_or(TournamentError::BoardNotFound {
                round: round_number,
                board: board_index,
            })
    }

    /// Validate a tournament that was deserialized from an untrusted source (an
    /// uploaded save file).
    ///
    /// This is the *only* gate an imported file passes through, so every rule
    /// that [`Tournament::new`] enforces on a tournament we build ourselves has
    /// to be restated here — a file is not held to a constructor it never ran.
    /// Deserializing successfully proves the shape, not the invariants.
    pub fn validate_loaded(&self) -> Result<(), TournamentError> {
        if self.format_version != TOURNAMENT_FORMAT_VERSION {
            return Err(TournamentError::UnsupportedFormatVersion {
                found: self.format_version,
                supported: TOURNAMENT_FORMAT_VERSION,
            });
        }
        if self.name.trim().is_empty() {
            return Err(TournamentError::EmptyTournamentName);
        }
        self.validate_field()?;
        self.validate_rounds_name_the_field()?;
        // A justified absence on a board is a team-mode fact: an individual
        // tournament excludes an absent player before a board exists. A file
        // carrying one outside team mode would export `0-` cells nothing in the
        // tournament can account for, so it is rejected rather than half-trusted.
        if !self.settings.team_mode()
            && self
                .rounds
                .iter()
                .flat_map(|r| &r.boards)
                .filter_map(|b| b.outcome().forfeit())
                .any(Forfeit::has_justified)
        {
            return Err(TournamentError::JustifiedAbsenceOutsideTeamMode);
        }
        // Nobody played, so nobody conceded odds: a forfeited board carrying a
        // handicap is a state no setter here can produce (`set_board_no_show`
        // drops the handicap, `set_board_handicap` refuses a forfeited board).
        // Rejected rather than tolerated because the two disagree about whether
        // the game happened, and every reader — the cross-table, the FESA export,
        // the "X gives" hint — believes a different one. Nothing older loads into
        // this state either: the only backwards compatibility this build offers is
        // for a tournament that has not started, which has no boards at all.
        if let Some((round, board)) = self.rounds.iter().find_map(|r| {
            r.boards
                .iter()
                .position(|b| b.handicap.is_some() && b.outcome().forfeit().is_some())
                .map(|i| (r.number, i))
        }) {
            return Err(TournamentError::HandicapOnForfeitedBoard { round, board });
        }
        // A frozen explanation names the round it explains. A file where the two
        // disagree, or whose watermark points past the last round, would put a
        // rationale under the wrong pairings (or vouch for rounds that aren't
        // there) — both silent, both wrong.
        if let Some(round) = self.rounds.iter().find(|r| r.explanation.round != r.number) {
            return Err(TournamentError::MisplacedExplanation {
                round: round.number,
                explains: round.explanation.round,
            });
        }
        if self.explanations_faithful_through as usize > self.rounds.len() {
            return Err(TournamentError::WatermarkPastLastRound {
                mark: self.explanations_faithful_through,
                rounds: self.rounds.len(),
            });
        }
        // The cup is the one part of the record whose shape is load-bearing on the
        // *read* path: `cup_bracket()` replays the whole bracket for every
        // tournament response, and that replay panics on a size or a seed count
        // `finalize_registration_with` would have rejected. Only that constructor
        // enforced them, and a file never ran it.
        if let Some(cup) = &self.cup {
            cup.validate_shape()?;
            // A seed is a tournament number, and everything downstream — the
            // boards the bracket pairs, the score tables those boards feed —
            // resolves it against the field. One that names nobody is a corrupt
            // file, not a player who left (removing a seeded player is refused).
            let known: HashSet<TournamentId> = self
                .players
                .iter()
                .filter_map(|p| p.tournament_id)
                .collect();
            if let Some(&seed) = cup.seed_order.iter().find(|s| !known.contains(s)) {
                return Err(TournamentError::UnknownCupSeed { seed });
            }
        }
        self.validate_long_games()?;
        Ok(())
    }

    /// The pairing invariants of carried long games (`docs/long-boards-v2.md`).
    ///
    /// A long game holds one record per round it occupies, and the two are only
    /// meaningful together: the inert `LongCarried` one in the round it started,
    /// and the live `LongEnd` one in the round it is finished in. A file with one
    /// and not the other has a game whose result is either unreachable or
    /// unattributable, and nothing downstream would say so — the grid would render
    /// a column from a record that isn't there, and the cup would replay a
    /// bracket round whose result it cannot find.
    fn validate_long_games(&self) -> Result<(), TournamentError> {
        let paired_in = |number: u32, board: &Board, want: fn(&GameRecord) -> bool| {
            self.rounds
                .iter()
                .find(|r| r.number == number)
                .is_some_and(|r| {
                    r.boards.iter().any(|b| {
                        want(&b.record) && b.player1 == board.player1 && b.player2 == board.player2
                    })
                })
        };
        for round in &self.rounds {
            for board in &round.boards {
                match board.record {
                    // Its live record must be in the next round.
                    GameRecord::LongCarried
                        if !paired_in(round.number + 1, board, |r| {
                            matches!(r, GameRecord::LongEnd(_))
                        }) =>
                    {
                        return Err(TournamentError::OrphanedLongGame {
                            round: round.number,
                        })
                    }
                    // And a live record must have the record it came from behind
                    // it — which also rules it out of round 1, where there is no
                    // previous round to have started it.
                    GameRecord::LongEnd(_)
                        if round.number == 1
                            || !paired_in(round.number - 1, board, |r| {
                                matches!(r, GameRecord::LongCarried)
                            }) =>
                    {
                        return Err(TournamentError::OrphanedLongGame {
                            round: round.number,
                        })
                    }
                    // Its players are *playing* this round, so neither can also be
                    // sitting it out. Both at once double-scores them, and the
                    // sit-out takes precedence in the cross-table, hiding the game
                    // from the export entirely.
                    GameRecord::LongEnd(_)
                        if round.sitout(board.player1).is_some()
                            || round.sitout(board.player2).is_some() =>
                    {
                        return Err(TournamentError::OrphanedLongGame {
                            round: round.number,
                        })
                    }
                    // A game must have been carried once a later round exists: it
                    // spans two rounds, so a start still sitting there un-carried
                    // means the round after it was built without it.
                    GameRecord::LongStart(_)
                        if self.rounds.iter().any(|r| r.number > round.number) =>
                    {
                        return Err(TournamentError::OrphanedLongGame {
                            round: round.number,
                        })
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// The registered players themselves: distinct ids, distinct tournament
    /// numbers, and a number each once registration is finalized.
    ///
    /// [`compute_scores`](crate::scoring::compute_scores) keys every score by
    /// tournament number and reaches for it with `tournament_id.unwrap()`, so a
    /// finalized file with an unnumbered player panics the first `GET` that
    /// derives standings. Two players sharing a number is quieter and worse:
    /// they share one score slot, and the table simply reports the wrong
    /// results. Both are states [`finalize_registration_with`] cannot produce
    /// and a file has never been held to.
    ///
    /// [`finalize_registration_with`]: Self::finalize_registration_with
    fn validate_field(&self) -> Result<(), TournamentError> {
        let mut ids = HashSet::with_capacity(self.players.len());
        let mut numbers = HashSet::with_capacity(self.players.len());
        for player in &self.players {
            if !ids.insert(player.id) {
                return Err(TournamentError::DuplicatePlayerId { player: player.id });
            }
            match player.tournament_id {
                Some(number) => {
                    if !numbers.insert(number) {
                        return Err(TournamentError::DuplicateTournamentNumber { number });
                    }
                }
                // Before finalization nobody has a number yet, and nothing reads
                // one — `standings` returns empty rather than score an
                // unnumbered field.
                None if !self.registration_finalized => {}
                None => {
                    return Err(TournamentError::UnnumberedPlayer { player: player.id });
                }
            }
        }
        Ok(())
    }

    /// The rounds: numbered `1..=n` in order, and naming only players the file
    /// actually registers.
    ///
    /// A board or sit-out citing a number nobody has indexes past the end of the
    /// score vector — another panic on the read path rather than a rejection.
    /// A board pairing a player with themselves would count one game twice for
    /// them. The numbering matters because a round is identified by its
    /// `number` in every request, while the pairing model, the staleness
    /// watermark and the cup stages all read `rounds` as a positional prefix:
    /// where the two disagree, an edit lands on a different round than the one
    /// the referee is looking at.
    fn validate_rounds_name_the_field(&self) -> Result<(), TournamentError> {
        let known: HashSet<TournamentId> = self
            .players
            .iter()
            .filter_map(|p| p.tournament_id)
            .collect();
        for (index, round) in self.rounds.iter().enumerate() {
            let expected = index as u32 + 1;
            if round.number != expected {
                return Err(TournamentError::MisnumberedRound {
                    expected,
                    found: round.number,
                });
            }
            for board in &round.boards {
                for player in [board.player1, board.player2] {
                    if !known.contains(&player) {
                        return Err(TournamentError::UnknownRoundPlayer {
                            round: round.number,
                            player,
                        });
                    }
                }
                if board.player1 == board.player2 {
                    return Err(TournamentError::BoardAgainstSelf {
                        round: round.number,
                        player: board.player1,
                    });
                }
            }
            if let Some(sitout) = round.sitouts.iter().find(|s| !known.contains(&s.player)) {
                return Err(TournamentError::UnknownRoundPlayer {
                    round: round.number,
                    player: sitout.player,
                });
            }
            // One game, or one reason there was no game — never two of either,
            // and never one of each. See [`TournamentError::PlayerTwiceInRound`]
            // for what a second entry costs.
            if let Some(player) = first_repeat(round_participants(round)) {
                return Err(TournamentError::PlayerTwiceInRound {
                    round: round.number,
                    player,
                });
            }
        }
        Ok(())
    }
}

/// Everyone the round accounts for: both sides of every board, and every
/// sit-out. Exactly the players a round is allowed to name once each.
fn round_participants(round: &Round) -> impl Iterator<Item = TournamentId> + '_ {
    round
        .boards
        .iter()
        .flat_map(|b| [b.player1, b.player2])
        .chain(round.sitouts.iter().map(|s| s.player))
}

/// The first value that appears twice, if any.
///
/// Shared by the load-time check above and by the absent-list check both
/// confirmation paths run ([`Tournament::confirm_round`] and
/// [`Tournament::confirm_team_round`]), so "a player takes part once" is one
/// rule written once rather than the same loop spelled out per call site.
pub(crate) fn first_repeat<T: Eq + std::hash::Hash + Copy>(
    items: impl IntoIterator<Item = T>,
) -> Option<T> {
    let mut seen = HashSet::new();
    items.into_iter().find(|&item| !seen.insert(item))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::AbsenceKind;

    fn named(last_name: &str) -> NewPlayer {
        NewPlayer {
            last_name: last_name.to_string(),
            ..Default::default()
        }
    }

    /// Prepare and confirm the next round with no customization.
    fn start_next_round(t: &mut Tournament) {
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
    }

    /// A finalized four-player tournament with long boards enabled, round 1
    /// confirmed (two boards). Returns the tournament.
    fn four_players_round1_with_long_enabled() -> Tournament {
        let mut t = Tournament::new("Long").unwrap();
        for n in ["A", "B", "C", "D"] {
            t.add_player(named(n)).unwrap();
        }
        t.settings.long_boards_enabled = true;
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        t
    }

    #[test]
    fn set_board_long_is_gated_by_the_setting_and_the_result_state() {
        let mut t = four_players_round1_with_long_enabled();
        // Disabled → rejected even though everything else is valid.
        t.settings.long_boards_enabled = false;
        assert!(matches!(
            t.set_board_long(1, 0, true),
            Err(TournamentError::InvalidDraft(_))
        ));
        t.settings.long_boards_enabled = true;

        // Flagging an undecided board on is fine.
        t.set_board_long(1, 0, true).unwrap();
        assert!(t.rounds[0].boards[0].is_long());

        // Flagging a *decided* board on is refused...
        t.toggle_board_winner(1, 1, Winner::Player1).unwrap();
        assert_eq!(
            t.set_board_long(1, 1, true),
            Err(TournamentError::LongFlagAfterResult)
        );
        // ...but flagging a decided long board *off* (the demote path) is allowed.
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        assert!(t.rounds[0].boards[0].is_decided());
        t.set_board_long(1, 0, false).unwrap();
        assert!(!t.rounds[0].boards[0].is_long());
    }

    /// The carry, end to end: a long game started in round 1 gets its live record
    /// in round 2 — the round it is actually finished in — and the record it came
    /// from goes inert. Round 2 is then an ordinary round with an ordinary
    /// undecided board, which is what gates round 3.
    #[test]
    fn a_long_game_is_carried_into_the_round_it_is_finished_in() {
        let mut t = Tournament::new("Long").unwrap();
        for n in ["Alpha", "Bravo"] {
            t.add_player(named(n)).unwrap();
        }
        t.settings.long_boards_enabled = true;
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        t.set_board_long(1, 0, true).unwrap();
        assert!(
            t.rounds[0].completed,
            "a long board does not hold its own round open"
        );
        let players = (t.rounds[0].boards[0].player1, t.rounds[0].boards[0].player2);

        start_next_round(&mut t);

        // Round 1's record is now inert, and round 2 holds the live one.
        assert_eq!(t.rounds[0].boards[0].record, GameRecord::LongCarried);
        assert_eq!(
            t.rounds[1].boards.len(),
            1,
            "the carried game, and nothing else"
        );
        let carried = &t.rounds[1].boards[0];
        assert_eq!(carried.record, GameRecord::LongEnd(Outcome::PENDING));
        assert_eq!(carried.source, PairingSource::Carried);
        assert_eq!((carried.player1, carried.player2), players);
        assert!(
            t.rounds[1].sitouts.is_empty(),
            "they are playing, not sitting out"
        );

        // Round 2 is held open by that board like any other, so round 3 is gated
        // by the ordinary rule rather than by a special long-game guard.
        assert!(!t.rounds[1].completed);
        assert_eq!(
            t.prepare_round(),
            Err(TournamentError::PreviousRoundNotComplete)
        );

        // The result is entered on the live record, in round 2.
        t.toggle_board_winner(2, 0, Winner::Player1).unwrap();
        assert!(t.rounds[1].completed);
        t.prepare_round().expect("round 3 prepares");
    }

    /// A round can be born complete, and the carry is what makes that reachable:
    /// a long game finished inside round 1 is carried into round 2 *already
    /// decided*, so round 2 has nothing left to record the moment it is
    /// confirmed. `completed` is otherwise only recomputed when a board changes,
    /// and there is no board change coming — stamping it `false` at construction
    /// left such a round permanently unfinishable and every later round
    /// unpreparable.
    #[test]
    fn a_round_born_with_nothing_left_to_record_is_complete_at_once() {
        let mut t = Tournament::new("Long").unwrap();
        for n in ["Alpha", "Bravo"] {
            t.add_player(named(n)).unwrap();
        }
        t.settings.long_boards_enabled = true;
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        t.set_board_long(1, 0, true).unwrap();
        // The long game finishes inside its own round. It is carried anyway — it
        // still took both rounds — so round 2 receives it already decided.
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();

        start_next_round(&mut t);
        assert_eq!(
            t.rounds[1].boards[0].record,
            GameRecord::LongEnd(Outcome::won(Winner::Player1)),
            "the outcome moved onto the live record"
        );
        assert_eq!(t.rounds[0].boards[0].record, GameRecord::LongCarried);
        assert!(
            t.rounds[1].completed,
            "nothing left to record, so nothing to wait for"
        );
        t.prepare_round().expect("round 3 prepares");
    }

    #[test]
    fn long_board_spans_two_rounds_completes_early_and_gates_the_next() {
        let mut t = four_players_round1_with_long_enabled();
        assert_eq!(t.rounds[0].boards.len(), 2);

        // Board 0 is long; record only the other board.
        t.set_board_long(1, 0, true).unwrap();
        let long_players = [t.rounds[0].boards[0].player1, t.rounds[0].boards[0].player2];
        t.toggle_board_winner(1, 1, Winner::Player1).unwrap();

        // Round 1 is complete even though the long board is still unplayed.
        assert!(t.rounds[0].completed);
        assert!(!t.rounds[0].boards[0].is_decided());

        // Round 2 does not *pair* the two long players: their only board there is
        // the carried record of the game they are still playing.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let r2 = t.rounds.last().unwrap();
        for b in &r2.boards {
            let theirs = long_players.contains(&b.player1) || long_players.contains(&b.player2);
            assert!(!theirs || b.source == PairingSource::Carried);
        }
        assert!(r2.byes().all(|x| !long_players.contains(&x)));

        // The long flag can no longer be touched on round 1 (not the current round).
        assert_eq!(
            t.set_board_long(1, 0, false),
            Err(TournamentError::NotCurrentRound)
        );

        // Round 2 holds the carried game, so recording every *other* board leaves
        // it open — and round 3 is gated by that, not by a special guard.
        let carried = t.rounds[1]
            .boards
            .iter()
            .position(|b| b.source == PairingSource::Carried)
            .expect("the carried game is a board of round 2");
        let n = t.rounds[1].boards.len();
        for i in (0..n).filter(|i| *i != carried) {
            t.toggle_board_winner(2, i, Winner::Player1).unwrap();
        }
        assert!(!t.rounds[1].completed);
        assert_eq!(
            t.prepare_round(),
            Err(TournamentError::PreviousRoundNotComplete)
        );

        // Enter the long result on its live record; now the next round prepares.
        t.toggle_board_winner(2, carried, Winner::Player1).unwrap();
        assert!(t.rounds[1].completed);
        assert!(t.prepare_round().is_ok());
    }

    /// A long game is *one* game played across *two* rounds — which is exactly
    /// why its winner scores two points. So it occupies rounds N and N+1
    /// whichever round it actually finishes in. Finishing early (a quick
    /// resignation, or a no-show) does not hand its players a second game.
    ///
    /// The referee who decides it was really a one-round game unticks the box
    /// before the round advances, demoting it to an ordinary one-point board;
    /// that is the escape hatch, and the only one.
    #[test]
    fn a_long_game_decided_early_still_occupies_the_next_round() {
        let mut t = four_players_round1_with_long_enabled();
        t.set_board_long(1, 0, true).unwrap();
        let long_players = [t.rounds[0].boards[0].player1, t.rounds[0].boards[0].player2];

        // The long game ends inside round 1, and so does the ordinary board.
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        t.toggle_board_winner(1, 1, Winner::Player1).unwrap();
        assert!(t.rounds[0].completed);

        start_next_round(&mut t);
        let r2 = t.rounds.last().unwrap();
        for b in &r2.boards {
            let theirs = long_players.contains(&b.player1) || long_players.contains(&b.player2);
            assert!(
                !theirs || b.source == PairingSource::Carried,
                "a long game takes rounds N and N+1 even when it finishes in N, so \
                 their only board here is the carried one"
            );
        }
        assert!(r2.byes().all(|x| !long_players.contains(&x)));
    }

    /// The scoring consequence of the rule above, and the reason it matters:
    /// a long board is worth two points, so freeing its players for the next
    /// round lets them take three wins out of two rounds while the rest of the
    /// field can take at most two. That is a wrong final standing.
    ///
    /// Uses a no-show, which is the likeliest way to reach it in practice: the
    /// player who turned up banks the long weight without playing a move.
    #[test]
    fn a_long_board_cannot_buy_three_wins_in_two_rounds() {
        let mut t = four_players_round1_with_long_enabled();
        t.set_board_long(1, 0, true).unwrap();
        let winner = t.rounds[0].boards[0].player1;

        // Player 2 doesn't turn up: the long board resolves at its long weight.
        t.set_board_no_show(1, 0, Some(Forfeit::Player2(AbsenceKind::NoShow)))
            .unwrap();
        t.toggle_board_winner(1, 1, Winner::Player1).unwrap();
        assert!(t.rounds[0].completed);

        // Round 2. If the winner is (wrongly) paired again, let them win that too.
        start_next_round(&mut t);
        let number = t.rounds.last().unwrap().number;
        let seat = t
            .rounds
            .last()
            .unwrap()
            .boards
            .iter()
            .position(|b| b.player1 == winner || b.player2 == winner)
            .map(|i| {
                let side = if t.rounds.last().unwrap().boards[i].player1 == winner {
                    Winner::Player1
                } else {
                    Winner::Player2
                };
                (i, side)
            });
        if let Some((i, side)) = seat {
            t.toggle_board_winner(number, i, side).unwrap();
        }

        let standing = t
            .standings()
            .into_iter()
            .find(|s| {
                t.players
                    .iter()
                    .any(|p| p.id == s.player_id && p.tournament_id == Some(winner))
            })
            .expect("the long winner is in the standings");
        assert!(
            standing.victories <= crate::Wins::from_whole(2),
            "two completed rounds cannot yield more than two wins, got {:?}",
            standing.victories
        );
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
        assert_eq!(
            t.add_player(named("  ")),
            Err(TournamentError::EmptyPlayerName)
        );
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
                    first_name: Some("Anne".into()),
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
    fn point_adjustment_add_and_remove_round_trip() {
        let mut t = Tournament::new("Paris Open").unwrap();
        let id = t.add_player(named("Alice")).unwrap().id;

        let player = t
            .add_point_adjustment(id, 2, "fair-play bonus".into())
            .unwrap();
        assert_eq!(player.adjustments.len(), 1);
        assert_eq!(player.adjustments[0].delta, 2);
        assert_eq!(player.adjustments[0].reason, "fair-play bonus");
        let adjustment_id = player.adjustments[0].id;

        let player = t.remove_point_adjustment(id, adjustment_id).unwrap();
        assert!(player.adjustments.is_empty());
        assert_eq!(
            t.remove_point_adjustment(id, adjustment_id),
            Err(TournamentError::AdjustmentNotFound {
                player: id,
                adjustment: adjustment_id
            })
        );
    }

    #[test]
    fn point_adjustment_rejects_blank_reason_and_zero_delta() {
        let mut t = Tournament::new("Paris Open").unwrap();
        let id = t.add_player(named("Alice")).unwrap().id;
        assert_eq!(
            t.add_point_adjustment(id, 1, "  ".into()),
            Err(TournamentError::EmptyAdjustmentReason)
        );
        assert_eq!(
            t.add_point_adjustment(id, 0, "typo".into()),
            Err(TournamentError::ZeroPointAdjustment)
        );
        let missing = uuid::Uuid::new_v4();
        assert_eq!(
            t.add_point_adjustment(missing, 1, "reason".into()),
            Err(TournamentError::PlayerNotFound(missing))
        );
    }

    #[test]
    fn remove_player_works_and_reports_missing() {
        let mut t = Tournament::new("Paris Open").unwrap();
        let id = t.add_player(named("Alice")).unwrap().id;
        assert!(t.remove_player(id).is_ok());
        assert!(t.players.is_empty());
        assert_eq!(
            t.remove_player(id),
            Err(TournamentError::PlayerNotFound(id))
        );
    }

    #[test]
    fn cannot_remove_a_player_who_has_played() {
        let mut t = Tournament::new("Open").unwrap();
        for n in ["A", "B", "C", "D"] {
            t.add_player(named(n)).unwrap();
        }
        t.finalize_registration().unwrap();
        // Before any round, a player can still be removed.
        let d = t.players.iter().find(|p| p.last_name == "D").unwrap().id;
        assert!(t.remove_player(d).is_ok());
        // Once a round is confirmed, everyone paired onto a board is locked in:
        // their results are about to feed every opponent's tie-breaks.
        start_next_round(&mut t);
        let played_tid = t.rounds[0].boards[0].player1;
        let played = t
            .players
            .iter()
            .find(|p| p.tournament_id == Some(played_tid))
            .unwrap()
            .id;
        assert_eq!(
            t.remove_player(played),
            Err(TournamentError::CannotRemovePlayedPlayer)
        );
        assert_eq!(t.players.len(), 3, "the played player is still present");
    }

    /// Five players, round 1 confirmed with `absent_name` marked absent, every
    /// board decided. Returns the tournament and that player's id — the "said
    /// they would be late, then never came" case.
    fn five_players_with_an_absentee(absent_name: &str) -> (Tournament, Uuid) {
        let mut t = Tournament::new("Open").unwrap();
        // Distinctive names: the grid is matched as text below, and a
        // single-letter name collides with its column headers.
        for n in ["Alpha", "Bravo", "Charlie", "Delta", "Echo"] {
            t.add_player(named(n)).unwrap();
        }
        t.finalize_registration().unwrap();
        let who = t
            .players
            .iter()
            .find(|p| p.last_name == absent_name)
            .unwrap();
        let (id, tid) = (who.id, who.tournament_id.unwrap());
        t.prepare_round().unwrap();
        t.update_draft(vec![tid], Vec::new(), Vec::new()).unwrap();
        t.confirm_round().unwrap();
        for i in 0..t.rounds[0].boards.len() {
            t.toggle_board_winner(1, i, Winner::Player1).unwrap();
        }
        (t, id)
    }

    /// A player who never played can be removed mid-tournament and leaves no
    /// trace. The number they held is *not* reused, so the score table keeps a
    /// hole where they were — which is fine below the highest number in play,
    /// and used to index past the end when the removal freed that highest
    /// number itself.
    #[test]
    fn removing_an_absentee_leaves_no_dangling_sitout() {
        // "Charlie" holds a middle number, "Echo" the highest one — the case
        // that used to panic, because the highest number is what sizes the
        // table it was still referenced from.
        for name in ["Charlie", "Echo"] {
            let (mut t, who) = five_players_with_an_absentee(name);
            assert!(t.rounds[0]
                .sitouts
                .iter()
                .any(|s| s.kind == SitoutKind::Absent
                    && Some(s.player)
                        == t.players
                            .iter()
                            .find(|p| p.id == who)
                            .unwrap()
                            .tournament_id));

            t.remove_player(who).unwrap();

            assert!(
                t.rounds[0].sitouts.is_empty(),
                "{name}: their sit-out went with them"
            );
            let standings = t.standings();
            assert_eq!(standings.len(), 4, "{name}: four players left");
            assert!(
                standings.iter().all(|s| s.player_id != who),
                "{name}: and none of them is the one who never came"
            );
            // The american grid is built from the same records, and no longer
            // has a line for them.
            let grid = crate::american_grid::to_grid(&t, &standings).unwrap();
            assert!(
                !grid.contains(name),
                "{name}: gone from the american grid too"
            );
        }
    }

    /// The open draft names the absent player too, and confirming it would pair
    /// a number no player answers to.
    #[test]
    fn removing_a_player_clears_them_from_the_open_draft() {
        let (mut t, who) = five_players_with_an_absentee("Echo");
        let tid = t
            .players
            .iter()
            .find(|p| p.id == who)
            .unwrap()
            .tournament_id
            .unwrap();
        t.prepare_round().unwrap();
        t.update_draft(vec![tid], Vec::new(), Vec::new()).unwrap();
        assert_eq!(t.draft.as_ref().unwrap().absent, vec![tid]);

        t.remove_player(who).unwrap();

        assert!(t.draft.as_ref().unwrap().absent.is_empty());
        t.confirm_round().expect("round 2 confirms without them");
    }

    /// The engine's bye is named in that round's frozen explanation, so removing
    /// the player who took it drops the faithfulness watermark. A referee-forced
    /// bye is not the engine's choice and appears in no ledger, so it does not.
    #[test]
    fn only_an_engine_bye_stales_the_explanations_when_its_player_leaves() {
        // Odd field, nobody absent: the engine hands out the bye itself.
        let mut t = Tournament::new("Open").unwrap();
        for n in ["A", "B", "C"] {
            t.add_player(named(n)).unwrap();
        }
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        let bye = t.rounds[0]
            .sitouts
            .iter()
            .find(|s| s.kind == SitoutKind::Bye)
            .unwrap()
            .player;
        let bye_id = t
            .players
            .iter()
            .find(|p| p.tournament_id == Some(bye))
            .unwrap()
            .id;
        assert_eq!(t.explanations_faithful_through, 1);
        t.remove_player(bye_id).unwrap();
        assert_eq!(
            t.explanations_faithful_through, 0,
            "the ledger named them, so it can no longer be trusted"
        );

        // Same shape, but the referee fixed the bye by hand.
        let mut t = Tournament::new("Open").unwrap();
        for n in ["A", "B", "C"] {
            t.add_player(named(n)).unwrap();
        }
        t.finalize_registration().unwrap();
        let forced = t.players[2].tournament_id.unwrap();
        let forced_id = t.players[2].id;
        t.prepare_round().unwrap();
        t.update_draft(Vec::new(), Vec::new(), vec![forced])
            .unwrap();
        t.confirm_round().unwrap();
        assert_eq!(
            t.rounds[0].sitout(forced).unwrap().kind,
            SitoutKind::ForcedBye
        );
        assert_eq!(t.explanations_faithful_through, 1);
        t.remove_player(forced_id).unwrap();
        assert_eq!(
            t.explanations_faithful_through, 1,
            "no ledger mentioned a bye the engine did not choose"
        );
    }

    #[test]
    fn start_round_pairs_players_and_numbers_rounds() {
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B", "C"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        {
            let round = t.confirm_round().unwrap();
            assert_eq!(round.number, 1);
            assert_eq!(round.boards.len(), 1); // 3 players → 1 board + 1 bye
            assert!(round.swiss_bye().is_some());
        }

        // Playing the only board completes round 1 automatically, unlocking round 2.
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        assert!(t.rounds[0].completed);

        start_next_round(&mut t);
        assert_eq!(t.rounds.last().unwrap().number, 2);
        assert_eq!(t.rounds.len(), 2);
    }

    #[test]
    fn finalize_assigns_tournament_ids_by_elo() {
        fn rated(name: &str, rating: u32) -> NewPlayer {
            NewPlayer {
                last_name: name.to_string(),
                rating: Some(rating),
                ..Default::default()
            }
        }

        let mut t = Tournament::new("Cup").unwrap();
        // Registered out of ELO order.
        let low = t.add_player(rated("Low", 1000)).unwrap().id;
        let high = t.add_player(rated("High", 2000)).unwrap().id;
        let unrated = t.add_player(named("Unrated")).unwrap().id;
        let mid = t.add_player(rated("Mid", 1500)).unwrap().id;
        assert!(t.players.iter().all(|p| p.tournament_id.is_none()));

        t.finalize_registration().unwrap();
        let id_of = |uuid| {
            t.players
                .iter()
                .find(|p| p.id == uuid)
                .unwrap()
                .tournament_id
                .map(|t| t.0)
        };
        assert_eq!(id_of(high), Some(1));
        assert_eq!(id_of(mid), Some(2));
        assert_eq!(id_of(low), Some(3));
        assert_eq!(id_of(unrated), Some(4)); // unrated last

        // Added after finalization → next free number, regardless of rating.
        let newcomer = t.add_player(rated("Newcomer", 9000)).unwrap();
        assert_eq!(newcomer.tournament_id.map(|t| t.0), Some(5));
    }

    #[test]
    fn round_flow_is_gated() {
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        // Can't prepare before finalizing.
        assert_eq!(
            t.prepare_round(),
            Err(TournamentError::RegistrationNotFinalized)
        );
        t.finalize_registration().unwrap();
        assert_eq!(
            t.finalize_registration(),
            Err(TournamentError::RegistrationAlreadyFinalized)
        );
        t.prepare_round().unwrap();
        // Can't prepare a second draft while one exists.
        assert_eq!(t.prepare_round(), Err(TournamentError::DraftAlreadyExists));
        t.confirm_round().unwrap();
        // Can't prepare round 2 while round 1 is in progress (a game unplayed).
        assert_eq!(
            t.prepare_round(),
            Err(TournamentError::PreviousRoundNotComplete)
        );
        // Playing the last game completes the round automatically.
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        assert!(t.rounds[0].completed);
        // Now round 2 can be prepared and started.
        start_next_round(&mut t);
        assert_eq!(t.rounds.len(), 2);
    }

    #[test]
    fn cancel_last_round_peels_one_stage() {
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();

        // Nothing to cancel right after finalizing.
        assert_eq!(t.cancel_last_round(), Err(TournamentError::NoRoundToCancel));

        // A draft is peeled first. With no round behind it, this is the whole of
        // what preparing round 1 did, so registration reopens with it — see
        // `cancel_the_first_draft_reopens_registration`.
        t.prepare_round().unwrap();
        assert!(t.draft.is_some());
        t.cancel_last_round().unwrap();
        assert!(t.draft.is_none());
        assert!(t.rounds.is_empty());
        assert!(!t.registration_finalized);

        // Play round 1 (recording the game completes it).
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        assert_eq!(t.rounds.len(), 1);

        // With a draft open for round 2, cancel drops the draft but keeps round 1.
        t.prepare_round().unwrap();
        t.cancel_last_round().unwrap();
        assert!(t.draft.is_none());
        assert_eq!(t.rounds.len(), 1);
        assert!(t.rounds[0].completed);

        // No draft now: cancel removes round 1, all the way back to registration
        // (finalize is undone too: tournament numbers are cleared).
        t.cancel_last_round().unwrap();
        assert!(t.rounds.is_empty());
        assert!(!t.registration_finalized);
        assert!(t.players.iter().all(|p| p.tournament_id.is_none()));
        assert_eq!(t.cancel_last_round(), Err(TournamentError::NoRoundToCancel));
    }

    #[test]
    fn cancelling_the_first_draft_reopens_registration() {
        // Preparing round 1 is one click that finalizes registration *and* opens
        // the draft. Discarding that draft has to undo both, or the referee is
        // left with registration closed, no draft, no round — a step ahead of
        // where they clicked, with no way back but undo.
        let mut t = Tournament::new("Open").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();

        t.cancel_last_round().unwrap();
        assert!(!t.registration_finalized, "registration is open again");
        assert!(t.draft.is_none());
        assert!(t.rounds.is_empty());
        // The numbers go with it, exactly as when round 1 itself is cancelled.
        assert!(t.players.iter().all(|p| p.tournament_id.is_none()));
        // And the roster is editable again — the case that is outright blocked
        // in team mode, where late registration is refused.
        assert!(t.add_player(named("C")).is_ok());
    }

    #[test]
    fn cancelling_a_later_draft_leaves_registration_closed() {
        // The mirror image: with round 1 played, finalization belongs to that
        // round and not to the draft being discarded, so it stays — and the
        // numbers round 1 references stay with it.
        let mut t = Tournament::new("Open").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();

        t.prepare_round().unwrap();
        t.cancel_last_round().unwrap();
        assert!(t.registration_finalized, "round 1 still needs its numbers");
        assert!(t.draft.is_none());
        assert_eq!(t.rounds.len(), 1);
        assert!(t.players.iter().all(|p| p.tournament_id.is_some()));
    }

    #[test]
    fn cancel_last_round_keeps_the_previous_round_complete() {
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();

        // Play rounds 1 and 2 (recording each game completes its round).
        start_next_round(&mut t);
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        start_next_round(&mut t);
        t.toggle_board_winner(2, 0, Winner::Player1).unwrap();
        assert_eq!(t.rounds.len(), 2);

        // Cancelling round 2 removes it but leaves round 1's recorded games
        // intact — so round 1 stays complete, ready to re-prepare round 2.
        t.cancel_last_round().unwrap();
        assert_eq!(t.rounds.len(), 1);
        assert!(t.rounds[0].completed);
        assert!(t.registration_finalized);
    }

    #[test]
    fn cancel_last_round_can_drop_an_in_progress_round() {
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        start_next_round(&mut t); // round 1 in progress (not completed)
        assert_eq!(t.rounds.len(), 1);
        assert!(!t.rounds[0].completed);

        t.cancel_last_round().unwrap();
        assert!(t.rounds.is_empty());
        assert!(t.draft.is_none());
    }

    #[test]
    fn toggle_board_winner_cycles_states() {
        use crate::round::Winner;
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        start_next_round(&mut t);

        // not played -> player 1 wins
        assert_eq!(
            t.toggle_board_winner(1, 0, Winner::Player1)
                .unwrap()
                .outcome(),
            Outcome::won(Winner::Player1)
        );
        // click player 2 -> switch winner
        assert_eq!(
            t.toggle_board_winner(1, 0, Winner::Player2)
                .unwrap()
                .outcome(),
            Outcome::won(Winner::Player2)
        );
        // click the current winner again -> back to not played
        assert_eq!(
            t.toggle_board_winner(1, 0, Winner::Player2)
                .unwrap()
                .outcome(),
            Outcome::PENDING
        );
    }

    /// A draw recorded before the decisive replay survives the winner toggle in
    /// both directions: whether a draw happened is independent of who eventually
    /// won, and clearing the winner leaves the game still to be replayed.
    #[test]
    fn toggling_the_winner_keeps_the_draw_flag() {
        use crate::round::Winner;
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        start_next_round(&mut t);

        t.set_board_drawn(1, 0, true).unwrap();
        assert_eq!(
            t.toggle_board_winner(1, 0, Winner::Player1)
                .unwrap()
                .outcome(),
            Outcome::Won {
                winner: Winner::Player1,
                drawn: true
            }
        );
        assert_eq!(
            t.toggle_board_winner(1, 0, Winner::Player1)
                .unwrap()
                .outcome(),
            Outcome::Pending { drawn: true }
        );
    }

    #[test]
    fn no_show_completes_the_round_and_is_exclusive_with_a_result() {
        use crate::round::Winner;
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        start_next_round(&mut t);

        // Marking a no-show settles the only board, so the round completes even
        // though no game was played.
        let board = t
            .set_board_no_show(1, 0, Some(Forfeit::Player2(AbsenceKind::NoShow)))
            .unwrap();
        assert_eq!(
            board.outcome(),
            Outcome::Forfeit {
                absent: Forfeit::Player2(AbsenceKind::NoShow)
            }
        );
        assert!(t.rounds[0].completed);

        // Recording an actual winner supersedes the no-show (game was played).
        let board = t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        assert_eq!(board.outcome(), Outcome::won(Winner::Player1));

        // And marking a no-show again clears the recorded result.
        let board = t
            .set_board_no_show(1, 0, Some(Forfeit::Player1(AbsenceKind::NoShow)))
            .unwrap();
        assert_eq!(
            board.outcome(),
            Outcome::Forfeit {
                absent: Forfeit::Player1(AbsenceKind::NoShow)
            }
        );

        // Both players absent settles the board too, with no winner.
        let board = t
            .set_board_no_show(
                1,
                0,
                Some(Forfeit::Both(AbsenceKind::NoShow, AbsenceKind::NoShow)),
            )
            .unwrap();
        assert_eq!(
            board.outcome(),
            Outcome::Forfeit {
                absent: Forfeit::Both(AbsenceKind::NoShow, AbsenceKind::NoShow)
            }
        );
        assert!(t.rounds[0].completed);

        // Clearing it reopens the round.
        t.set_board_no_show(1, 0, None).unwrap();
        assert!(!t.rounds[0].completed);
    }

    #[test]
    fn no_show_players_default_to_absent_in_the_next_draft() {
        // Four players over one round. On board 0, player2 is a no-show; on board
        // 1, both players are no-shows. Preparing round 2 should pre-mark all
        // three as absent (as if they'd been marked absent this round).
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B", "C", "D"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        start_next_round(&mut t);

        let board0 = &t.rounds[0].boards[0];
        let present = board0.player1; // showed up on board 0
        let no_show0 = board0.player2;
        let board1 = &t.rounds[0].boards[1];
        let (both1, both2) = (board1.player1, board1.player2);
        t.set_board_no_show(1, 0, Some(Forfeit::Player2(AbsenceKind::NoShow)))
            .unwrap();
        t.set_board_no_show(
            1,
            1,
            Some(Forfeit::Both(AbsenceKind::NoShow, AbsenceKind::NoShow)),
        )
        .unwrap();
        assert!(t.rounds[0].completed);

        let draft = t.prepare_round().unwrap();
        assert!(draft.absent.contains(&no_show0));
        assert!(draft.absent.contains(&both1));
        assert!(draft.absent.contains(&both2));
        // The player who showed up is not pre-marked.
        assert!(!draft.absent.contains(&present));
    }

    #[test]
    fn toggle_board_winner_reports_bad_indices() {
        use crate::round::Winner;
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        assert_eq!(
            t.toggle_board_winner(9, 0, Winner::Player1),
            Err(TournamentError::RoundNotFound(9))
        );
        assert_eq!(
            t.toggle_board_winner(1, 5, Winner::Player1),
            Err(TournamentError::BoardNotFound { round: 1, board: 5 })
        );
    }

    #[test]
    fn confirm_needs_enough_present_players() {
        let mut t = Tournament::new("Cup").unwrap();
        t.add_player(named("Solo")).unwrap();
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        assert_eq!(
            t.confirm_round(),
            Err(TournamentError::NotEnoughPresentPlayers { needed: 2, have: 1 })
        );
    }

    #[test]
    fn draft_defaults_absent_to_previous_round() {
        let mut t = Tournament::new("Cup").unwrap();
        t.add_player(named("A")).unwrap();
        t.add_player(named("B")).unwrap();
        t.add_player(named("C")).unwrap();
        t.finalize_registration().unwrap();
        let tid = |t: &Tournament, last: &str| {
            t.players
                .iter()
                .find(|p| p.last_name == last)
                .unwrap()
                .tournament_id
                .unwrap()
        };
        let (a, b, c) = (tid(&t, "A"), tid(&t, "B"), tid(&t, "C"));

        // Round 1: C absent, A vs B.
        t.prepare_round().unwrap();
        t.update_draft(vec![c], vec![], Vec::new()).unwrap();
        t.confirm_round().unwrap();
        assert_eq!(t.rounds[0].absentees().collect::<Vec<_>>(), vec![c]);
        assert_eq!(t.rounds[0].boards.len(), 1); // A vs B, no bye
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();

        // Round 2 draft defaults absent to the previous round's absentees.
        let draft = t.prepare_round().unwrap();
        assert_eq!(draft.absent, vec![c]);
        let _ = (a, b);
    }

    #[test]
    fn confirm_honors_forced_pairing_and_bye() {
        let mut t = Tournament::new("Cup").unwrap();
        for n in ["A", "B", "C", "D", "E"] {
            t.add_player(named(n)).unwrap();
        }
        t.finalize_registration().unwrap();
        let ids: Vec<TournamentId> = ["A", "B", "C", "D", "E"]
            .iter()
            .map(|n| {
                t.players
                    .iter()
                    .find(|p| p.last_name == *n)
                    .unwrap()
                    .tournament_id
                    .unwrap()
            })
            .collect();
        t.prepare_round().unwrap();
        // Force A vs C, and E as the bye (5 present → odd, ok).
        let forced = vec![Board::pending(ids[0], ids[2], 0, PairingSource::Swiss)];
        t.update_draft(vec![], forced, vec![ids[4]]).unwrap();
        let round = t.confirm_round().unwrap();
        assert_eq!(round.byes().collect::<Vec<_>>(), vec![ids[4]]);
        // A vs C is present as a board; B and D auto-paired.
        assert!(round
            .boards
            .iter()
            .any(|b| b.player1 == ids[0] && b.player2 == ids[2]));
        assert_eq!(round.boards.len(), 2);
    }

    /// Every other way of naming a player twice in a draft was already refused
    /// at confirmation — two forced pairings, a pairing and a bye, a forced
    /// player who is also absent. Repeating them inside the *absent* list was
    /// the one that got through, and it wrote one sit-out row per entry: silent
    /// while an absence is worth nothing (the default), and free points the
    /// moment `half_point_absences` is on. Found by the fuzzer.
    #[test]
    fn a_player_named_absent_twice_is_refused_rather_than_paid_twice() {
        let mut t = Tournament::new("Absent twice").unwrap();
        for n in ["A", "B", "C", "D", "E", "F"] {
            t.add_player(named(n)).unwrap();
        }
        t.settings.half_point_absences = true;
        t.finalize_registration().unwrap();
        let all: Vec<TournamentId> = t.players.iter().map(|p| p.tournament_id.unwrap()).collect();

        t.prepare_round().unwrap();
        t.update_draft(vec![all[5], all[5], all[5]], vec![], vec![])
            .unwrap();
        assert_eq!(
            t.confirm_round().err(),
            Some(TournamentError::PlayerTwiceInRound {
                round: 1,
                player: all[5],
            })
        );

        // Named once, the same draft confirms and pays the absence once.
        t.update_draft(vec![all[5]], vec![], vec![]).unwrap();
        let round = t.confirm_round().unwrap();
        assert_eq!(
            round.sitouts.iter().filter(|s| s.player == all[5]).count(),
            1
        );
    }

    #[test]
    fn several_byes_can_be_forced_in_one_round() {
        // Two of five players are forced onto byes, leaving three to pair — an odd
        // count, so the engine byes one more of its own accord. All three byes
        // score their point.
        let mut t = Tournament::new("Byes").unwrap();
        for n in ["A", "B", "C", "D", "E"] {
            t.add_player(named(n)).unwrap();
        }
        t.finalize_registration().unwrap();
        let all: Vec<TournamentId> = t.players.iter().map(|p| p.tournament_id.unwrap()).collect();

        t.prepare_round().unwrap();
        t.update_draft(vec![], vec![], vec![all[0], all[1]])
            .unwrap();
        let round = t.confirm_round().unwrap();

        assert_eq!(
            round.forced_byes().collect::<Vec<_>>(),
            vec![all[0], all[1]]
        );
        // Three left over is odd, so the engine adds its own bye and pairs the rest.
        assert!(round.swiss_bye().is_some(), "the odd leftover is byed");
        assert_eq!(round.byes().count(), 3);
        assert_eq!(round.boards.len(), 1);
        // Every bye scores its point.
        assert!(round
            .sitouts
            .iter()
            .all(|s| s.value == SitoutValue::Full && s.kind.is_bye()));
    }

    #[test]
    fn a_forced_bye_no_longer_needs_an_odd_field() {
        // Four players and one forced bye: the three left over are odd, so the
        // engine byes a second player rather than rejecting the draft.
        let mut t = Tournament::new("Even").unwrap();
        for n in ["A", "B", "C", "D"] {
            t.add_player(named(n)).unwrap();
        }
        t.finalize_registration().unwrap();
        let first = t.players[0].tournament_id.unwrap();
        t.prepare_round().unwrap();
        t.update_draft(vec![], vec![], vec![first]).unwrap();
        let round = t.confirm_round().unwrap();
        assert_eq!(round.byes().count(), 2);
        assert_eq!(round.boards.len(), 1);
    }

    #[test]
    fn set_sitout_value_rescores_a_past_round_but_not_who_sat_out() {
        // Three players, one absent: the referee decides that absence was excused
        // and worth a full point. The score follows; the reason does not.
        let mut t = Tournament::new("Excused").unwrap();
        for n in ["A", "B", "C"] {
            t.add_player(named(n)).unwrap();
        }
        t.finalize_registration().unwrap();
        let c = t.players[2].tournament_id.unwrap();
        t.prepare_round().unwrap();
        t.update_draft(vec![c], vec![], vec![]).unwrap();
        t.confirm_round().unwrap();
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        assert!(t.rounds[0].completed);

        let points_of = |t: &Tournament, tid: TournamentId| {
            let id = t
                .players
                .iter()
                .find(|p| p.tournament_id == Some(tid))
                .unwrap()
                .id;
            t.standings()
                .into_iter()
                .find(|s| s.player_id == id)
                .unwrap()
                .points
        };
        assert_eq!(points_of(&t, c), 0); // absences score nothing by default

        t.set_sitout_value(1, c, SitoutValue::Full).unwrap();
        assert_eq!(points_of(&t, c), 2); // a full point = 2 half-points

        // Still an absence, so it never made them "had a bye" for pairing.
        assert_eq!(t.rounds[0].sitout(c).unwrap().kind, SitoutKind::Absent);
        assert_eq!(t.rounds[0].absentees().collect::<Vec<_>>(), vec![c]);
    }

    #[test]
    fn set_sitout_value_rejects_a_player_who_played() {
        let mut t = Tournament::new("Played").unwrap();
        for n in ["A", "B"] {
            t.add_player(named(n)).unwrap();
        }
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let a = t.players[0].tournament_id.unwrap();
        assert_eq!(
            t.set_sitout_value(1, a, SitoutValue::Half).unwrap_err(),
            TournamentError::PlayerNotSittingOut {
                round: 1,
                player: a
            }
        );
    }

    #[test]
    fn force_pairing_repairs_the_round_with_the_forced_board() {
        let mut t = Tournament::new("Cup").unwrap();
        let ids: Vec<Uuid> = ["A", "B", "C", "D"]
            .iter()
            .map(|n| t.add_player(named(n)).unwrap().id)
            .collect();
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();

        // Two players the engine put on different boards.
        let r1 = t.rounds.last().unwrap();
        let a = r1.boards[0].player1;
        let b = r1.boards[1].player1;
        let paired = |bd: &Board, x, y| {
            (bd.player1 == x && bd.player2 == y) || (bd.player1 == y && bd.player2 == x)
        };
        assert!(
            !r1.boards.iter().any(|bd| paired(bd, a, b)),
            "a and b start unpaired"
        );

        let round = t.force_pairing(UnitKey::from(a), UnitKey::from(b)).unwrap();
        assert_eq!(
            round.number, 1,
            "the same round is re-paired, not a new one"
        );
        assert!(
            round
                .boards
                .iter()
                .any(|bd| matches!(bd.source, PairingSource::Forced) && paired(bd, a, b)),
            "a and b are now a forced board"
        );
        assert_eq!(t.rounds.len(), 1);
        let _ = ids;
    }

    #[test]
    fn force_pairing_refuses_when_results_exist() {
        use crate::round::Winner;
        let mut t = Tournament::new("Cup").unwrap();
        let ids: Vec<Uuid> = ["A", "B", "C", "D"]
            .iter()
            .map(|n| t.add_player(named(n)).unwrap().id)
            .collect();
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();

        // Cross-pair two players from different boards; the recorded result blocks it.
        let r1 = t.rounds.last().unwrap();
        let a = r1.boards[0].player1;
        let b = r1.boards[1].player1;
        assert_eq!(
            t.force_pairing(UnitKey::from(a), UnitKey::from(b)),
            Err(TournamentError::RoundHasResults)
        );
        let _ = ids;
    }

    #[test]
    fn force_pairing_onto_the_bye_reassigns_the_sit_out() {
        let mut t = Tournament::new("Cup").unwrap();
        for n in ["A", "B", "C"] {
            t.add_player(named(n)).unwrap();
        }
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();

        let r1 = t.rounds.last().unwrap();
        let bye = r1.swiss_bye().expect("odd count byes someone");
        let playing = r1.boards[0].player1;

        let round = t
            .force_pairing(UnitKey::from(playing), UnitKey::from(PHANTOM))
            .unwrap();
        assert_eq!(round.number, 1, "the same round is re-paired");
        assert_eq!(
            round.byes().collect::<Vec<_>>(),
            vec![playing],
            "the forced player now byes"
        );
        assert!(
            round
                .boards
                .iter()
                .any(|bd| bd.player1 == bye || bd.player2 == bye),
            "the old bye-taker now plays"
        );
    }

    #[test]
    fn explain_counterfactual_forbidding_the_bye_is_in_scope() {
        let mut t = Tournament::new("Cup").unwrap();
        ["A", "B", "C"].iter().for_each(|n| {
            t.add_player(named(n)).unwrap();
        });
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();

        let r1 = t.rounds.last().unwrap();
        let bye = r1.swiss_bye().expect("odd count byes someone");

        let cf = t
            .explain_counterfactual(
                1,
                UnitKey::from(bye),
                UnitKey::from(PHANTOM),
                CounterfactualMode::Forbid,
            )
            .unwrap();
        assert!(cf.scoped_out.is_none(), "the bye is in scope for the probe");
    }

    fn rated(last_name: &str, rating: u32) -> NewPlayer {
        NewPlayer {
            last_name: last_name.to_string(),
            rating: Some(rating),
            ..Default::default()
        }
    }

    #[test]
    fn confirm_round_orders_boards_by_rank_not_engine_emission_order() {
        // Registered out of rating order, so the matching engine's internal
        // (registration-order-derived) emission order doesn't already happen to
        // match the rank order — a real test of the sort, not a coincidence.
        let mut t = Tournament::new("Cup").unwrap();
        for rating in [1000, 8000, 3000, 6000, 2000, 7000, 4000, 5000] {
            t.add_player(rated(&format!("P{rating}"), rating)).unwrap();
        }
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        let round = t.confirm_round().unwrap();
        assert_eq!(round.boards.len(), 4);

        // Rank is by the standings entering this round (no completed rounds yet,
        // so purely tournament number, which was assigned by ELO descending).
        let tid_of: HashMap<Uuid, TournamentId> = t
            .players
            .iter()
            .filter_map(|p| p.tournament_id.map(|tid| (p.id, tid)))
            .collect();
        let rank_of: HashMap<TournamentId, usize> = t
            .standings()
            .into_iter()
            .enumerate()
            .map(|(rank, s)| (tid_of[&s.player_id], rank))
            .collect();
        let ranks: Vec<usize> = t
            .rounds
            .last()
            .unwrap()
            .boards
            .iter()
            .map(|b| rank_of[&b.player1].min(rank_of[&b.player2]))
            .collect();
        let mut sorted = ranks.clone();
        sorted.sort_unstable();
        assert_eq!(
            ranks, sorted,
            "boards should already be in rank order: {ranks:?}"
        );
    }

    #[test]
    fn set_board_drawn_toggles_flag_without_touching_result() {
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        start_next_round(&mut t);

        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        let board = t.set_board_drawn(1, 0, true).unwrap();
        assert!(board.outcome().drawn());
        // The winner is untouched, and unaffected by the draw flag.
        assert_eq!(board.outcome().winner(), Some(Winner::Player1));
        assert_eq!(board.effective_winner(false), Some(Winner::Player1));
        assert!(!t.set_board_drawn(1, 0, false).unwrap().outcome().drawn());
    }

    /// Nobody played, so nobody drew: the draw flag is rejected on a forfeited
    /// board rather than silently recorded, where it would feed the ELO estimate
    /// a game that never happened.
    /// A justified absence is a team-mode fact — an individual tournament
    /// excludes an absent player before a board exists — so it is refused both
    /// when recorded and when loaded, rather than leaving a `0-` in the grid
    /// that nothing else in the tournament can explain.
    #[test]
    fn a_justified_absence_is_refused_outside_team_mode() {
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        start_next_round(&mut t);

        assert!(matches!(
            t.set_board_no_show(1, 0, Some(Forfeit::Player1(AbsenceKind::Justified))),
            Err(TournamentError::JustifiedAbsenceOutsideTeamMode)
        ));
        // An ordinary no-show is of course fine.
        t.set_board_no_show(1, 0, Some(Forfeit::Player1(AbsenceKind::NoShow)))
            .unwrap();
        assert!(t.validate_loaded().is_ok());

        // ...and a save that carries one anyway is rejected on load.
        t.rounds[0].boards[0].set_outcome(Outcome::Forfeit {
            absent: Forfeit::Player1(AbsenceKind::Justified),
        });
        assert!(matches!(
            t.validate_loaded(),
            Err(TournamentError::JustifiedAbsenceOutsideTeamMode)
        ));
    }

    #[test]
    fn set_board_drawn_rejects_a_forfeited_board() {
        let mut t = Tournament::new("Cup").unwrap();
        for name in ["A", "B"] {
            t.add_player(named(name)).unwrap();
        }
        t.finalize_registration().unwrap();
        start_next_round(&mut t);

        t.set_board_no_show(1, 0, Some(Forfeit::Player2(AbsenceKind::NoShow)))
            .unwrap();
        assert!(matches!(
            t.set_board_drawn(1, 0, true),
            Err(TournamentError::DrawnOnForfeitedBoard { round: 1, board: 0 })
        ));
        // Clearing the forfeit makes the board an ordinary pending one again.
        t.set_board_no_show(1, 0, None).unwrap();
        assert!(t.set_board_drawn(1, 0, true).unwrap().outcome().drawn());
    }

    /// Nobody played, so nobody conceded odds: the handicap follows the draw
    /// flag. It is refused on a forfeited board — in both directions, since the
    /// picker is disabled there — and declaring a no-show drops a handicap the
    /// board already carried rather than leaving it to be published (or to come
    /// back if the forfeit is cleared).
    #[test]
    fn a_forfeit_refuses_a_handicap_and_drops_the_one_already_set() {
        let mut t = Tournament::new("Cup").unwrap();
        t.add_player(rated("High", 2000)).unwrap();
        t.add_player(rated("Low", 1000)).unwrap();
        t.finalize_registration().unwrap();
        start_next_round(&mut t);

        t.set_board_handicap(1, 0, Some(Handicap::TwoPiece))
            .unwrap();
        assert!(t.rounds[0].boards[0].handicap.is_some());

        // Declaring the no-show removes it, exactly as it removes the draw flag.
        t.set_board_no_show(1, 0, Some(Forfeit::Player2(AbsenceKind::NoShow)))
            .unwrap();
        assert_eq!(t.rounds[0].boards[0].handicap, None);

        // And it cannot be set again while the forfeit stands. Nor cleared: the
        // UI offers neither, so either request is a client out of sync.
        assert!(matches!(
            t.set_board_handicap(1, 0, Some(Handicap::TwoPiece)),
            Err(TournamentError::HandicapOnForfeitedBoard { round: 1, board: 0 })
        ));
        assert!(matches!(
            t.set_board_handicap(1, 0, None),
            Err(TournamentError::HandicapOnForfeitedBoard { round: 1, board: 0 })
        ));

        // Clearing the forfeit reopens the picker, but does not resurrect the
        // handicap — the referee re-enters it.
        t.set_board_no_show(1, 0, None).unwrap();
        assert_eq!(t.rounds[0].boards[0].handicap, None);
        t.set_board_handicap(1, 0, Some(Handicap::TwoPiece))
            .unwrap();
        assert!(t.rounds[0].boards[0].handicap.is_some());
        assert!(t.validate_loaded().is_ok());

        // ...and a save that pairs the two anyway is rejected on load. No file
        // this build can otherwise read reaches that state: the handicap and the
        // forfeit are exclusive through every setter, and the only older saves
        // accepted are of tournaments that have not started, which have no boards.
        t.rounds[0].boards[0].set_outcome(Outcome::Forfeit {
            absent: Forfeit::Player2(AbsenceKind::NoShow),
        });
        assert_eq!(
            t.validate_loaded(),
            Err(TournamentError::HandicapOnForfeitedBoard { round: 1, board: 0 })
        );
    }

    #[test]
    fn handicap_freezes_giver_and_flips_effective_winner_with_wiel_rule_on() {
        let mut t = Tournament::new("Cup").unwrap();
        // High is rated above Low, so High is the giver.
        let high = t.add_player(rated("High", 2000)).unwrap().id;
        let _low = t.add_player(rated("Low", 1000)).unwrap().id;
        t.finalize_registration().unwrap();
        let high_tid = t
            .players
            .iter()
            .find(|p| p.id == high)
            .unwrap()
            .tournament_id
            .unwrap();
        start_next_round(&mut t);

        let p1_is_high = t.rounds[0].boards[0].player1 == high_tid;

        // Give a 4-piece handicap. Whoever actually loses, the giver (High)
        // scores the effective point.
        t.set_board_handicap(1, 0, Some(Handicap::FourPiece))
            .unwrap();
        // The receiver actually wins the game...
        let receiver_wins = if p1_is_high {
            Winner::Player2
        } else {
            Winner::Player1
        };
        t.toggle_board_winner(1, 0, receiver_wins).unwrap();

        let board = &t.rounds[0].boards[0];
        assert_eq!(board.outcome().winner(), Some(receiver_wins)); // actual result recorded
        let giver_side = if p1_is_high {
            Winner::Player1
        } else {
            Winner::Player2
        };
        assert_eq!(board.handicap.unwrap().giver, giver_side);
        // ...but with the Wiel rule on, the giver still counts as the effective winner.
        assert_eq!(board.effective_winner(true), Some(giver_side));
        // With the Wiel rule off (the default), the actual result counts instead.
        assert_eq!(board.effective_winner(false), Some(receiver_wins));

        // The frozen giver survives a later rating swap (Low now outrates High).
        let low_id = t.players.iter().find(|p| p.id != high).unwrap().id;
        t.edit_player(low_id, rated("Low", 9999)).unwrap();
        assert_eq!(t.rounds[0].boards[0].handicap.unwrap().giver, giver_side);
    }

    #[test]
    fn handicap_rejected_when_ratings_equal() {
        let mut t = Tournament::new("Cup").unwrap();
        t.add_player(rated("A", 1500)).unwrap();
        t.add_player(rated("B", 1500)).unwrap();
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        assert_eq!(
            t.set_board_handicap(1, 0, Some(Handicap::Rook)),
            Err(TournamentError::HandicapNeedsRatingDifference)
        );
        // Clearing a handicap is always allowed, even with equal ratings.
        assert!(t.set_board_handicap(1, 0, None).unwrap().handicap.is_none());
    }

    #[test]
    fn handicap_rejected_for_cup_board() {
        let mut t = Tournament::new("Cup").unwrap();
        t.add_player(rated("High", 2000)).unwrap();
        t.add_player(rated("Low", 1000)).unwrap();
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        t.rounds[0].boards[0].source = PairingSource::Cup {
            stage: CupStage::Final,
        };
        assert_eq!(
            t.set_board_handicap(1, 0, Some(Handicap::Rook)),
            Err(TournamentError::HandicapNotAllowedForCup)
        );
        // Clearing a handicap is still allowed.
        assert!(t.set_board_handicap(1, 0, None).unwrap().handicap.is_none());
        // No suggestion either, despite the large rating gap.
        let board = t.rounds[0].boards[0].clone();
        assert_eq!(t.suggested_handicap_for_board(&board), None);
    }

    #[test]
    fn handicap_rejected_when_both_unrated() {
        let mut t = Tournament::new("Cup").unwrap();
        t.add_player(named("A")).unwrap();
        t.add_player(named("B")).unwrap();
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        assert_eq!(
            t.set_board_handicap(1, 0, Some(Handicap::Rook)),
            Err(TournamentError::HandicapNeedsRatingDifference)
        );
    }

    #[test]
    fn unrated_player_receives_handicap_from_rated() {
        let mut t = Tournament::new("Cup").unwrap();
        let rated_id = t.add_player(rated("Rated", 1800)).unwrap().id;
        t.add_player(named("Unrated")).unwrap();
        t.finalize_registration().unwrap();
        let rated_tid = t
            .players
            .iter()
            .find(|p| p.id == rated_id)
            .unwrap()
            .tournament_id
            .unwrap();
        start_next_round(&mut t);
        t.set_board_handicap(1, 0, Some(Handicap::TwoPiece))
            .unwrap();
        let board = &t.rounds[0].boards[0];
        let giver_side = if board.player1 == rated_tid {
            Winner::Player1
        } else {
            Winner::Player2
        };
        assert_eq!(board.handicap.unwrap().giver, giver_side); // rated player gives
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

    /// Confirming a round must leave its own explanation faithful: nothing has
    /// been edited since it was frozen a moment earlier, so the "why these
    /// pairings?" panel must not open under a warning that the data has moved.
    #[test]
    fn confirming_a_round_leaves_its_explanation_faithful() {
        let mut t = Tournament::new("Long").unwrap();
        for n in ["Alpha", "Bravo", "Carol", "Dave"] {
            t.add_player(named(n)).unwrap();
        }
        t.settings.long_boards_enabled = true;
        t.finalize_registration().unwrap();

        start_next_round(&mut t);
        assert_eq!(t.explanations_faithful_through, 1);

        // Flagging a board long is an edit *inside* round 1, which cannot unsettle
        // round 1's own explanation — it was frozen before the flag existed.
        t.set_board_long(1, 0, true).unwrap();
        assert_eq!(t.explanations_faithful_through, 1);

        t.toggle_board_winner(1, 1, Winner::Player1).unwrap();
        start_next_round(&mut t);
        assert_eq!(
            t.explanations_faithful_through, 2,
            "round 2 was just paired from the present, so it is faithful to it"
        );
    }

    /// A player finishing a long game is not available to be marked absent: they
    /// are playing. Allowing it gave them a sit-out *and* the carried board in the
    /// same round — the sit-out then hid the game from the cross-table export and
    /// added its value on top of the game's own score.
    #[test]
    fn a_player_mid_long_game_cannot_be_marked_absent() {
        let mut t = Tournament::new("Long").unwrap();
        for n in ["Alpha", "Bravo", "Carol", "Dave"] {
            t.add_player(named(n)).unwrap();
        }
        t.settings.long_boards_enabled = true;
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        t.set_board_long(1, 0, true).unwrap();
        let busy = t.rounds[0].boards[0].player1;
        t.toggle_board_winner(1, 1, Winner::Player1).unwrap();

        t.prepare_round().unwrap();
        t.update_draft(vec![busy], Vec::new(), Vec::new()).unwrap();
        let err = t.confirm_round().unwrap_err();
        assert!(
            matches!(err, TournamentError::InvalidDraft(ref m) if m.contains("long game")),
            "expected a long-game refusal, got {err:?}"
        );
    }

    /// Flagging a cup round long moves *every* cup board, so the "not after a
    /// result" guard has to be asked of every one of them. Asking only the
    /// clicked board let a decided game be flipped long by a click on its
    /// neighbour — retroactively doubling a result already recorded, which is
    /// exactly what the guard exists to prevent.
    #[test]
    fn a_cup_round_cannot_go_long_once_any_of_its_boards_is_decided() {
        let mut t = Tournament::new("Champ").unwrap();
        enable_cup(&mut t);
        let s: Vec<Uuid> = (0..8)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2000 - i * 100, true))
            .collect();
        t.settings.long_boards_enabled = true;
        t.finalize_registration_with(Some(8)).unwrap();
        start_next_round(&mut t);

        // Record one quarterfinal, then try to make the round long from another.
        decide(&mut t, 1, s[0], s[7]);
        let undecided = t.rounds[0]
            .boards
            .iter()
            .position(|b| !b.is_decided())
            .expect("three quarterfinals are still open");
        assert_eq!(
            t.set_board_long(1, undecided, true),
            Err(TournamentError::LongFlagAfterCoupledResult),
            "one decided board in the coupled unit blocks the whole flip, and \
             says so — the referee clicked an empty board"
        );
        assert!(
            t.rounds[0].boards.iter().all(|b| !b.is_long()),
            "and nothing moved"
        );
    }

    /// A qualifier cup's first round is one session of one cup: the play-off
    /// boards *and* the pre-qualified players' games. Flagging it long has to
    /// take in both, or the pre-qualified finish a round early and are free to be
    /// paired while the qualifiers are still playing — desynchronised from the
    /// bracket they are about to meet.
    #[test]
    fn a_qualifier_cups_first_round_goes_long_with_its_prequalified_games() {
        let mut t = Tournament::new("Champ").unwrap();
        t.update_settings(TournamentSettings {
            cup_enabled: true,
            cup_format: CupFormat::Qualifier,
            long_boards_enabled: true,
            ..Default::default()
        })
        .unwrap();
        // A size-8 qualifier cup takes 12 eligible: four pre-qualified and eight
        // in the qualification round. Four outsiders give the pre-qualified
        // somebody to face in the open.
        for i in 0..12 {
            add_rated(&mut t, &format!("E{i}"), 2400 - i * 50, true);
        }
        for i in 0..4 {
            add_rated(&mut t, &format!("N{i}"), 1500 - i * 50, false);
        }
        t.finalize_registration_with(Some(8)).unwrap();
        start_next_round(&mut t);

        let prequalified: Vec<TournamentId> = t.prequalified_in_round(1).to_vec();
        assert_eq!(prequalified.len(), 4, "size/2 pre-qualified in round 1");

        // Flag from a qualification board; the pre-qualified games move with it.
        let cup_board = t.rounds[0]
            .boards
            .iter()
            .position(|b| matches!(b.source, PairingSource::Cup { .. }))
            .expect("the qualification round has cup boards");
        t.set_board_long(1, cup_board, true).unwrap();

        for b in &t.rounds[0].boards {
            let theirs = prequalified.contains(&b.player1) || prequalified.contains(&b.player2);
            let cup = matches!(b.source, PairingSource::Cup { .. });
            assert_eq!(
                b.is_long(),
                cup || theirs,
                "one session, one length: cup={cup} prequalified={theirs}"
            );
        }

        // And the coupling is symmetric — unflagging from a pre-qualified game
        // takes the qualification boards back with it.
        let pre_board = t.rounds[0]
            .boards
            .iter()
            .position(|b| {
                !matches!(b.source, PairingSource::Cup { .. })
                    && (prequalified.contains(&b.player1) || prequalified.contains(&b.player2))
            })
            .expect("a pre-qualified player plays in the open");
        t.set_board_long(1, pre_board, false).unwrap();
        assert!(t.rounds[0].boards.iter().all(|b| !b.is_long()));
    }

    /// A no-show *on* the long board resolves the game but does not release its
    /// players — it still took both of its rounds. So the next draft must not
    /// carry them into its absent list, which is otherwise defaulted from the
    /// previous round's no-shows: proposing them there would offer the referee a
    /// draft that `confirm_round` then refuses.
    #[test]
    fn a_no_show_on_a_long_board_does_not_default_its_players_absent() {
        let mut t = Tournament::new("Long").unwrap();
        for n in ["Alpha", "Bravo", "Carol", "Dave"] {
            t.add_player(named(n)).unwrap();
        }
        t.settings.long_boards_enabled = true;
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        t.set_board_long(1, 0, true).unwrap();
        let (a, b) = (t.rounds[0].boards[0].player1, t.rounds[0].boards[0].player2);
        t.set_board_no_show(1, 0, Some(Forfeit::Player2(AbsenceKind::NoShow)))
            .unwrap();
        t.toggle_board_winner(1, 1, Winner::Player1).unwrap();

        let draft = t.prepare_round().unwrap();
        assert!(
            !draft.absent.contains(&a) && !draft.absent.contains(&b),
            "the long game's players are playing it, not absent: {:?}",
            draft.absent
        );
        t.confirm_round().expect("and the round confirms");
    }

    /// The blockers are what the buttons read, so they must answer exactly what
    /// the guards do. Pinned here because the alternative — a client deriving the
    /// same rule from the same data — is what this replaced, and what drifted.
    #[test]
    fn the_blockers_agree_with_the_guards_they_report_on() {
        let mut t = Tournament::new("Long").unwrap();
        for n in ["Alpha", "Bravo", "Carol", "Dave"] {
            t.add_player(named(n)).unwrap();
        }
        t.settings.long_boards_enabled = true;

        // Before finalization, and with a draft open, `prepare_round` refuses for
        // reasons the blocker names identically.
        assert_eq!(
            t.next_round_blocker(),
            Some(TournamentError::RegistrationNotFinalized)
        );
        t.finalize_registration().unwrap();
        assert_eq!(t.next_round_blocker(), None);
        t.prepare_round().unwrap();
        assert_eq!(
            t.next_round_blocker(),
            Some(TournamentError::DraftAlreadyExists)
        );
        t.confirm_round().unwrap();

        // A long game leaves round 2 open, and that is what stops round 3 — the
        // blocker says so rather than the client working it out.
        t.set_board_long(1, 0, true).unwrap();
        t.toggle_board_winner(1, 1, Winner::Player1).unwrap();
        assert_eq!(t.next_round_blocker(), None, "round 1 is complete");
        start_next_round(&mut t);
        assert_eq!(
            t.next_round_blocker(),
            Some(TournamentError::PreviousRoundNotComplete),
            "the carried game holds round 2 open"
        );
        assert_eq!(
            t.prepare_round(),
            Err(TournamentError::PreviousRoundNotComplete)
        );

        // And the export refuses for as long as the game is unresolved, with the
        // blocker and `to_grid` naming the same round.
        assert_eq!(
            t.grid_export_blocker(),
            Some(TournamentError::UnresolvedLongGame { round: 2 })
        );
        assert_eq!(
            crate::american_grid::to_grid(&t, &t.standings()),
            Err(TournamentError::UnresolvedLongGame { round: 2 })
        );

        // Entering the result clears both, together. The carried board is not
        // index 0 — it is appended after the round's own pairings — so find it.
        assert!(
            t.rounds[1]
                .boards
                .iter()
                .any(|b| b.source == PairingSource::Carried),
            "round 2 holds the carried game"
        );
        for i in 0..t.rounds[1].boards.len() {
            if !t.rounds[1].boards[i].is_decided() {
                t.toggle_board_winner(2, i, Winner::Player1).unwrap();
            }
        }
        assert_eq!(t.grid_export_blocker(), None);
        assert_eq!(t.next_round_blocker(), None);
        assert!(crate::american_grid::to_grid(&t, &t.standings()).is_ok());
        t.prepare_round().expect("round 3 prepares");
    }

    /// A long game counts as **two** games against its opponent for the
    /// opponent-based tie-breaks (SOS, SODOS, SOSOS, Buchholz) — it is worth two
    /// rounds, so it weighs two. Not four.
    ///
    /// It now holds one record per round it occupies, and both are in completed
    /// rounds, so a per-record multiplier of two would count the game twice over.
    /// Each record contributes one game instead, which is the same total and says
    /// the truer thing: one game faced per round played.
    #[test]
    fn a_long_game_is_two_opponents_faced_not_four() {
        let mut t = Tournament::new("Long").unwrap();
        for n in ["Alpha", "Bravo", "Carol", "Dave"] {
            t.add_player(named(n)).unwrap();
        }
        t.settings.long_boards_enabled = true;
        t.finalize_registration().unwrap();
        start_next_round(&mut t);

        // Board 0 goes long; the other board finishes, so round 1 completes.
        t.set_board_long(1, 0, true).unwrap();
        let (a, b) = (t.rounds[0].boards[0].player1, t.rounds[0].boards[0].player2);
        t.toggle_board_winner(1, 1, Winner::Player1).unwrap();
        assert!(t.rounds[0].completed);

        // Round 2 carries the long game; finish everything.
        start_next_round(&mut t);
        let n = t.rounds[1].boards.len();
        for i in 0..n {
            t.toggle_board_winner(2, i, Winner::Player1).unwrap();
        }
        assert!(t.rounds[1].completed);

        let uuid_of = |tid: TournamentId| {
            t.players
                .iter()
                .find(|p| p.tournament_id == Some(tid))
                .expect("a current player")
                .id
        };
        let (a_id, b_id) = (uuid_of(a), uuid_of(b));
        let standings = t.standings();
        let faced = |who: Uuid, whom: Uuid| {
            standings
                .iter()
                .find(|s| s.player_id == who)
                .expect("in the standings")
                .opponents
                .iter()
                .filter(|&&o| o == whom)
                .count()
        };
        assert_eq!(faced(a_id, b_id), 2, "two rounds of one game, so two games");
        assert_eq!(faced(b_id, a_id), 2);
    }

    /// A handicap is part of the *game*, not of the round it was drawn in, so it
    /// travels with the game onto the live record and comes back with it.
    ///
    /// Leaving a copy behind would duplicate it, and a duplicate can disagree; it
    /// would also put the grid's `(-r)` suffix on the `0-` placeholder cell, which
    /// renders no game at all.
    #[test]
    fn a_handicap_travels_with_the_game_it_was_conceded_in() {
        let mut t = Tournament::new("Long").unwrap();
        for (n, elo) in [("Alpha", 2000), ("Bravo", 1600)] {
            t.add_player(NewPlayer {
                last_name: n.to_string(),
                rating: Some(elo),
                ..Default::default()
            })
            .unwrap();
        }
        t.settings.long_boards_enabled = true;
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        t.set_board_long(1, 0, true).unwrap();
        t.set_board_handicap(1, 0, Some(Handicap::Rook)).unwrap();
        let conceded = t.rounds[0].boards[0].handicap;
        assert!(conceded.is_some());

        start_next_round(&mut t);
        assert_eq!(
            t.rounds[1].boards[0].handicap, conceded,
            "the handicap moved onto the live record"
        );
        assert_eq!(
            t.rounds[0].boards[0].handicap, None,
            "and did not stay behind to be a second copy of itself"
        );

        // Back again with the game, so a cancel loses nothing.
        t.cancel_last_round().unwrap();
        assert_eq!(t.rounds[0].boards[0].handicap, conceded);
        t.validate_loaded().expect("still a whole record");
    }

    /// Everything about a carried long game lives on its live record, so every
    /// way of writing to the inert one is refused, naming where the game is.
    ///
    /// Without these the result path would *panic* (a carried record has nowhere
    /// to put an outcome) and the handicap path would silently write a handicap
    /// that scores nothing — both reachable by addressing the board the round it
    /// started in, which the round tab still shows.
    #[test]
    fn writing_to_a_carried_long_games_inert_record_is_refused() {
        let mut t = Tournament::new("Long").unwrap();
        // Rated, and unequally: a handicap needs a rating difference to size it.
        for (n, elo) in [("Alpha", 2000), ("Bravo", 1600)] {
            t.add_player(NewPlayer {
                last_name: n.to_string(),
                rating: Some(elo),
                ..Default::default()
            })
            .unwrap();
        }
        t.settings.long_boards_enabled = true;
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        t.set_board_long(1, 0, true).unwrap();
        start_next_round(&mut t);

        let carried = Err(TournamentError::CarriedLongGame { round: 2 });
        assert_eq!(t.toggle_board_winner(1, 0, Winner::Player1), carried);
        assert_eq!(t.set_board_drawn(1, 0, true), carried);
        assert_eq!(
            t.set_board_no_show(1, 0, Some(Forfeit::Player2(AbsenceKind::NoShow))),
            carried
        );
        assert_eq!(
            t.set_board_handicap(1, 0, Some(Handicap::Rook)),
            Err(TournamentError::CarriedLongGame { round: 2 })
        );
        // Longness is refused there too, but for the older and truer reason: it
        // was fixed when the round advanced, and round 2 cannot change it either.
        assert_eq!(
            t.set_board_long(1, 0, false),
            Err(TournamentError::NotCurrentRound)
        );
        // And the live record's length is fixed by the round that started it —
        // demoting it here would orphan the record left behind.
        assert_eq!(
            t.set_board_long(2, 0, false),
            Err(TournamentError::LongGameStartedEarlier { round: 1 })
        );

        // The live record takes all of it.
        t.toggle_board_winner(2, 0, Winner::Player1).unwrap();
        t.set_board_handicap(2, 0, Some(Handicap::Rook)).unwrap();
        t.validate_loaded().expect("still a whole record");
    }

    /// Cancelling the round a long game is finished in moves its outcome back
    /// onto the record it came from — the exact inverse of the carry, so the fact
    /// that the game was played survives the round-trip. Dropping the result (or
    /// refusing the cancel) would both lose it.
    #[test]
    fn cancelling_the_round_a_long_game_ends_in_gives_its_result_back() {
        let mut t = Tournament::new("Long").unwrap();
        for n in ["Alpha", "Bravo"] {
            t.add_player(named(n)).unwrap();
        }
        t.settings.long_boards_enabled = true;
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        t.set_board_long(1, 0, true).unwrap();
        start_next_round(&mut t);
        // The game is played out in round 2, on its live record.
        t.toggle_board_winner(2, 0, Winner::Player1).unwrap();

        t.cancel_last_round().unwrap();
        assert_eq!(t.rounds.len(), 1);
        assert_eq!(
            t.rounds[0].boards[0].record,
            GameRecord::LongStart(Outcome::won(Winner::Player1)),
            "the outcome came back to the record it started on"
        );
        t.validate_loaded().expect("and the record is whole again");

        // Re-confirming carries it forward again, result and all.
        start_next_round(&mut t);
        assert_eq!(t.rounds[0].boards[0].record, GameRecord::LongCarried);
        assert_eq!(
            t.rounds[1].boards[0].record,
            GameRecord::LongEnd(Outcome::won(Winner::Player1))
        );
    }

    /// A carried long game is two records written as a pair. A file holding only
    /// one of them has a game whose result is unreachable or unattributable, and
    /// nothing downstream would say so — so loading it is refused.
    #[test]
    fn validate_loaded_rejects_half_of_a_carried_long_game() {
        // A real carried game, produced the only way there is to produce one.
        let mut t = Tournament::new("Long").unwrap();
        for n in ["Alpha", "Bravo"] {
            t.add_player(named(n)).unwrap();
        }
        t.settings.long_boards_enabled = true;
        t.finalize_registration().unwrap();
        start_next_round(&mut t);
        t.set_board_long(1, 0, true).unwrap();
        start_next_round(&mut t);
        t.validate_loaded()
            .expect("a carried game is a valid record");

        // The live record without the one it came from.
        let mut orphan = t.clone();
        orphan.rounds[0].boards.clear();
        assert_eq!(
            orphan.validate_loaded(),
            Err(TournamentError::OrphanedLongGame { round: 2 })
        );

        // The starting record without the live one.
        let mut orphan = t.clone();
        orphan.rounds[1].boards.clear();
        assert_eq!(
            orphan.validate_loaded(),
            Err(TournamentError::OrphanedLongGame { round: 1 })
        );

        // A start that was never carried, although a later round exists.
        let mut orphan = t.clone();
        orphan.rounds[0].boards[0].record = GameRecord::LongStart(Outcome::PENDING);
        orphan.rounds[1].boards.clear();
        assert_eq!(
            orphan.validate_loaded(),
            Err(TournamentError::OrphanedLongGame { round: 1 })
        );

        // A long game in the last round has nothing after it to be carried into,
        // which is the one time a bare `LongStart` is correct.
        let mut last = t.clone();
        last.rounds.pop();
        last.rounds[0].boards[0].record = GameRecord::LongStart(Outcome::PENDING);
        last.explanations_faithful_through = 1;
        last.validate_loaded()
            .expect("an uncarried long game in the last round is fine");
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

    /// `Tournament::new` refuses a blank name, but an imported file never runs
    /// it — nothing but `validate_loaded` stands between the JSON and the
    /// registry, so the same rule has to hold here.
    #[test]
    fn validate_loaded_rejects_a_blank_name() {
        for blank in ["", "   ", "\t\n"] {
            let mut t = Tournament::new("Paris Open").unwrap();
            t.name = blank.to_string();
            assert_eq!(
                t.validate_loaded(),
                Err(TournamentError::EmptyTournamentName),
                "a file named {blank:?} must not import"
            );
        }
    }

    /// A two-player tournament with one confirmed round — the smallest thing
    /// whose rounds a corrupted file could misdescribe.
    fn played_one_round() -> Tournament {
        let mut t = Tournament::new("Paris Open").unwrap();
        t.add_player(named("Alice")).unwrap();
        t.add_player(named("Bob")).unwrap();
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        t
    }

    /// Everything downstream of a load indexes players by tournament number and
    /// assumes the numbers are there, distinct, and the only ones any round
    /// names. Each of these used to reach the read path: the first three panic
    /// (`compute_scores` unwraps the number, and indexes a vector sized by the
    /// largest one), and the rest quietly report the wrong tournament.
    #[test]
    fn validate_loaded_rejects_a_file_whose_rounds_dont_name_its_field() {
        // A finalized player with no number: `standings()` unwraps it.
        let mut unnumbered = played_one_round();
        let id = unnumbered.players[0].id;
        unnumbered.players[0].tournament_id = None;
        assert_eq!(
            unnumbered.validate_loaded(),
            Err(TournamentError::UnnumberedPlayer { player: id })
        );

        // Two players on one number share a single score slot.
        let mut shared = played_one_round();
        shared.players[1].tournament_id = shared.players[0].tournament_id;
        assert_eq!(
            shared.validate_loaded(),
            Err(TournamentError::DuplicateTournamentNumber {
                number: TournamentId(1)
            })
        );

        // Two rows for one player: an edit to either would silently pick one.
        let mut twice = played_one_round();
        twice.players[1].id = twice.players[0].id;
        assert_eq!(
            twice.validate_loaded(),
            Err(TournamentError::DuplicatePlayerId {
                player: twice.players[0].id
            })
        );

        // A board naming somebody who isn't here indexes past the score vector.
        let mut stranger = played_one_round();
        stranger.rounds[0].boards[0].player2 = TournamentId(99);
        assert_eq!(
            stranger.validate_loaded(),
            Err(TournamentError::UnknownRoundPlayer {
                round: 1,
                player: TournamentId(99)
            })
        );

        // So does a sit-out.
        let mut sitout = played_one_round();
        sitout.rounds[0].sitouts.push(crate::round::Sitout {
            player: TournamentId(99),
            kind: crate::round::SitoutKind::Bye,
            value: crate::round::SitoutValue::Full,
        });
        assert_eq!(
            sitout.validate_loaded(),
            Err(TournamentError::UnknownRoundPlayer {
                round: 1,
                player: TournamentId(99)
            })
        );

        // A player paired with themselves would be counted twice.
        let mut alone = played_one_round();
        alone.rounds[0].boards[0].player2 = alone.rounds[0].boards[0].player1;
        assert_eq!(
            alone.validate_loaded(),
            Err(TournamentError::BoardAgainstSelf {
                round: 1,
                player: TournamentId(1)
            })
        );

        // Two sit-out rows for one player: each one scores, so the round pays
        // them twice for the one round they missed.
        let mut twice_absent = played_one_round();
        for _ in 0..2 {
            twice_absent.rounds[0].sitouts.push(crate::round::Sitout {
                player: TournamentId(1),
                kind: crate::round::SitoutKind::Absent,
                value: crate::round::SitoutValue::Half,
            });
        }
        assert_eq!(
            twice_absent.validate_loaded(),
            Err(TournamentError::PlayerTwiceInRound {
                round: 1,
                player: TournamentId(1)
            })
        );

        // A board *and* a sit-out is the same problem wearing a different hat:
        // the player both played and did not play.
        let mut played_and_sat = played_one_round();
        played_and_sat.rounds[0].sitouts.push(crate::round::Sitout {
            player: TournamentId(2),
            kind: crate::round::SitoutKind::Bye,
            value: crate::round::SitoutValue::Full,
        });
        assert_eq!(
            played_and_sat.validate_loaded(),
            Err(TournamentError::PlayerTwiceInRound {
                round: 1,
                player: TournamentId(2)
            })
        );

        // Rounds are addressed by number but read as a positional prefix, so the
        // two disagreeing puts an edit on a different round than the referee saw.
        let mut misnumbered = played_one_round();
        misnumbered.rounds[0].number = 7;
        misnumbered.rounds[0].explanation.round = 7;
        assert_eq!(
            misnumbered.validate_loaded(),
            Err(TournamentError::MisnumberedRound {
                expected: 1,
                found: 7
            })
        );

        // And the tournament these were all derived from is fine.
        played_one_round().validate_loaded().unwrap();
    }

    /// A save is external input, so a key this build doesn't know is drift, not
    /// something to skip past: `outcome` renamed on a `Board` would erase every
    /// recorded result, `rounds` renamed on the tournament would erase the event.
    #[test]
    fn a_drifted_key_is_refused_rather_than_dropped() {
        let mut t = played_one_round();
        // A result, so `outcome` is actually in the JSON — it is skipped while
        // the board is pending, and it is the field whose loss would be worst.
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        let json = serde_json::to_string(&t).unwrap();

        for (from, to) in [("\"rounds\"", "\"round\""), ("\"outcome\"", "\"outome\"")] {
            assert!(
                json.contains(from),
                "the fixture should contain {from} to misspell"
            );
            let drifted = json.replacen(from, to, 1);
            assert!(
                serde_json::from_str::<Tournament>(&drifted).is_err(),
                "{to} must be refused, not silently dropped"
            );
        }
    }

    // --- Player categories ------------------------------------------------

    fn category(name: &str) -> crate::settings::PlayerCategory {
        crate::settings::PlayerCategory {
            id: Uuid::new_v4(),
            name: name.to_string(),
        }
    }

    fn with_categories(cats: Vec<crate::settings::PlayerCategory>) -> TournamentSettings {
        TournamentSettings {
            categories: cats,
            ..Default::default()
        }
    }

    fn cats_of(t: &Tournament, id: Uuid) -> Vec<Uuid> {
        t.players
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .categories
            .clone()
    }

    #[test]
    fn set_player_category_adds_and_removes_membership() {
        let mut t = Tournament::new("Paris Open").unwrap();
        let women = category("Women");
        let u18 = category("U18");
        t.update_settings(with_categories(vec![women.clone(), u18.clone()]))
            .unwrap();
        let p = t.add_player(named("Tanaka")).unwrap().id;

        // Default: no memberships.
        assert!(cats_of(&t, p).is_empty());

        // Join both categories; the list stays sorted and de-duplicated.
        t.set_player_category(p, women.id, true).unwrap();
        t.set_player_category(p, u18.id, true).unwrap();
        t.set_player_category(p, women.id, true).unwrap(); // idempotent
        let mut expected = vec![women.id, u18.id];
        expected.sort();
        assert_eq!(cats_of(&t, p), expected);

        // Leave one; the other remains.
        t.set_player_category(p, women.id, false).unwrap();
        assert_eq!(cats_of(&t, p), vec![u18.id]);
    }

    #[test]
    fn set_player_category_rejects_an_unknown_category() {
        let mut t = Tournament::new("Paris Open").unwrap();
        let p = t.add_player(named("Tanaka")).unwrap().id;
        let stray = Uuid::new_v4();
        assert_eq!(
            t.set_player_category(p, stray, true),
            Err(TournamentError::CategoryNotFound(stray))
        );
    }

    #[test]
    fn deleting_a_category_prunes_it_from_every_player() {
        let mut t = Tournament::new("Paris Open").unwrap();
        let women = category("Women");
        let u18 = category("U18");
        t.update_settings(with_categories(vec![women.clone(), u18.clone()]))
            .unwrap();
        let p = t.add_player(named("Tanaka")).unwrap().id;
        t.set_player_category(p, women.id, true).unwrap();
        t.set_player_category(p, u18.id, true).unwrap();

        // Drop "Women" from the settings: the membership is pruned, "U18" stays.
        t.update_settings(with_categories(vec![u18.clone()]))
            .unwrap();
        assert_eq!(cats_of(&t, p), vec![u18.id]);
    }

    #[test]
    fn normalizing_settings_trims_blank_and_duplicate_categories() {
        let id = Uuid::new_v4();
        let s = TournamentSettings {
            categories: vec![
                crate::settings::PlayerCategory {
                    id,
                    name: "  Women  ".to_string(),
                },
                crate::settings::PlayerCategory {
                    id, // duplicate id — first kept
                    name: "dup".to_string(),
                },
                crate::settings::PlayerCategory {
                    id: Uuid::new_v4(),
                    name: "   ".to_string(), // blank — dropped
                },
            ],
            ..Default::default()
        }
        .normalized();
        assert_eq!(s.categories.len(), 1);
        assert_eq!(s.categories[0].name, "Women"); // trimmed
        assert_eq!(s.categories[0].id, id);
    }

    // --- Hybrid cup -------------------------------------------------------

    use crate::cup::CupFormat;
    use crate::round::CupStage;

    fn enable_cup(t: &mut Tournament) {
        t.update_settings(TournamentSettings {
            cup_enabled: true,
            ..Default::default()
        })
        .unwrap();
    }

    fn add_rated(t: &mut Tournament, name: &str, rating: u32, eligible: bool) -> Uuid {
        let id = t.add_player(rated(name, rating)).unwrap().id;
        if eligible {
            t.set_player_eligible(id, true).unwrap();
        }
        id
    }

    /// The tournament number assigned to a registered player (only valid after
    /// `finalize_registration`).
    fn tid(t: &Tournament, id: Uuid) -> TournamentId {
        t.players
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .tournament_id
            .unwrap()
    }

    /// Find the board (in the round with the given number) pairing `a` and `b`.
    fn find_board(t: &Tournament, rnum: u32, a: Uuid, b: Uuid) -> Option<&Board> {
        let (a, b) = (tid(t, a), tid(t, b));
        t.rounds
            .iter()
            .find(|r| r.number == rnum)?
            .boards
            .iter()
            .find(|bd| (bd.player1 == a && bd.player2 == b) || (bd.player1 == b && bd.player2 == a))
    }

    #[test]
    fn finalize_rejects_an_elo_estimate_with_no_scale_anchor() {
        use crate::settings::EloPriorShape;

        // ELO pairing with a flat unrated prior and an all-unrated field: nothing
        // pins the scale, so the estimate would have no absolute reference.
        let flat_unrated = TournamentSettings::elo_pairing()
            .map_estimator(|e| e.prior_shape_unrated = EloPriorShape::Flat);

        let mut t = Tournament::new("Open").unwrap();
        // During registration the field may still grow, so setting it is allowed.
        t.update_settings(flat_unrated).unwrap();
        t.add_player(named("A")).unwrap();
        t.add_player(named("B")).unwrap();

        // Finalizing into the unanchored estimate is refused, leaving registration
        // open (nothing was mutated).
        assert!(matches!(
            t.finalize_registration(),
            Err(TournamentError::EloEstimateUnanchored)
        ));
        assert!(!t.registration_finalized);

        // A single rated player anchors the scale (its Gaussian prior is centered on
        // a fixed rating), so finalization then succeeds.
        add_rated(&mut t, "R", 1500, false);
        assert!(t.finalize_registration().is_ok());
    }

    #[test]
    fn changing_settings_after_start_rejects_an_unanchored_elo_estimate() {
        use crate::settings::EloPriorShape;

        // A started Swiss tournament of unrated players.
        let mut t = Tournament::new("Open").unwrap();
        t.add_player(named("A")).unwrap();
        t.add_player(named("B")).unwrap();
        t.finalize_registration().unwrap();

        // Switching to ELO pairing with a flat unrated prior now — no rated player
        // to anchor the scale — is refused at the settings change.
        let flat_unrated = TournamentSettings::elo_pairing()
            .map_estimator(|e| e.prior_shape_unrated = EloPriorShape::Flat);
        assert!(matches!(
            t.update_settings(flat_unrated),
            Err(TournamentError::EloEstimateUnanchored)
        ));

        // The default ELO settings (a Gaussian unrated prior) DO anchor the scale,
        // so that change is accepted.
        assert!(t.update_settings(TournamentSettings::elo_pairing()).is_ok());
    }

    /// Record `winner` beating `loser` on their board in round `rnum`.
    fn decide(t: &mut Tournament, rnum: u32, winner: Uuid, loser: Uuid) {
        let (winner, loser) = (tid(t, winner), tid(t, loser));
        let round = t.rounds.iter().find(|r| r.number == rnum).unwrap();
        let idx = round
            .boards
            .iter()
            .position(|b| {
                (b.player1 == winner && b.player2 == loser)
                    || (b.player1 == loser && b.player2 == winner)
            })
            .expect("board exists");
        let clicked = if round.boards[idx].player1 == winner {
            Winner::Player1
        } else {
            Winner::Player2
        };
        t.toggle_board_winner(rnum, idx, clicked).unwrap();
    }

    /// Give player1 the win on every still-unplayed board (to complete a round).
    fn decide_rest(t: &mut Tournament, rnum: u32) {
        let n = t
            .rounds
            .iter()
            .find(|r| r.number == rnum)
            .unwrap()
            .boards
            .len();
        for idx in 0..n {
            let undecided =
                !t.rounds.iter().find(|r| r.number == rnum).unwrap().boards[idx].is_decided();
            if undecided {
                t.toggle_board_winner(rnum, idx, Winner::Player1).unwrap();
            }
        }
    }

    /// Mark the board pairing `a` and `b` in round `rnum` as a double no-show.
    fn no_show_both(t: &mut Tournament, rnum: u32, a: Uuid, b: Uuid) {
        let (a, b) = (tid(t, a), tid(t, b));
        let round = t.rounds.iter().find(|r| r.number == rnum).unwrap();
        let idx = round
            .boards
            .iter()
            .position(|bd| {
                (bd.player1 == a && bd.player2 == b) || (bd.player1 == b && bd.player2 == a)
            })
            .expect("board exists");
        t.set_board_no_show(
            rnum,
            idx,
            Some(Forfeit::Both(AbsenceKind::NoShow, AbsenceKind::NoShow)),
        )
        .unwrap();
    }

    #[test]
    fn cup_full_top8_run_produces_the_podium() {
        let mut t = Tournament::new("Champ").unwrap();
        enable_cup(&mut t);
        // 8 eligible (E0 strongest … E7 weakest) + 2 non-eligible Swiss players.
        let s: Vec<Uuid> = (0..8)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2000 - i * 100, true))
            .collect();
        let n9 = add_rated(&mut t, "N9", 1250, false);
        let n10 = add_rated(&mut t, "N10", 1200, false);
        t.finalize_registration_with(Some(8)).unwrap();
        let s_tid: Vec<TournamentId> = s.iter().map(|&id| tid(&t, id)).collect();
        assert_eq!(t.cup.as_ref().unwrap().seed_order, s_tid); // seeded by rating

        // Round 1 — quarterfinals fold the seeds, the two non-eligibles play Swiss.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let qf = find_board(&t, 1, s[0], s[7]).unwrap();
        assert!(matches!(
            qf.source,
            PairingSource::Cup {
                stage: CupStage::Quarterfinal
            }
        ));
        assert!(find_board(&t, 1, s[3], s[4]).is_some());
        let swiss = find_board(&t, 1, n9, n10).unwrap();
        assert!(matches!(swiss.source, PairingSource::Swiss));
        // Top seed of each QF wins.
        for i in 0..4 {
            decide(&mut t, 1, s[i], s[7 - i]);
        }
        decide_rest(&mut t, 1); // deciding the last board completes the round

        // Round 2 — semifinals fold [E0,E1,E2,E3]; the four QF losers drop to Swiss.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        assert!(matches!(
            find_board(&t, 2, s[0], s[3]).unwrap().source,
            PairingSource::Cup {
                stage: CupStage::Semifinal
            }
        ));
        assert!(find_board(&t, 2, s[1], s[2]).is_some());
        // A QF loser is now Swiss-paired (not in a cup board).
        assert!(matches!(
            t.rounds[1]
                .boards
                .iter()
                .find(|b| b.player1 == s_tid[4] || b.player2 == s_tid[4])
                .unwrap()
                .source,
            PairingSource::Swiss
        ));
        decide(&mut t, 2, s[0], s[3]); // E0 beats E3
        decide(&mut t, 2, s[1], s[2]); // E1 beats E2
        decide_rest(&mut t, 2);

        // Round 3 — the final (E0 vs E1) and the small final (the SF losers E3 vs E2).
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        assert!(matches!(
            find_board(&t, 3, s[0], s[1]).unwrap().source,
            PairingSource::Cup {
                stage: CupStage::Final
            }
        ));
        assert!(matches!(
            find_board(&t, 3, s[3], s[2]).unwrap().source,
            PairingSource::Cup {
                stage: CupStage::SmallFinal
            }
        ));
        decide(&mut t, 3, s[0], s[1]); // champion E0, runner-up E1
        decide(&mut t, 3, s[3], s[2]); // third E3, fourth E2
        decide_rest(&mut t, 3);

        let podium = t.cup_podium().unwrap();
        assert_eq!(podium.champion, Some(s_tid[0]));
        assert_eq!(podium.runner_up, Some(s_tid[1]));
        assert_eq!(podium.third, Some(s_tid[3]));
        assert_eq!(podium.fourth, Some(s_tid[2]));
    }

    #[test]
    fn qualifier_cup_plays_the_prequalified_in_the_swiss_then_folds_them_in() {
        let mut t = Tournament::new("Champ").unwrap();
        t.update_settings(TournamentSettings {
            cup_enabled: true,
            cup_format: CupFormat::Qualifier,
            ..Default::default()
        })
        .unwrap();
        // A size-8 qualifier cup takes 12 eligible: E0-E3 pre-qualified, E4-E11
        // in the qualification round. Two more players round out the Swiss.
        let e: Vec<Uuid> = (0..12)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2400 - i * 100, true))
            .collect();
        let n12 = add_rated(&mut t, "N12", 1150, false);
        let n13 = add_rated(&mut t, "N13", 1100, false);

        // Eight eligible is enough for a direct cup but not for this one.
        assert!(matches!(
            t.clone().finalize_registration_with(Some(16)),
            Err(TournamentError::NotEnoughEligiblePlayers {
                needed: 24,
                have: 12
            })
        ));
        t.finalize_registration_with(Some(8)).unwrap();
        assert_eq!(t.cup.as_ref().unwrap().seed_order.len(), 12);

        // Round 1: the play-off is cup, and the four pre-qualified are in the
        // Swiss pool with the two non-eligible players (three Swiss boards).
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let cup_boards: Vec<&Board> = t.rounds[0]
            .boards
            .iter()
            .filter(|b| matches!(b.source, PairingSource::Cup { .. }))
            .collect();
        assert_eq!(cup_boards.len(), 4);
        assert!(cup_boards.iter().all(|b| matches!(
            b.source,
            PairingSource::Cup {
                stage: CupStage::Qualification
            }
        )));
        // Seeds 5-12 fold 5v12, 6v11, 7v10, 8v9.
        assert!(find_board(&t, 1, e[4], e[11]).is_some());
        assert!(find_board(&t, 1, e[7], e[8]).is_some());
        let prequalified: HashSet<TournamentId> = e[..4].iter().map(|&id| tid(&t, id)).collect();
        assert!(t.rounds[0]
            .boards
            .iter()
            .filter(|b| prequalified.contains(&b.player1) || prequalified.contains(&b.player2))
            .all(|b| matches!(b.source, PairingSource::Swiss)));
        // The whole field of 14 is paired: 4 cup + 3 Swiss boards, nobody idle.
        assert_eq!(t.rounds[0].boards.len(), 7);
        assert!(t.rounds[0].sitouts.is_empty());

        // The higher seed wins every play-off, so E4-E7 qualify.
        for i in 0..4 {
            decide(&mut t, 1, e[4 + i], e[11 - i]);
        }
        decide_rest(&mut t, 1);

        // Round 2 is the bracket's first round: [E0..E3] ++ [E4..E7] folded.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        for (a, b) in [(0, 7), (1, 6), (2, 5), (3, 4)] {
            assert!(matches!(
                find_board(&t, 2, e[a], e[b]).unwrap().source,
                PairingSource::Cup {
                    stage: CupStage::Quarterfinal
                }
            ));
        }
        // The four beaten qualifiers drop into the Swiss alongside n12 and n13 —
        // six players, so three Swiss boards beside the four quarterfinals.
        let swiss: HashSet<TournamentId> = t.rounds[1]
            .boards
            .iter()
            .filter(|b| matches!(b.source, PairingSource::Swiss))
            .flat_map(|b| [b.player1, b.player2])
            .collect();
        let expected: HashSet<TournamentId> = e[8..]
            .iter()
            .chain([&n12, &n13])
            .map(|&id| tid(&t, id))
            .collect();
        assert_eq!(swiss, expected);
    }

    /// The pre-qualified play the open in round 1, but never each other: the
    /// engine's `CupPrequalified` rule keeps them apart.
    ///
    /// The ratings are chosen so the plain Swiss fold *would* pair them together:
    /// the round-1 pool is one score group of eight ranked P,P,O,O,P,P,O,O, whose
    /// top-half-vs-bottom-half fold is 1v5 and 2v6 — two all-pre-qualified boards.
    /// Only the new rule pulls the pairing off that fold.
    #[test]
    fn qualifier_cup_never_pairs_two_prequalified_in_the_qualification_round() {
        let mut t = Tournament::new("Champ").unwrap();
        t.update_settings(TournamentSettings {
            cup_enabled: true,
            cup_format: CupFormat::Qualifier,
            ..Default::default()
        })
        .unwrap();
        // E0-E3 are pre-qualified, E4-E11 play the qualification round.
        let elig_ratings = [
            2000, 1900, 1600, 1500, 1440, 1430, 1420, 1410, 1405, 1404, 1403, 1402,
        ];
        let e: Vec<Uuid> = elig_ratings
            .iter()
            .enumerate()
            .map(|(i, &r)| add_rated(&mut t, &format!("E{i}"), r, true))
            .collect();
        // Two open players slot *between* the pre-qualified and two below them,
        // which is what puts P,P,O,O,P,P,O,O on the fold.
        let others: Vec<Uuid> = [1800, 1700, 1000, 900]
            .iter()
            .enumerate()
            .map(|(i, &r)| add_rated(&mut t, &format!("N{i}"), r, false))
            .collect();
        t.finalize_registration_with(Some(8)).unwrap();

        t.prepare_round().unwrap();
        t.confirm_round().unwrap();

        let prequalified: HashSet<TournamentId> = e[..4].iter().map(|&id| tid(&t, id)).collect();
        let clashes: Vec<&Board> = t.rounds[0]
            .boards
            .iter()
            .filter(|b| prequalified.contains(&b.player1) && prequalified.contains(&b.player2))
            .collect();
        assert!(
            clashes.is_empty(),
            "pre-qualified paired together: {clashes:?}"
        );
        // Each of them is instead out in the Swiss against one of the others.
        let opens: HashSet<TournamentId> = others.iter().map(|&id| tid(&t, id)).collect();
        for &p in &prequalified {
            let board = t.rounds[0]
                .boards
                .iter()
                .find(|b| b.player1 == p || b.player2 == p)
                .expect("a pre-qualified player plays the open");
            let opp = if board.player1 == p {
                board.player2
            } else {
                board.player1
            };
            assert!(opens.contains(&opp));
            assert!(matches!(board.source, PairingSource::Swiss));
        }
    }

    /// The rule is a penalty, not a hard exclusion: with nobody entered beyond the
    /// cup field, the pre-qualified are the only players in the round-1 Swiss pool
    /// and have to face each other. Pairing must still succeed.
    #[test]
    fn prequalified_pair_each_other_when_the_open_is_empty() {
        let mut t = Tournament::new("Champ").unwrap();
        t.update_settings(TournamentSettings {
            cup_enabled: true,
            cup_format: CupFormat::Qualifier,
            ..Default::default()
        })
        .unwrap();
        let e: Vec<Uuid> = (0..12)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2400 - i * 100, true))
            .collect();
        t.finalize_registration_with(Some(8)).unwrap();
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();

        // 4 qualification boards + the 4 pre-qualified paired among themselves.
        assert_eq!(t.rounds[0].boards.len(), 6);
        let prequalified: HashSet<TournamentId> = e[..4].iter().map(|&id| tid(&t, id)).collect();
        let swiss: Vec<&Board> = t.rounds[0]
            .boards
            .iter()
            .filter(|b| matches!(b.source, PairingSource::Swiss))
            .collect();
        assert_eq!(swiss.len(), 2);
        assert!(swiss
            .iter()
            .all(|b| prequalified.contains(&b.player1) && prequalified.contains(&b.player2)));
    }

    /// The rule is confined to the qualification round: from round 2 on the
    /// eliminated and idle players pair normally, with no lingering separation.
    #[test]
    fn the_prequalified_rule_is_gone_after_the_qualification_round() {
        let mut t = Tournament::new("Champ").unwrap();
        t.update_settings(TournamentSettings {
            cup_enabled: true,
            cup_format: CupFormat::Qualifier,
            ..Default::default()
        })
        .unwrap();
        for i in 0..12 {
            add_rated(&mut t, &format!("E{i}"), 2400 - i * 100, true);
        }
        for i in 0..4 {
            add_rated(&mut t, &format!("N{i}"), 1150 - i * 10, false);
        }
        t.finalize_registration_with(Some(8)).unwrap();
        assert_eq!(t.prequalified_in_round(1).len(), 4);
        assert!(t.prequalified_in_round(2).is_empty());
        assert!(t.prequalified_in_round(3).is_empty());
    }

    #[test]
    fn long_cup_round_couples_all_cup_boards_gaps_the_next_round_and_resumes() {
        let mut t = Tournament::new("Champ").unwrap();
        t.update_settings(TournamentSettings {
            cup_enabled: true,
            long_boards_enabled: true,
            ..Default::default()
        })
        .unwrap();
        let s: Vec<Uuid> = (0..8)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2000 - i * 100, true))
            .collect();
        let n9 = add_rated(&mut t, "N9", 1250, false);
        let n10 = add_rated(&mut t, "N10", 1200, false);
        t.finalize_registration_with(Some(8)).unwrap();
        let s_tid: Vec<TournamentId> = s.iter().map(|&id| tid(&t, id)).collect();

        // Round 1: quarterfinals (cup) plus one Swiss board (n9 vs n10).
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();

        // Flag ONE cup board long → all four cup boards couple to long; the Swiss
        // board is left alone.
        let cup_idx = t.rounds[0]
            .boards
            .iter()
            .position(|b| matches!(b.source, PairingSource::Cup { .. }))
            .unwrap();
        t.set_board_long(1, cup_idx, true).unwrap();
        assert!(t.rounds[0]
            .boards
            .iter()
            .filter(|b| matches!(b.source, PairingSource::Cup { .. }))
            .all(|b| b.is_long()));
        assert_eq!(
            t.rounds[0]
                .boards
                .iter()
                .find(|b| matches!(b.source, PairingSource::Swiss))
                .map(|b| b.is_long()),
            Some(false)
        );

        // Deciding only the Swiss board completes round 1 with the QFs pending.
        decide(&mut t, 1, n9, n10);
        assert!(t.rounds[0].completed);
        assert!(t.rounds[0]
            .boards
            .iter()
            .filter(|b| matches!(b.source, PairingSource::Cup { .. }))
            .all(|b| !b.is_decided()));

        // Round 2 is the gap round: the eight cup players are still playing their
        // long QFs, which are carried into this round because that is where they
        // are finished. Nothing new is *paired* for them — only the two
        // non-eligibles are — and no fresh cup board is drawn.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let r2 = t.rounds.last().unwrap();
        assert!(r2
            .boards
            .iter()
            .all(|b| matches!(b.source, PairingSource::Swiss | PairingSource::Carried)));
        assert!(find_board(&t, 2, n9, n10).is_some());
        assert_eq!(
            r2.boards
                .iter()
                .filter(|b| b.source == PairingSource::Carried)
                .count(),
            4,
            "the four quarterfinals are being played in this round"
        );
        assert!(r2
            .boards
            .iter()
            .filter(|b| b.source != PairingSource::Carried)
            .all(|b| !s_tid.contains(&b.player1) && !s_tid.contains(&b.player2)));

        // Deciding everything *but* the carried QFs leaves round 2 open, and
        // round 3 is gated by that ordinary rule.
        let swiss: Vec<usize> = r2
            .boards
            .iter()
            .enumerate()
            .filter(|(_, b)| b.source != PairingSource::Carried)
            .map(|(i, _)| i)
            .collect();
        for i in swiss {
            t.toggle_board_winner(2, i, Winner::Player1).unwrap();
        }
        assert!(!t.rounds[1].completed);
        assert_eq!(
            t.prepare_round(),
            Err(TournamentError::PreviousRoundNotComplete)
        );

        // Record the QF results on their live records, in round 2; round 3 then
        // hosts the semifinal.
        for i in 0..4 {
            decide(&mut t, 2, s[i], s[7 - i]);
        }
        assert!(t.rounds[1].completed);
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        assert!(matches!(
            find_board(&t, 3, s[0], s[3]).unwrap().source,
            PairingSource::Cup {
                stage: CupStage::Semifinal
            }
        ));
        assert!(find_board(&t, 3, s[1], s[2]).is_some());
    }

    #[test]
    fn cup_double_no_show_drops_both_to_swiss_and_byes_the_next_opponent() {
        let mut t = Tournament::new("Champ").unwrap();
        enable_cup(&mut t);
        let s: Vec<Uuid> = (0..8)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2000 - i * 100, true))
            .collect();
        add_rated(&mut t, "N9", 1250, false);
        add_rated(&mut t, "N10", 1200, false);
        t.finalize_registration_with(Some(8)).unwrap();
        let s_tid: Vec<TournamentId> = s.iter().map(|&id| tid(&t, id)).collect();

        // Round 1 (QF). The top match (s0 v s7) is a double no-show — neither
        // shows up — so it produces no winner. The other three top seeds win.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        no_show_both(&mut t, 1, s[0], s[7]);
        decide(&mut t, 1, s[1], s[6]);
        decide(&mut t, 1, s[2], s[5]);
        decide(&mut t, 1, s[3], s[4]);
        decide_rest(&mut t, 1);

        // Both no-shows score nothing this round and drop into the Swiss open.
        let scored = |t: &Tournament, id| {
            t.standings()
                .into_iter()
                .find(|st| st.player_id == id)
                .unwrap()
                .points
        };
        assert_eq!(scored(&t, s[0]), 0);
        assert_eq!(scored(&t, s[7]), 0);

        // Round 2 (SF). The vanished winner's slot leaves s3 (drawn to face it) to
        // advance unopposed — the one case a cup bye occurs — while s0 and s7 are
        // now Swiss-paired. As no-shows they default to absent in the new draft,
        // so clear that to keep them in the round (the referee's call).
        let draft = t.prepare_round().unwrap();
        assert!(draft.absent.contains(&s_tid[0]) && draft.absent.contains(&s_tid[7]));
        t.update_draft(Vec::new(), Vec::new(), Vec::new()).unwrap();
        t.confirm_round().unwrap();
        assert!(
            matches!(
                t.rounds[1].sitout(s_tid[3]).map(|s| s.kind),
                Some(SitoutKind::CupBye { .. })
            ),
            "s3 gets the cup bye"
        );
        for dropped in [s_tid[0], s_tid[7]] {
            let board = t.rounds[1]
                .boards
                .iter()
                .find(|b| b.player1 == dropped || b.player2 == dropped)
                .expect("dropped cup player is now Swiss-paired");
            assert!(matches!(board.source, PairingSource::Swiss));
        }
        // The cup bye is worth a point, like any bye (2 half-points).
        assert_eq!(scored(&t, s[3]), 2);
        decide(&mut t, 2, s[1], s[2]); // the one real semifinal
        decide_rest(&mut t, 2);

        // Round 3. Final is s3 (via bye) vs s1; the small final has only one
        // semifinal loser (s2), who takes third by walkover — no fourth.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        decide(&mut t, 3, s[1], s[3]); // s1 wins the final
        decide_rest(&mut t, 3);

        let podium = t.cup_podium().unwrap();
        assert_eq!(podium.champion, Some(s_tid[1]));
        assert_eq!(podium.runner_up, Some(s_tid[3]));
        assert_eq!(podium.third, Some(s_tid[2]));
        assert_eq!(podium.fourth, None); // the fourth-place slot never existed
    }

    #[test]
    fn cup_bye_player_also_marked_absent_is_not_double_scored() {
        // Same setup that produces a cup bye (a R1 double no-show leaves s3 to
        // advance unopposed in R2), but then the referee also marks that very
        // cup-bye player absent. A player may hold only one sit-out; without the
        // guard s3 would carry both a CupBye and an Absent entry and be scored
        // twice.
        let mut t = Tournament::new("Champ").unwrap();
        enable_cup(&mut t);
        let s: Vec<Uuid> = (0..8)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2000 - i * 100, true))
            .collect();
        add_rated(&mut t, "N9", 1250, false);
        add_rated(&mut t, "N10", 1200, false);
        t.finalize_registration_with(Some(8)).unwrap();
        let s_tid: Vec<TournamentId> = s.iter().map(|&id| tid(&t, id)).collect();

        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        no_show_both(&mut t, 1, s[0], s[7]);
        decide(&mut t, 1, s[1], s[6]);
        decide(&mut t, 1, s[2], s[5]);
        decide(&mut t, 1, s[3], s[4]);
        decide_rest(&mut t, 1);

        // R2: s3 takes the cup bye. Additionally mark s3 absent (the referee's odd
        // call — the UI hides cup-bye players, but `update_draft` accepts any known
        // player, so it must be handled).
        t.prepare_round().unwrap();
        t.update_draft(vec![s_tid[3]], Vec::new(), Vec::new())
            .unwrap();
        t.confirm_round().unwrap();

        // Exactly one sit-out for s3, and it's the cup bye — not a second Absent.
        let s3_sitouts: Vec<_> = t.rounds[1]
            .sitouts
            .iter()
            .filter(|so| so.player == s_tid[3])
            .collect();
        assert_eq!(
            s3_sitouts.len(),
            1,
            "a cup-bye player must not also receive an Absent sit-out"
        );
        assert!(matches!(s3_sitouts[0].kind, SitoutKind::CupBye { .. }));

        // And the cup bye scores its single point (2 half-points), not two.
        let s3_points = t
            .standings()
            .into_iter()
            .find(|st| st.player_id == s[3])
            .unwrap()
            .points;
        assert_eq!(s3_points, 2);
    }

    #[test]
    fn cup_double_no_show_final_awards_no_champion() {
        let mut t = Tournament::new("Champ").unwrap();
        enable_cup(&mut t);
        let s: Vec<Uuid> = (0..8)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2000 - i * 100, true))
            .collect();
        add_rated(&mut t, "N9", 1250, false);
        add_rated(&mut t, "N10", 1200, false);
        t.finalize_registration_with(Some(8)).unwrap();
        let s_tid: Vec<TournamentId> = s.iter().map(|&id| tid(&t, id)).collect();

        // Play a clean bracket down to the final round.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        for i in 0..4 {
            decide(&mut t, 1, s[i], s[7 - i]);
        }
        decide_rest(&mut t, 1);
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        decide(&mut t, 2, s[0], s[3]);
        decide(&mut t, 2, s[1], s[2]);
        decide_rest(&mut t, 2);

        // Both finalists no-show the final; the small final is played normally.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        no_show_both(&mut t, 3, s[0], s[1]);
        decide(&mut t, 3, s[3], s[2]);
        decide_rest(&mut t, 3);

        // The podium resolves without panicking: no champion or runner-up, but
        // third and fourth from the small final stand.
        let podium = t.cup_podium().unwrap();
        assert_eq!(podium.champion, None);
        assert_eq!(podium.runner_up, None);
        assert_eq!(podium.third, Some(s_tid[3]));
        assert_eq!(podium.fourth, Some(s_tid[2]));
    }

    #[test]
    fn finalize_validates_the_cup() {
        // Cup enabled but no size chosen.
        let mut t = Tournament::new("C").unwrap();
        enable_cup(&mut t);
        for i in 0..8 {
            add_rated(&mut t, &format!("E{i}"), 2000 - i * 10, true);
        }
        assert_eq!(
            t.clone().finalize_registration_with(None),
            Err(TournamentError::CupSizeRequired)
        );
        assert_eq!(
            t.clone().finalize_registration_with(Some(12)),
            Err(TournamentError::InvalidCupSize { size: 12 })
        );
        // Only 8 eligible, ask for 16.
        assert_eq!(
            t.clone().finalize_registration_with(Some(16)),
            Err(TournamentError::NotEnoughEligiblePlayers {
                needed: 16,
                have: 8
            })
        );
        // A rejected cup leaves registration open.
        assert!(!t.registration_finalized);
        assert!(t.cup.is_none());
        // Exactly enough works.
        t.finalize_registration_with(Some(8)).unwrap();
        assert_eq!(t.cup.unwrap().size, 8);
    }

    #[test]
    fn cannot_remove_a_seeded_cup_player() {
        let mut t = Tournament::new("C").unwrap();
        enable_cup(&mut t);
        let seeds: Vec<Uuid> = (0..8)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2000 - i * 10, true))
            .collect();
        let bystander = add_rated(&mut t, "B", 1000, false);
        t.finalize_registration_with(Some(8)).unwrap();
        assert_eq!(
            t.remove_player(seeds[0]),
            Err(TournamentError::CannotRemoveCupPlayer)
        );
        // A non-seeded player can still be removed.
        assert!(t.remove_player(bystander).is_ok());
    }

    #[test]
    fn absent_cup_player_still_gets_a_bracket_board() {
        let mut t = Tournament::new("C").unwrap();
        enable_cup(&mut t);
        let s: Vec<Uuid> = (0..8)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2000 - i * 10, true))
            .collect();
        t.finalize_registration_with(Some(8)).unwrap();
        // Mark the top seed absent — their QF board is still generated (unplayed),
        // to be forfeited by the referee.
        t.prepare_round().unwrap();
        t.update_draft(vec![tid(&t, s[0])], vec![], Vec::new())
            .unwrap();
        t.confirm_round().unwrap();
        let board = find_board(&t, 1, s[0], s[7]).expect("bracket board created despite absence");
        assert!(matches!(board.source, PairingSource::Cup { .. }));
        assert!(!board.is_decided());
    }

    /// A finalized, valid cup — the starting point for the malformed-file tests
    /// below, which break it one field at a time.
    fn direct_cup_of_8() -> Tournament {
        let mut t = Tournament::new("C").unwrap();
        enable_cup(&mut t);
        for i in 0..8 {
            add_rated(&mut t, &format!("E{i}"), 2000 - i * 10, true);
        }
        t.finalize_registration_with(Some(8)).unwrap();
        t.validate_loaded().unwrap();
        t
    }

    /// Only `finalize_registration_with` enforced the bracket size, and a loaded
    /// file never ran it — while `cup_bracket()` (built for *every* tournament
    /// response) derives its round count from that size and indexes the
    /// semifinal's two losers. A size the fold can't reach is a panic on the read
    /// path, so `validate_loaded` has to restate the rule.
    #[test]
    fn validate_loaded_rejects_an_unsupported_cup_size() {
        let t = direct_cup_of_8();
        for size in [0, 1, 2, 7, 12, 128] {
            let mut bad = t.clone();
            bad.cup.as_mut().unwrap().size = size;
            assert_eq!(
                bad.validate_loaded(),
                Err(TournamentError::InvalidCupSize { size }),
                "a cup of size {size} must not import"
            );
        }
    }

    /// The seed list and the bracket size are frozen together; a file where they
    /// disagree folds a short list against a full bracket (or splits it at a point
    /// past its end, under the qualifier format).
    #[test]
    fn validate_loaded_rejects_a_seed_list_that_doesnt_fill_the_bracket() {
        let t = direct_cup_of_8();
        let mut short = t.clone();
        short.cup.as_mut().unwrap().seed_order.truncate(3);
        assert_eq!(
            short.validate_loaded(),
            Err(TournamentError::CupSeedCountMismatch {
                size: 8,
                expected: 8,
                found: 3,
            })
        );
        let mut long = t.clone();
        long.cup.as_mut().unwrap().seed_order.push(TournamentId(1));
        assert_eq!(
            long.validate_loaded(),
            Err(TournamentError::CupSeedCountMismatch {
                size: 8,
                expected: 8,
                found: 9,
            })
        );
        // Switching the stored format changes how many seeds the same bracket
        // takes: a qualifier cup of 8 needs 12, not 8.
        let mut requalified = t;
        requalified.cup.as_mut().unwrap().format = CupFormat::Qualifier;
        assert_eq!(
            requalified.validate_loaded(),
            Err(TournamentError::CupSeedCountMismatch {
                size: 8,
                expected: 12,
                found: 8,
            })
        );
    }

    /// Seeds are tournament numbers, resolved against the field by everything the
    /// bracket feeds. A repeated one puts a player in two slots; one that names
    /// nobody has no player to resolve to at all.
    #[test]
    fn validate_loaded_rejects_seeds_that_dont_name_the_field() {
        let t = direct_cup_of_8();
        let mut twice = t.clone();
        let top = twice.cup.as_ref().unwrap().seed_order[0];
        twice.cup.as_mut().unwrap().seed_order[7] = top;
        assert_eq!(
            twice.validate_loaded(),
            Err(TournamentError::DuplicateCupSeed { seed: top })
        );
        let mut phantom = t;
        phantom.cup.as_mut().unwrap().seed_order[7] = TournamentId(99);
        assert_eq!(
            phantom.validate_loaded(),
            Err(TournamentError::UnknownCupSeed {
                seed: TournamentId(99)
            })
        );
    }

    // --- Frozen explanations and the staleness watermark ---------------------

    /// An eight-player tournament with `rounds` rounds played out (player 1 of
    /// every board wins), so the later rounds have real score groups to pair on.
    fn eight_players_played(rounds: u32) -> Tournament {
        let mut t = Tournament::new("Frozen").unwrap();
        for n in ["A", "B", "C", "D", "E", "F", "G", "H"] {
            t.add_player(named(n)).unwrap();
        }
        t.finalize_registration().unwrap();
        for r in 1..=rounds {
            start_next_round(&mut t);
            for i in 0..t.rounds[(r - 1) as usize].boards.len() {
                t.toggle_board_winner(r, i, Winner::Player1).unwrap();
            }
        }
        t
    }

    #[test]
    fn a_rounds_explanation_survives_a_correction_to_an_earlier_round() {
        let mut t = eight_players_played(3);
        let before = t.explain_round(3).unwrap();
        // Correcting round 1 changes the standings round 3 was paired from — the
        // exact edit that used to rewrite round 3's rationale under the referee.
        t.toggle_board_winner(1, 0, Winner::Player2).unwrap();
        assert_eq!(t.explain_round(3).unwrap(), before);
        // ...and the round that edit was *inside* keeps its own explanation too.
        assert_eq!(t.explain_round(1).unwrap().round, 1);

        // The point of freezing, made explicit: recomputing round 3's ledger from
        // the tournament as it now stands gives a *different* answer — a model
        // that never paired anything, which is what used to be served.
        let recomputed = {
            let swiss: Vec<(UnitKey, UnitKey)> = t.rounds[2]
                .boards
                .iter()
                .filter(|b| matches!(b.source, PairingSource::Swiss))
                .map(|b| (UnitKey::from(b.player1), UnitKey::from(b.player2)))
                .collect();
            let units = t.pairing_units(3, &t.rounds[..2]);
            explain_pairing(3, &t.settings, &units, &swiss, None)
        };
        assert_ne!(recomputed, before);
        assert_eq!(t.explanations_faithful_through, 1);
    }

    #[test]
    fn confirming_a_round_extends_the_watermark_but_never_rescues_a_stale_prefix() {
        let mut t = eight_players_played(2);
        assert_eq!(t.explanations_faithful_through, 2);

        // An edit inside round 1 leaves round 1's own explanation faithful (it was
        // paired before the game was played) and warns from round 2 on.
        t.toggle_board_winner(1, 0, Winner::Player2).unwrap();
        assert_eq!(t.explanations_faithful_through, 1);

        // Round 3 is faithful by construction, but the mark is a prefix: it cannot
        // step over round 2 to say so.
        start_next_round(&mut t);
        assert_eq!(t.explanations_faithful_through, 1);
    }

    #[test]
    fn a_pairing_relevant_player_edit_stales_every_explanation_and_a_no_op_does_not() {
        let mut t = eight_players_played(2);
        let id = t.players[0].id;

        // Renaming touches nothing the engine reads.
        t.edit_player(
            id,
            NewPlayer {
                last_name: "Renamed".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(t.explanations_faithful_through, 2);

        // A rating does: it sits under every round's model at once.
        t.edit_player(
            id,
            NewPlayer {
                last_name: "Renamed".into(),
                rating: Some(1800),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(t.explanations_faithful_through, 0);
    }

    #[test]
    fn a_point_adjustment_stales_every_explanation() {
        let mut t = eight_players_played(2);
        let id = t.players[0].id;
        let player = t
            .add_point_adjustment(id, 1, "late arrival".into())
            .unwrap();
        let adjustment = player.adjustments[0].id;
        assert_eq!(t.explanations_faithful_through, 0);

        // Removing it doesn't undo the doubt — the ledgers still describe scores
        // that moved twice.
        start_next_round(&mut t);
        t.remove_point_adjustment(id, adjustment).unwrap();
        assert_eq!(t.explanations_faithful_through, 0);
    }

    #[test]
    fn cancelling_a_round_pulls_the_watermark_back_inside_the_rounds_it_indexes() {
        let mut t = eight_players_played(2);
        assert_eq!(t.explanations_faithful_through, 2);
        t.cancel_last_round().unwrap();
        assert_eq!(t.explanations_faithful_through, 1);
    }

    #[test]
    fn re_pairing_the_current_round_re_freezes_its_explanation_and_keeps_the_mark() {
        // Round 1 played, round 2 paired but not yet played — the only state a
        // round can be re-paired in.
        let mut t = eight_players_played(1);
        start_next_round(&mut t);
        assert_eq!(t.explanations_faithful_through, 2);

        let (a, b) = {
            let boards = &t.rounds[1].boards;
            (boards[0].player1, boards[1].player2)
        };
        t.force_pairing(UnitKey::from(a), UnitKey::from(b)).unwrap();
        // The re-paired round carries the ledger of the pairing it now has.
        let explanation = t.explain_round(2).unwrap();
        assert_eq!(explanation.round, 2);
        assert_eq!(explanation, t.rounds[1].explanation);
        assert_eq!(t.explanations_faithful_through, 2);
    }
}
