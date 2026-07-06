//! The tournament aggregate.
//!
//! For now a tournament is just a named list of players. Rounds, pairings and
//! results will be added here later; keeping the mutation logic in this crate
//! (rather than in the server) means the server, a future CLI, and the Tauri app
//! all share exactly one implementation.

use std::cmp::Ordering;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pairing::pair_round_weighted;
use crate::player::{NewPlayer, Player};
use crate::round::{Board, Handicap, HandicapGame, Round, RoundDraft, Winner};
use crate::settings::TournamentSettings;
use crate::standings::{compute_standings, Standing};

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
    /// There is no round in progress to complete.
    #[error("no round in progress to complete")]
    NoRoundToComplete,
    /// The round still has games without a result.
    #[error("all games in the round must be played first")]
    RoundHasUnplayedGames,
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
            settings: TournamentSettings::default(),
            players: Vec::new(),
            registration_finalized: false,
            draft: None,
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

    /// Replace the MacMahon threshold list (stored sorted and de-duplicated).
    ///
    /// Allowed at any point; the caller (UI) warns when registration is already
    /// finalized, since changing the groups shifts everyone's points and future
    /// pairings.
    pub fn update_settings(&mut self, macmahon_thresholds: Vec<u32>) -> &TournamentSettings {
        self.settings.macmahon_thresholds =
            TournamentSettings::normalize_thresholds(macmahon_thresholds);
        &self.settings
    }

    /// The ranked standings (points and tie-breaks) from the completed rounds.
    ///
    /// This is the canonical ordering — used by the Results tab and, later, the
    /// American grid — so scoring lives in one place rather than being re-derived
    /// by each client.
    pub fn standings(&self) -> Vec<Standing> {
        compute_standings(&self.players, &self.settings, &self.rounds)
    }

    /// Finalize registration. Prerequisite for starting the first round.
    ///
    /// A no-op beyond flipping the flag for now (registration stays editable);
    /// it exists to gate round creation behind an explicit step.
    pub fn finalize_registration(&mut self) -> Result<(), TournamentError> {
        if self.registration_finalized {
            return Err(TournamentError::RegistrationAlreadyFinalized);
        }
        // Assign tournament numbers 1..N in the sorted-table order: highest ELO
        // first, unrated players last, ties broken by registration order.
        let mut order: Vec<usize> = (0..self.players.len()).collect();
        order.sort_by(|&a, &b| {
            let (ra, rb) = (self.players[a].rating, self.players[b].rating);
            let by_rating = match (ra, rb) {
                (Some(x), Some(y)) => y.cmp(&x), // descending
                (Some(_), None) => Ordering::Less, // rated before unrated
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            by_rating.then(a.cmp(&b)) // stable: registration order breaks ties
        });
        for (rank, &idx) in order.iter().enumerate() {
            self.players[idx].tournament_id = Some(rank as u32 + 1);
        }
        self.registration_finalized = true;
        Ok(())
    }

    /// Complete the current (last, in-progress) round.
    ///
    /// Only possible once every game in the round has a result. Completing a
    /// round locks it in and unlocks starting the next one.
    pub fn complete_current_round(&mut self) -> Result<(), TournamentError> {
        let round = self
            .rounds
            .last_mut()
            .ok_or(TournamentError::NoRoundToComplete)?;
        if round.completed {
            return Err(TournamentError::NoRoundToComplete);
        }
        if round.boards.iter().any(|b| b.result.is_none()) {
            return Err(TournamentError::RoundHasUnplayedGames);
        }
        round.completed = true;
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
        let existing: HashSet<Uuid> = self.players.iter().map(|p| p.id).collect();
        let default_absent: Vec<Uuid> = self
            .rounds
            .last()
            .map(|r| {
                r.absent
                    .iter()
                    .copied()
                    .filter(|id| existing.contains(id))
                    .collect()
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
        absent: Vec<Uuid>,
        forced_boards: Vec<Board>,
        forced_bye: Option<Uuid>,
    ) -> Result<&RoundDraft, TournamentError> {
        if self.draft.is_none() {
            return Err(TournamentError::NoDraft);
        }
        let known: HashSet<Uuid> = self.players.iter().map(|p| p.id).collect();
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
            .map(|b| Board {
                player1: b.player1,
                player2: b.player2,
                result: None,
                drawn: false,
                handicap: None,
            })
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
        let draft = self.draft.clone().ok_or(TournamentError::NoDraft)?;

        let absent: HashSet<Uuid> = draft.absent.iter().copied().collect();
        let present: Vec<Uuid> = self
            .players
            .iter()
            .map(|p| p.id)
            .filter(|id| !absent.contains(id))
            .collect();
        if present.len() < MIN_PLAYERS_PER_ROUND {
            return Err(TournamentError::NotEnoughPresentPlayers {
                needed: MIN_PLAYERS_PER_ROUND,
                have: present.len(),
            });
        }

        let present_set: HashSet<Uuid> = present.iter().copied().collect();
        let mut placed: HashSet<Uuid> = HashSet::new();
        for board in &draft.forced_boards {
            if board.player1 == board.player2 {
                return Err(TournamentError::InvalidDraft(
                    "a forced pairing has the same player twice".into(),
                ));
            }
            for player in [board.player1, board.player2] {
                if !present_set.contains(&player) {
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
            if !present_set.contains(&bye) {
                return Err(TournamentError::InvalidDraft(
                    "the forced bye is an absent player".into(),
                ));
            }
            if !placed.insert(bye) {
                return Err(TournamentError::InvalidDraft(
                    "the forced bye is also in a forced pairing".into(),
                ));
            }
            // With a forced bye, the players left to auto-pair must be even.
            let leftover = present.len() - 2 * draft.forced_boards.len() - 1;
            if leftover % 2 != 0 {
                return Err(TournamentError::InvalidDraft(
                    "a forced bye needs an odd number of present players".into(),
                ));
            }
        }

        let mut round = pair_round_weighted(
            draft.number,
            &self.players,
            &self.settings,
            &self.rounds,
            &present,
            &draft.forced_boards,
            draft.forced_bye,
        );
        round.absent = draft.absent;
        self.rounds.push(round);
        self.draft = None;
        Ok(self.rounds.last().expect("just pushed a round"))
    }

    /// Toggle the winner of a board in response to a player being clicked.
    ///
    /// If `clicked` is already the recorded winner, the result is cleared (back
    /// to "not played"); otherwise `clicked` becomes the winner. This gives the
    /// three states — not played, player 1 won, player 2 won — from clicks alone.
    pub fn toggle_board_winner(
        &mut self,
        round_number: u32,
        board_index: usize,
        clicked: Winner,
    ) -> Result<&Board, TournamentError> {
        let board = self.board_mut(round_number, board_index)?;
        board.result = if board.result == Some(clicked) {
            None
        } else {
            Some(clicked)
        };
        Ok(board)
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
    /// ratings are equal (or both unrated), since then there is no giver.
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
                let rating = |id| self.players.iter().find(|p| p.id == id).and_then(|p| p.rating);
                let giver = Self::rating_giver(rating(board.player1), rating(board.player2))
                    .ok_or(TournamentError::HandicapNeedsRatingDifference)?;
                Some(HandicapGame { handicap, giver })
            }
        };
        let board = self.board_mut(round_number, board_index)?;
        board.handicap = game;
        Ok(board)
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
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        {
            let round = t.confirm_round().unwrap();
            assert_eq!(round.number, 1);
            assert_eq!(round.boards.len(), 1); // 3 players → 1 board + 1 bye
            assert!(round.bye.is_some());
        }

        // Play and complete round 1 before starting round 2.
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        t.complete_current_round().unwrap();

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
        let id_of = |uuid| t.players.iter().find(|p| p.id == uuid).unwrap().tournament_id;
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
        // Can't prepare round 2 while round 1 is in progress.
        assert_eq!(
            t.prepare_round(),
            Err(TournamentError::PreviousRoundNotComplete)
        );
        // Can't complete while a game is unplayed.
        assert_eq!(
            t.complete_current_round(),
            Err(TournamentError::RoundHasUnplayedGames)
        );
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        t.complete_current_round().unwrap();
        assert!(t.rounds[0].completed);
        // Now round 2 can be prepared and started.
        start_next_round(&mut t);
        assert_eq!(t.rounds.len(), 2);
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
        assert_eq!(t.toggle_board_winner(1, 0, Winner::Player2).unwrap().result, None);
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
        let a = t.add_player(named("A")).unwrap().id;
        let b = t.add_player(named("B")).unwrap().id;
        let c = t.add_player(named("C")).unwrap().id;
        t.finalize_registration().unwrap();

        // Round 1: C absent, A vs B.
        t.prepare_round().unwrap();
        t.update_draft(vec![c], vec![], None).unwrap();
        t.confirm_round().unwrap();
        assert_eq!(t.rounds[0].absent, vec![c]);
        assert_eq!(t.rounds[0].boards.len(), 1); // A vs B, no bye
        t.toggle_board_winner(1, 0, Winner::Player1).unwrap();
        t.complete_current_round().unwrap();

        // Round 2 draft defaults absent to the previous round's absentees.
        let draft = t.prepare_round().unwrap();
        assert_eq!(draft.absent, vec![c]);
        let _ = (a, b);
    }

    #[test]
    fn confirm_honors_forced_pairing_and_bye() {
        let mut t = Tournament::new("Cup").unwrap();
        let ids: Vec<Uuid> = ["A", "B", "C", "D", "E"]
            .iter()
            .map(|n| t.add_player(named(n)).unwrap().id)
            .collect();
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        // Force A vs C, and E as the bye (5 present → odd, ok).
        let forced = vec![Board {
            player1: ids[0],
            player2: ids[2],
            result: None,
            drawn: false,
            handicap: None,
        }];
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

    fn rated(last_name: &str, rating: u32) -> NewPlayer {
        NewPlayer {
            last_name: last_name.to_string(),
            rating: Some(rating),
            ..Default::default()
        }
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
        assert_eq!(board.effective_winner(), Some(Winner::Player1));
        assert!(!t.set_board_drawn(1, 0, false).unwrap().drawn);
    }

    #[test]
    fn handicap_freezes_giver_and_flips_effective_winner() {
        let mut t = Tournament::new("Cup").unwrap();
        // High is rated above Low, so High is the giver.
        let high = t.add_player(rated("High", 2000)).unwrap().id;
        let _low = t.add_player(rated("Low", 1000)).unwrap().id;
        t.finalize_registration().unwrap();
        start_next_round(&mut t);

        let (p1_is_high, giver) = {
            let b = &t.rounds[0].boards[0];
            (b.player1 == high, ())
        };
        let _ = giver;

        // Give a 4-piece handicap. Whoever actually loses, the giver (High)
        // scores the effective point.
        t.set_board_handicap(1, 0, Some(Handicap::FourPiece)).unwrap();
        // The receiver actually wins the game...
        let receiver_wins = if p1_is_high { Winner::Player2 } else { Winner::Player1 };
        t.toggle_board_winner(1, 0, receiver_wins).unwrap();

        let board = &t.rounds[0].boards[0];
        assert_eq!(board.result, Some(receiver_wins)); // actual result recorded
        let giver_side = if p1_is_high { Winner::Player1 } else { Winner::Player2 };
        assert_eq!(board.handicap.unwrap().giver, giver_side);
        // ...but the giver still counts as the effective winner.
        assert_eq!(board.effective_winner(), Some(giver_side));

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
        start_next_round(&mut t);
        t.set_board_handicap(1, 0, Some(Handicap::TwoPiece)).unwrap();
        let board = &t.rounds[0].boards[0];
        let giver_side = if board.player1 == rated_id {
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
}
