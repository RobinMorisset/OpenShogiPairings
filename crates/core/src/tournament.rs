//! The tournament aggregate.
//!
//! For now a tournament is just a named list of players. Rounds, pairings and
//! results will be added here later; keeping the mutation logic in this crate
//! (rather than in the server) means the server, a future CLI, and the Tauri app
//! all share exactly one implementation.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::cup::{Cup, CupPairings, CupPodium, CUP_SIZES};
use crate::pairing::{
    counterfactual_forbid, counterfactual_force, explain_pairing, pair_round_weighted,
    Counterfactual, CounterfactualMode, RoundExplanation, ScopeReason, PHANTOM,
};
use crate::player::{NewPlayer, Player, PointAdjustment};
use crate::round::{
    Board, Handicap, HandicapGame, NoShow, PairingSource, Round, RoundDraft, Winner,
};
use crate::scoring::compute_scores;
use crate::settings::TournamentSettings;
use crate::standings::{compute_standings, Standing};
use crate::units::TournamentId;

/// On-disk / on-the-wire format version for a serialized [`Tournament`].
///
/// Bumped whenever the saved shape changes incompatibly, so that loading an old
/// file can be detected (and, later, migrated) instead of silently mis-parsed.
///
/// v2: players carry `last_name` + `first_name` + `nationality` instead of a
/// single `name`.
/// v3: tournaments carry a list of `rounds`.
/// v4: players carry a list of manual point `adjustments`.
pub const TOURNAMENT_FORMAT_VERSION: u32 = 4;

fn default_format_version() -> u32 {
    TOURNAMENT_FORMAT_VERSION
}

/// Minimum number of players required to start a round.
pub const MIN_PLAYERS_PER_ROUND: usize = 2;

/// A tournament: a name and its registered players.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Tournament {
    /// Format version of this record (see [`TOURNAMENT_FORMAT_VERSION`]).
    #[serde(default = "default_format_version")]
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
    /// A board's "long game" flag can only be changed on the current round.
    #[error("a long game can only be set on the current round")]
    NotCurrentRound,
    /// The next round can't be prepared while a long game from two rounds ago is
    /// still unresolved (a long game spans exactly two rounds).
    #[error("the long game from round {round} must be resolved first")]
    UnresolvedLongGame { round: u32 },
    /// No round with the given number exists.
    #[error("no round number {0}")]
    RoundNotFound(u32),
    /// No board with the given index exists in the round.
    #[error("no board {board} in round {round}")]
    BoardNotFound { round: u32, board: usize },
    /// A handicap was requested for two players whose ratings are equal (or both
    /// unrated), so there is no unambiguous handicap giver.
    #[error("a handicap game needs two players with different ratings")]
    HandicapNeedsRatingDifference,
    /// A handicap was requested for a cup (direct-elimination) board — cup games
    /// are always played even.
    #[error("cup games cannot have a handicap")]
    HandicapNotAllowedForCup,
    /// The serialized record uses a format version this build cannot read.
    #[error("unsupported tournament format version {found} (this build supports {supported})")]
    UnsupportedFormatVersion { found: u32, supported: u32 },
    /// The cup is enabled but no size was chosen at finalization.
    #[error("choose a cup size to finalize (the cup is enabled)")]
    CupSizeRequired,
    /// The chosen cup size is not one of the supported powers of two.
    #[error("invalid cup size {size} (must be one of 8, 16, 32, 64)")]
    InvalidCupSize { size: u32 },
    /// Fewer eligible players than the chosen cup size.
    #[error("need at least {needed} eligible players for the cup (have {have})")]
    NotEnoughEligiblePlayers { needed: u32, have: usize },
    /// A player who is seeded in the cup bracket cannot be removed.
    #[error("cannot remove a player seeded in the cup")]
    CannotRemoveCupPlayer,
    /// A player who has already played a game (appears on a board of a started
    /// round) cannot be removed — their results are referenced by every opponent's
    /// score record, so erasing them would corrupt those tie-breaks. Mark them
    /// absent for future rounds instead.
    #[error("cannot remove a player who has already played a game")]
    CannotRemovePlayedPlayer,
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
        let mut player = Player::from_new(new);
        // A player registered after finalization gets the next free number
        // immediately (independent of rating); before finalization, numbers are
        // assigned in bulk by `finalize_registration`.
        if self.registration_finalized {
            player.tournament_id = Some(self.next_tournament_id());
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
            .unwrap_or(0)
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
        let player = self.player_mut(id)?;
        // Reuse the normalization in `from_new`, but keep the existing id.
        let normalized = Player::from_new(new);
        player.last_name = normalized.last_name;
        player.first_name = normalized.first_name;
        player.rating = normalized.rating;
        player.grade = normalized.grade;
        player.nationality = normalized.nationality;
        player.club = normalized.club;
        Ok(player)
    }

    /// Remove the player with the given id.
    ///
    /// Returns [`TournamentError::PlayerNotFound`] if no such player exists,
    /// [`TournamentError::CannotRemoveCupPlayer`] if the player is seeded in the
    /// cup bracket (removing them would corrupt it), or
    /// [`TournamentError::CannotRemovePlayedPlayer`] if the player has already been
    /// paired into a game (their results are referenced by every opponent's score
    /// record — mark them absent for future rounds instead).
    pub fn remove_player(&mut self, id: Uuid) -> Result<(), TournamentError> {
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
        Ok(())
    }

    /// Replace the tournament settings (MacMahon groups, degressive schedule, …),
    /// stored in canonical form (see [`TournamentSettings::normalized`]).
    ///
    /// Allowed at any point; the caller (UI) warns when registration is already
    /// finalized, since changing the groups shifts everyone's points and future
    /// pairings.
    pub fn update_settings(&mut self, settings: TournamentSettings) -> &TournamentSettings {
        self.settings = settings.normalized();
        &self.settings
    }

    /// The ranked standings (points and tie-breaks) from the completed rounds.
    ///
    /// This is the canonical ordering — used by the Results tab and, later, the
    /// American grid — so scoring lives in one place rather than being re-derived
    /// by each client.
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
    /// power of two (8/16/32/64) with at least that many eligible players; the top
    /// `cup_size` eligible players (by tournament number) are frozen into the
    /// bracket. The cup is validated *before* any mutation, so a rejected cup
    /// leaves registration open for the referee to fix.
    pub fn finalize_registration_with(
        &mut self,
        cup_size: Option<u32>,
    ) -> Result<(), TournamentError> {
        if self.registration_finalized {
            return Err(TournamentError::RegistrationAlreadyFinalized);
        }

        // Pre-validate the cup so a bad request doesn't half-finalize.
        let cup_size = if self.settings.cup_enabled {
            let size = cup_size.ok_or(TournamentError::CupSizeRequired)?;
            if !CUP_SIZES.contains(&size) {
                return Err(TournamentError::InvalidCupSize { size });
            }
            let eligible = self.players.iter().filter(|p| p.eligible).count();
            if eligible < size as usize {
                return Err(TournamentError::NotEnoughEligiblePlayers {
                    needed: size,
                    have: eligible,
                });
            }
            Some(size)
        } else {
            None
        };

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
            self.players[idx].tournament_id = Some(rank as u32 + 1);
        }

        // Seed the cup from the top eligible players by tournament number.
        if let Some(size) = cup_size {
            let mut eligible: Vec<&Player> = self.players.iter().filter(|p| p.eligible).collect();
            eligible.sort_by_key(|p| p.tournament_id.unwrap_or(u32::MAX));
            let seed_order = eligible
                .into_iter()
                .take(size as usize)
                .map(|p| p.tournament_id.expect("finalized players have a number"))
                .collect();
            self.cup = Some(Cup { size, seed_order });
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
        let reason = reason.trim().to_string();
        if reason.is_empty() {
            return Err(TournamentError::EmptyAdjustmentReason);
        }
        if delta == 0 {
            return Err(TournamentError::ZeroPointAdjustment);
        }
        let player = self.player_mut(player_id)?;
        player.adjustments.push(PointAdjustment {
            id: Uuid::new_v4(),
            delta,
            reason,
        });
        Ok(player)
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
        Ok(player)
    }

    /// The cup podium (champion, runner-up, third, fourth), once the final round is
    /// decided; `None` when there is no cup or the final isn't finished.
    pub fn cup_podium(&self) -> Option<CupPodium> {
        self.cup.as_ref()?.podium(&self.rounds)
    }

    /// The players the cup bracket will pair in the round currently being drafted
    /// (empty when there is no draft, no cup, or the draft round is past the cup).
    /// Lets clients keep those players out of the Swiss customization UI.
    pub fn draft_cup_players(&self) -> Vec<u32> {
        let (Some(draft), Some(cup)) = (&self.draft, &self.cup) else {
            return Vec::new();
        };
        cup.matches_for_round(&self.rounds, draft.number)
            .map(|p| {
                p.matches
                    .iter()
                    .flat_map(|m| [m.player1, m.player2])
                    .chain(p.byes)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Cancel the most recent round, stepping the tournament back one stage.
    ///
    /// Peels off exactly one step: if a round is currently being drafted, the
    /// draft is discarded; otherwise the last round is removed. Removing round 1
    /// also undoes `finalize_registration` (tournament numbers and any cup
    /// bracket are cleared) — back to open registration. Earlier rounds keep
    /// their results, so a removed round N>1 simply lands back on round N-1,
    /// which stays complete (its games are all still recorded) and ready to
    /// re-prepare the next round.
    ///
    /// This makes it easy to re-pair and replay a round in simulations, and lets
    /// a referee undo a round in the rare cases that call for it. It is undoable
    /// like any other mutation.
    ///
    /// Returns [`TournamentError::NoRoundToCancel`] when there is neither a draft
    /// nor any round to remove.
    pub fn cancel_last_round(&mut self) -> Result<(), TournamentError> {
        if self.draft.take().is_some() {
            return Ok(());
        }
        if self.rounds.pop().is_none() {
            return Err(TournamentError::NoRoundToCancel);
        }
        // Removing the very first round reopens registration; later rounds leave
        // the preceding one untouched (and thus still complete).
        if self.rounds.is_empty() {
            self.registration_finalized = false;
            self.cup = None;
            for player in &mut self.players {
                player.tournament_id = None;
            }
        }
        Ok(())
    }

    /// Begin preparing the next round (the `RoundDraft` state).
    ///
    /// Requires registration finalized and the previous round (if any)
    /// completed. The draft's absent set defaults to the previous round's
    /// absentees (restricted to players who still exist), so recurring absences
    /// carry over while late joiners are not pre-marked absent.
    pub fn prepare_round(&mut self) -> Result<&RoundDraft, TournamentError> {
        if !self.registration_finalized {
            return Err(TournamentError::RegistrationNotFinalized);
        }
        if self.draft.is_some() {
            return Err(TournamentError::DraftAlreadyExists);
        }
        if let Some(last) = self.rounds.last() {
            if !last.completed {
                return Err(TournamentError::PreviousRoundNotComplete);
            }
        }

        let number = self.rounds.len() as u32 + 1;
        // A long game started in round R spans R and R+1 only; its players sit out
        // R+1's pairing. Before R+2 can be prepared it must be resolved, so a long
        // game never straddles three rounds. (Its players are excluded from R+1 in
        // `confirm_round`; here we refuse to advance past R+1 while it is pending.)
        if let Some(stale) = self
            .rounds
            .iter()
            .find(|r| r.number + 1 < number && r.boards.iter().any(|b| b.long_pending()))
        {
            return Err(TournamentError::UnresolvedLongGame {
                round: stale.number,
            });
        }
        let existing: HashSet<u32> = self
            .players
            .iter()
            .filter_map(|p| p.tournament_id)
            .collect();
        // Default the new draft's absentees to the previous round's absentees,
        // plus anyone who was a no-show there — a no-show reads as "didn't turn
        // up", so it should carry over the same way an explicit absence does (the
        // referee can still uncheck it).
        let default_absent: Vec<u32> = self
            .rounds
            .last()
            .map(|r| {
                let mut absent: Vec<u32> = r.absent.clone();
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
                absent
            })
            .unwrap_or_default();

        self.draft = Some(RoundDraft {
            number,
            absent: default_absent,
            forced_boards: Vec::new(),
            forced_bye: None,
        });
        Ok(self.draft.as_ref().expect("just set the draft"))
    }

    /// Replace the current draft's customization (absent set, forced pairings,
    /// forced bye). Structural consistency is validated when the round is
    /// confirmed; here we only check that every referenced player exists.
    pub fn update_draft(
        &mut self,
        absent: Vec<u32>,
        forced_boards: Vec<Board>,
        forced_bye: Option<u32>,
    ) -> Result<&RoundDraft, TournamentError> {
        if self.draft.is_none() {
            return Err(TournamentError::NoDraft);
        }
        let known: HashSet<u32> = self
            .players
            .iter()
            .filter_map(|p| p.tournament_id)
            .collect();
        let referenced = absent
            .iter()
            .chain(forced_boards.iter().flat_map(|b| [&b.player1, &b.player2]))
            .chain(forced_bye.iter());
        for id in referenced {
            if !known.contains(id) {
                return Err(TournamentError::InvalidDraft(format!(
                    "references unknown player {id}"
                )));
            }
        }

        let draft = self.draft.as_mut().expect("draft present");
        draft.absent = absent;
        draft.forced_boards = forced_boards
            .into_iter()
            .map(|b| Board::pending(b.player1, b.player2, 0, PairingSource::Forced))
            .collect();
        draft.forced_bye = forced_bye;
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
        let draft = self.draft.clone().ok_or(TournamentError::NoDraft)?;

        let absent: HashSet<u32> = draft.absent.iter().copied().collect();
        // Present players, by tournament number (rounds are tid-native). Every
        // player has a number here — a round is only confirmed post-finalization.
        let present: Vec<u32> = self
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
            let cup_diff = |p1: u32, p2: u32| {
                cup_scores.get_tid(TournamentId(p1)).points.halves() as i32
                    - cup_scores.get_tid(TournamentId(p2)).points.halves() as i32
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
        let cup_players: HashSet<u32> = cup_boards
            .iter()
            .flat_map(|b| [b.player1, b.player2])
            .chain(cup_byes.iter().copied())
            .collect();

        // Players still busy on an unresolved long game (a two-round board started
        // in an earlier round). They sit out this round's pairing entirely — like
        // cup players — while their long game finishes.
        let busy_long: HashSet<u32> = self
            .rounds
            .iter()
            .flat_map(|r| &r.boards)
            .filter(|b| b.long_pending())
            .flat_map(|b| [b.player1, b.player2])
            .collect();

        // The Swiss pool: present players not taken by the cup and not mid-long-game.
        let swiss_present: Vec<u32> = present
            .iter()
            .copied()
            .filter(|id| !cup_players.contains(id) && !busy_long.contains(id))
            .collect();
        let swiss_set: HashSet<u32> = swiss_present.iter().copied().collect();

        // Validate the referee's forced boards/bye against the Swiss pool.
        let mut placed: HashSet<u32> = HashSet::new();
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
        if let Some(bye) = draft.forced_bye {
            if cup_players.contains(&bye) {
                return Err(TournamentError::InvalidDraft(
                    "the forced bye is a cup player".into(),
                ));
            }
            if busy_long.contains(&bye) {
                return Err(TournamentError::InvalidDraft(
                    "the forced bye is a player still in a long game".into(),
                ));
            }
            if !swiss_set.contains(&bye) {
                return Err(TournamentError::InvalidDraft(
                    "the forced bye is an absent player".into(),
                ));
            }
            if !placed.insert(bye) {
                return Err(TournamentError::InvalidDraft(
                    "the forced bye is also in a forced pairing".into(),
                ));
            }
            // With a forced bye, the Swiss players left to auto-pair must be even.
            let leftover = swiss_present.len() - 2 * draft.forced_boards.len() - 1;
            if !leftover.is_multiple_of(2) {
                return Err(TournamentError::InvalidDraft(
                    "a forced bye needs an odd number of Swiss players".into(),
                ));
            }
        }

        // Pair the Swiss pool with the engine, then prepend the cup boards.
        let swiss_round = pair_round_weighted(
            draft.number,
            &self.players,
            &self.settings,
            &self.rounds,
            &swiss_present,
            &draft.forced_boards,
            draft.forced_bye,
        );
        let mut boards = cup_boards;
        boards.extend(swiss_round.boards);

        // Display order: cup boards first, then by the rank (from the standings
        // entering this round) of the board's best-placed player — the same
        // criteria the pairings view sorts by, so the "Board" numbers it shows
        // match this stored order instead of drifting from a client-only sort.
        // Skipped by the simulator (see `confirm_round_unordered`): it never shows
        // the boards, and a full standings computation per round is the cost.
        if order_boards_for_display {
            // Standings face callers by id; boards store numbers. Map through each
            // player's number (this path is display-only, off the hot sim loop).
            let tid_of: HashMap<Uuid, u32> = self
                .players
                .iter()
                .filter_map(|p| p.tournament_id.map(|t| (p.id, t)))
                .collect();
            let rank_of: HashMap<u32, usize> = self
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

        let round = Round {
            number: draft.number,
            boards,
            bye: swiss_round.bye,
            cup_byes,
            absent: draft.absent,
            completed: false,
        };
        self.rounds.push(round);
        self.draft = None;
        Ok(self.rounds.last().expect("just pushed a round"))
    }

    /// Explain the Swiss pairings of the round numbered `round_number`: for each
    /// engine-paired board (and the bye), which rules were relaxed and by how
    /// much, plus a per-rule round report. Forced and cup boards are omitted —
    /// they were not chosen by the engine.
    ///
    /// Reconstructs the exact inputs the round was paired from (the standings
    /// entering it and the same free set), so the ledger matches what the engine
    /// actually optimized.
    pub fn explain_round(&self, round_number: u32) -> Result<RoundExplanation, TournamentError> {
        let idx = self
            .rounds
            .iter()
            .position(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        let round = &self.rounds[idx];
        let completed = &self.rounds[..idx];
        let swiss_boards: Vec<(u32, u32)> = round
            .boards
            .iter()
            .filter(|b| matches!(b.source, PairingSource::Swiss))
            .map(|b| (b.player1, b.player2))
            .collect();
        Ok(explain_pairing(
            round.number,
            &self.players,
            &self.settings,
            completed,
            &swiss_boards,
            round.bye,
        ))
    }

    /// Explain a counterfactual pairing in round `round_number`, relative to the
    /// round's confirmed Swiss pairing: which boards would change, the rings of
    /// affected players, and the net per-rule cost. [`CounterfactualMode::Force`]
    /// asks "why aren't A and B paired?"; [`CounterfactualMode::Forbid`] asks
    /// "why did you pair A and B?".
    ///
    /// If either player isn't an engine-paired Swiss player of that round (they
    /// were forced, are a cup player, or sat out), the result is `scoped_out`
    /// with the reason and no diff — the engine didn't choose their board.
    pub fn explain_counterfactual(
        &self,
        round_number: u32,
        a: u32,
        b: u32,
        mode: CounterfactualMode,
    ) -> Result<Counterfactual, TournamentError> {
        let idx = self
            .rounds
            .iter()
            .position(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        let round = &self.rounds[idx];
        let completed = &self.rounds[..idx];
        let swiss_boards: Vec<(u32, u32)> = round
            .boards
            .iter()
            .filter(|bd| matches!(bd.source, PairingSource::Swiss))
            .map(|bd| (bd.player1, bd.player2))
            .collect();

        // Both probed players must be engine-paired (in a Swiss board or the bye).
        // `PHANTOM` stands for the bye itself (the nil UUID, never a real player
        // id) — it's in scope exactly when this round actually has a Swiss bye
        // to negotiate.
        let in_swiss = |id: u32| {
            round.bye == Some(id) || swiss_boards.iter().any(|&(x, y)| x == id || y == id)
        };
        for id in [a, b] {
            if id == PHANTOM {
                if round.bye.is_none() {
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
        Ok(solve(
            round.number,
            &self.players,
            &self.settings,
            completed,
            &swiss_boards,
            round.bye,
            a,
            b,
        ))
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
    pub fn force_pairing(&mut self, a: u32, b: u32) -> Result<&Round, TournamentError> {
        let round = self.rounds.last().ok_or(TournamentError::NoCurrentRound)?;
        if round.completed {
            return Err(TournamentError::RoundHasResults);
        }
        if round.boards.iter().any(|bd| bd.is_decided()) {
            return Err(TournamentError::RoundHasResults);
        }

        // Rebuild the draft the round came from: its absentees, its existing
        // forced boards, plus the newly forced pair (or bye). The engine
        // re-picks everything else, including the bye when it isn't fixed here.
        let mut forced_boards: Vec<Board> = round
            .boards
            .iter()
            .filter(|bd| matches!(bd.source, PairingSource::Forced))
            .map(|bd| Board::pending(bd.player1, bd.player2, 0, PairingSource::Forced))
            .collect();
        let forced_bye = match (a == PHANTOM, b == PHANTOM) {
            (true, false) => Some(b),
            (false, true) => Some(a),
            _ => {
                forced_boards.push(Board::pending(a, b, 0, PairingSource::Forced));
                None
            }
        };
        let draft = RoundDraft {
            number: round.number,
            absent: round.absent.clone(),
            forced_boards,
            forced_bye,
        };

        // Drop the round and re-confirm from the reconstructed draft. Earlier
        // rounds stay completed, so the standings entering this round are intact.
        self.rounds.pop();
        self.draft = Some(draft);
        self.confirm_round()
    }

    /// Why `id` is out of the engine's hands for `round`: forced, a cup player,
    /// or sitting out. Errors only if `id` isn't a player at all.
    fn scope_reason(&self, round: &Round, id: u32) -> Result<ScopeReason, TournamentError> {
        if round.absent.contains(&id) {
            return Ok(ScopeReason::Absent);
        }
        for bd in &round.boards {
            if bd.player1 == id || bd.player2 == id {
                return Ok(match bd.source {
                    PairingSource::Forced => ScopeReason::Forced,
                    PairingSource::Cup { .. } => ScopeReason::Cup,
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
        let round = self
            .rounds
            .iter_mut()
            .find(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        let board = round
            .boards
            .get_mut(board_index)
            .ok_or(TournamentError::BoardNotFound {
                round: round_number,
                board: board_index,
            })?;
        board.result = if board.result == Some(clicked) {
            None
        } else {
            Some(clicked)
        };
        // Recording an actual result supersedes a no-show — the game was played
        // after all — so the two states stay mutually exclusive.
        if board.result.is_some() {
            board.no_show = None;
        }
        round.completed = round.is_complete();
        Ok(&round.boards[board_index])
    }

    /// Mark a board as a no-show, or clear it.
    ///
    /// `absent` names the side(s) that failed to appear — one player, or
    /// [`NoShow::Both`] — or `None` to clear the flag back to a normal unplayed
    /// board. A single no-show credits the opponent a free point exactly like a
    /// bye; [`NoShow::Both`] leaves no winner (both take a zero loss). A no-show
    /// isn't a played game, so recording one clears any actual result and draw
    /// flag on the board. Like recording a winner, this keeps the round's
    /// `completed` flag in sync — a no-show counts toward closing the round.
    pub fn set_board_no_show(
        &mut self,
        round_number: u32,
        board_index: usize,
        absent: Option<NoShow>,
    ) -> Result<&Board, TournamentError> {
        let round = self
            .rounds
            .iter_mut()
            .find(|r| r.number == round_number)
            .ok_or(TournamentError::RoundNotFound(round_number))?;
        let board = round
            .boards
            .get_mut(board_index)
            .ok_or(TournamentError::BoardNotFound {
                round: round_number,
                board: board_index,
            })?;
        board.no_show = absent;
        if absent.is_some() {
            board.result = None;
            board.drawn = false;
        }
        round.completed = round.is_complete();
        Ok(&round.boards[board_index])
    }

    /// Flag (or unflag) a board as a "long game": double time control, lasting two
    /// rounds and scoring two points for the winner (see
    /// `docs/two-round-boards.md`).
    ///
    /// Allowed only on the **current** (last) round, and only when the tournament
    /// enables long boards. Flagging *on* requires the board undecided; flagging
    /// *off* is allowed even after a result, so the referee can demote a long game
    /// that actually finished in a single round (or resolved by forfeit) back to
    /// an ordinary one-point board. Keeps the round's `completed` flag in sync,
    /// since flagging the last-undecided board long can close the round.
    ///
    /// Cup (direct-elimination) boards are not supported yet and are rejected.
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
        let round = &mut self.rounds[idx];
        let board = round
            .boards
            .get_mut(board_index)
            .ok_or(TournamentError::BoardNotFound {
                round: round_number,
                board: board_index,
            })?;
        // Making a board long after it is decided is meaningless; turning it off
        // after a result is the intended demote path.
        if long && board.is_decided() {
            return Err(TournamentError::RoundHasResults);
        }
        // A cup board's flag couples to every cup board of the round: a whole cup
        // bracket round is long or not as a unit, which is exactly the invariant
        // the cup↔tournament-round mapping relies on (see `Cup::cup_schedule`). A
        // Swiss/forced board toggles on its own.
        if matches!(board.source, PairingSource::Cup { .. }) {
            for b in round.boards.iter_mut() {
                if matches!(b.source, PairingSource::Cup { .. }) {
                    b.long = long;
                }
            }
        } else {
            board.long = long;
        }
        round.completed = round.is_complete();
        Ok(&self.rounds[idx])
    }

    /// Set (or clear) the "a draw occurred" flag on a board. The game is still
    /// replayed to a decisive [`Winner`]; this only records that the draw
    /// happened, which matters for end-of-tournament ELO.
    pub fn set_board_drawn(
        &mut self,
        round_number: u32,
        board_index: usize,
        drawn: bool,
    ) -> Result<&Board, TournamentError> {
        let board = self.board_mut(round_number, board_index)?;
        board.drawn = drawn;
        Ok(board)
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
    pub fn set_board_handicap(
        &mut self,
        round_number: u32,
        board_index: usize,
        handicap: Option<Handicap>,
    ) -> Result<&Board, TournamentError> {
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
        let board = self.board_mut(round_number, board_index)?;
        board.handicap = game;
        Ok(board)
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
    fn player_rating(&self, tid: u32) -> Option<u32> {
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
        assert!(t.rounds[0].boards[0].long);

        // Flagging a *decided* board on is refused...
        t.toggle_board_winner(1, 1, Winner::Player1).unwrap();
        assert_eq!(
            t.set_board_long(1, 1, true),
            Err(TournamentError::RoundHasResults)
        );
        // ...but flagging a decided long board *off* (the demote path) is allowed.
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        assert!(t.rounds[0].boards[0].is_decided());
        t.set_board_long(1, 0, false).unwrap();
        assert!(!t.rounds[0].boards[0].long);
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

        // Round 2 excludes the two long players.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let r2 = t.rounds.last().unwrap();
        for b in &r2.boards {
            assert!(!long_players.contains(&b.player1));
            assert!(!long_players.contains(&b.player2));
        }
        assert!(r2.bye.is_none_or(|x| !long_players.contains(&x)));

        // The long flag can no longer be touched on round 1 (not the current round).
        assert_eq!(
            t.set_board_long(1, 0, false),
            Err(TournamentError::NotCurrentRound)
        );

        // Complete round 2, then round 3 is gated on the still-pending long game.
        let n = t.rounds.last().unwrap().boards.len();
        for i in 0..n {
            t.toggle_board_winner(2, i, Winner::Player1).unwrap();
        }
        assert!(t.rounds[1].completed);
        assert_eq!(
            t.prepare_round(),
            Err(TournamentError::UnresolvedLongGame { round: 1 })
        );

        // Enter the long result; now the next round can be prepared.
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        assert!(t.prepare_round().is_ok());
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
            assert!(round.bye.is_some());
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
        };
        assert_eq!(id_of(high), Some(1));
        assert_eq!(id_of(mid), Some(2));
        assert_eq!(id_of(low), Some(3));
        assert_eq!(id_of(unrated), Some(4)); // unrated last

        // Added after finalization → next free number, regardless of rating.
        let newcomer = t.add_player(rated("Newcomer", 9000)).unwrap();
        assert_eq!(newcomer.tournament_id, Some(5));
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

        // A draft is peeled first, leaving the completed rounds untouched.
        t.prepare_round().unwrap();
        assert!(t.draft.is_some());
        t.cancel_last_round().unwrap();
        assert!(t.draft.is_none());
        assert!(t.rounds.is_empty());

        // Play round 1 (recording the game completes it).
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
            t.toggle_board_winner(1, 0, Winner::Player1).unwrap().result,
            Some(Winner::Player1)
        );
        // click player 2 -> switch winner
        assert_eq!(
            t.toggle_board_winner(1, 0, Winner::Player2).unwrap().result,
            Some(Winner::Player2)
        );
        // click the current winner again -> back to not played
        assert_eq!(
            t.toggle_board_winner(1, 0, Winner::Player2).unwrap().result,
            None
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
        let board = t.set_board_no_show(1, 0, Some(NoShow::Player2)).unwrap();
        assert_eq!(board.no_show, Some(NoShow::Player2));
        assert_eq!(board.result, None);
        assert!(t.rounds[0].completed);

        // Recording an actual winner supersedes the no-show (game was played).
        let board = t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        assert_eq!(board.result, Some(Winner::Player1));
        assert_eq!(board.no_show, None);

        // And marking a no-show again clears the recorded result.
        let board = t.set_board_no_show(1, 0, Some(NoShow::Player1)).unwrap();
        assert_eq!(board.result, None);
        assert_eq!(board.no_show, Some(NoShow::Player1));

        // Both players absent settles the board too, with no winner.
        let board = t.set_board_no_show(1, 0, Some(NoShow::Both)).unwrap();
        assert_eq!(board.no_show, Some(NoShow::Both));
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
        t.set_board_no_show(1, 0, Some(NoShow::Player2)).unwrap();
        t.set_board_no_show(1, 1, Some(NoShow::Both)).unwrap();
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
        t.update_draft(vec![c], vec![], None).unwrap();
        t.confirm_round().unwrap();
        assert_eq!(t.rounds[0].absent, vec![c]);
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
        let ids: Vec<u32> = ["A", "B", "C", "D", "E"]
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
        t.update_draft(vec![], forced, Some(ids[4])).unwrap();
        let round = t.confirm_round().unwrap();
        assert_eq!(round.bye, Some(ids[4]));
        // A vs C is present as a board; B and D auto-paired.
        assert!(round
            .boards
            .iter()
            .any(|b| b.player1 == ids[0] && b.player2 == ids[2]));
        assert_eq!(round.boards.len(), 2);
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

        let round = t.force_pairing(a, b).unwrap();
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
        assert_eq!(t.force_pairing(a, b), Err(TournamentError::RoundHasResults));
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
        let bye = r1.bye.expect("odd count byes someone");
        let playing = r1.boards[0].player1;

        let round = t.force_pairing(playing, PHANTOM).unwrap();
        assert_eq!(round.number, 1, "the same round is re-paired");
        assert_eq!(round.bye, Some(playing), "the forced player now byes");
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
        let bye = r1.bye.expect("odd count byes someone");

        let cf = t
            .explain_counterfactual(1, bye, PHANTOM, CounterfactualMode::Forbid)
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
        let tid_of: HashMap<Uuid, u32> = t
            .players
            .iter()
            .filter_map(|p| p.tournament_id.map(|tid| (p.id, tid)))
            .collect();
        let rank_of: HashMap<u32, usize> = t
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
        assert!(board.drawn);
        assert_eq!(board.result, Some(Winner::Player1)); // result untouched
                                                         // Effective winner unaffected by the draw flag.
        assert_eq!(board.effective_winner(false), Some(Winner::Player1));
        assert!(!t.set_board_drawn(1, 0, false).unwrap().drawn);
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
        assert_eq!(board.result, Some(receiver_wins)); // actual result recorded
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

    // --- Hybrid cup -------------------------------------------------------

    use crate::round::CupStage;

    fn enable_cup(t: &mut Tournament) {
        t.update_settings(TournamentSettings {
            cup_enabled: true,
            ..Default::default()
        });
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
    fn tid(t: &Tournament, id: Uuid) -> u32 {
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
        t.set_board_no_show(rnum, idx, Some(NoShow::Both)).unwrap();
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
        let s_tid: Vec<u32> = s.iter().map(|&id| tid(&t, id)).collect();
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
    fn long_cup_round_couples_all_cup_boards_gaps_the_next_round_and_resumes() {
        let mut t = Tournament::new("Champ").unwrap();
        t.update_settings(TournamentSettings {
            cup_enabled: true,
            long_boards_enabled: true,
            ..Default::default()
        });
        let s: Vec<Uuid> = (0..8)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2000 - i * 100, true))
            .collect();
        let n9 = add_rated(&mut t, "N9", 1250, false);
        let n10 = add_rated(&mut t, "N10", 1200, false);
        t.finalize_registration_with(Some(8)).unwrap();
        let s_tid: Vec<u32> = s.iter().map(|&id| tid(&t, id)).collect();

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
            .all(|b| b.long));
        assert_eq!(
            t.rounds[0]
                .boards
                .iter()
                .find(|b| matches!(b.source, PairingSource::Swiss))
                .map(|b| b.long),
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

        // Round 2 is the gap round: the eight cup players are busy on their long
        // QFs, so only the two non-eligibles are paired, with no cup boards.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let r2 = t.rounds.last().unwrap();
        assert!(r2
            .boards
            .iter()
            .all(|b| matches!(b.source, PairingSource::Swiss)));
        assert!(find_board(&t, 2, n9, n10).is_some());
        assert!(r2
            .boards
            .iter()
            .all(|b| !s_tid.contains(&b.player1) && !s_tid.contains(&b.player2)));

        // Complete round 2; round 3 can't be prepared until the long QFs resolve.
        decide_rest(&mut t, 2);
        assert_eq!(
            t.prepare_round(),
            Err(TournamentError::UnresolvedLongGame { round: 1 })
        );

        // Record the QF results; round 3 then hosts the semifinal.
        for i in 0..4 {
            decide(&mut t, 1, s[i], s[7 - i]);
        }
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
        let s_tid: Vec<u32> = s.iter().map(|&id| tid(&t, id)).collect();

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
        t.update_draft(Vec::new(), Vec::new(), None).unwrap();
        t.confirm_round().unwrap();
        assert!(
            t.rounds[1].cup_byes.contains(&s_tid[3]),
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
    fn cup_double_no_show_final_awards_no_champion() {
        let mut t = Tournament::new("Champ").unwrap();
        enable_cup(&mut t);
        let s: Vec<Uuid> = (0..8)
            .map(|i| add_rated(&mut t, &format!("E{i}"), 2000 - i * 100, true))
            .collect();
        add_rated(&mut t, "N9", 1250, false);
        add_rated(&mut t, "N10", 1200, false);
        t.finalize_registration_with(Some(8)).unwrap();
        let s_tid: Vec<u32> = s.iter().map(|&id| tid(&t, id)).collect();

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
        t.update_draft(vec![tid(&t, s[0])], vec![], None).unwrap();
        t.confirm_round().unwrap();
        let board = find_board(&t, 1, s[0], s[7]).expect("bracket board created despite absence");
        assert!(matches!(board.source, PairingSource::Cup { .. }));
        assert!(board.result.is_none());
    }
}
