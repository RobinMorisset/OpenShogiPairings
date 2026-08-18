//! Teams: the unit of pairing and ranking in a team tournament.
//!
//! A team tournament (the format that traditionally precedes the European
//! championships) pairs *teams*, but the games themselves stay ordinary
//! individual boards: a match between two teams of `size` players is `size`
//! boards, board k against board k. See `docs/archive/team-tournaments.md`.
//!
//! Only the roster is stored. Everything team-level that could be derived —
//! match results, team scores, the board↔match grouping — is recomputed by
//! replay from the boards, the same way the cup replays its bracket, so editing
//! a past board result re-derives every team outcome that depends on it.

use std::cmp::Ordering;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use typed_index_collections::TiVec;
use uuid::Uuid;

use crate::pairing::{explain_pairing, pair_round_weighted, PairingUnit};
use crate::player::{Player, PointAdjustment};
use crate::round::{
    AbsenceKind, Board, Forfeit, Outcome, PairingSource, Round, Sitout, SitoutKind, SitoutValue,
    Winner,
};
use crate::team_scoring::{match_boards, team_match_views, team_units, TeamMatchView, TeamSlots};
use crate::tournament::{Tournament, TournamentError, MIN_TEAMS_PER_ROUND};
use crate::units::{TeamId, TournamentId, UnitKey};

/// A registered team. Stored on [`Tournament`]; present (and
/// non-empty) only in team mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(deny_unknown_fields)]
pub struct Team {
    /// Stable identity, like [`Player::id`] — survives a rename or a roster edit.
    pub id: Uuid,
    /// Dense 1-based team number, assigned at finalization by descending average
    /// pairing rating (mirroring player numbering). The key the team score tables
    /// are indexed by, and what the boards of a match resolve to. `None` until
    /// registration is finalized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tournament_id: Option<TeamId>,
    /// Non-empty, and unique among the tournament's teams case-insensitively.
    pub name: String,
    /// The members in **board order** (index 0 = board 1), exactly
    /// [`TeamSettings::size`] of them once finalized.
    ///
    /// Referenced by registration [`Uuid`] rather than tournament number, because
    /// players have no number until finalization — and rosters are built before
    /// it. The order is frozen at finalization: a later rating edit must not
    /// reshuffle who plays board 1 (the same principle as frozen cup eligibility
    /// and frozen handicap givers).
    ///
    /// [`TeamSettings::size`]: crate::settings::TeamSettings::size
    #[serde(default)]
    pub members: Vec<Uuid>,
    /// Manual point bonuses/maluses the referee has applied to this **team**
    /// (a fair-play bonus, a penalty, a correction).
    ///
    /// Adjustments are team-level in team mode because the ranking is: a delta
    /// on one member would move nothing a referee can see, which is why the
    /// per-player ones are refused there. Folded into the team's points
    /// alongside its MacMahon starting points, so they shape both the standings
    /// and the score-gap pairing weight from the moment they are added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adjustments: Vec<PointAdjustment>,
}

impl Team {
    /// A new team with no members yet.
    pub fn new(name: String) -> Self {
        Team {
            id: Uuid::new_v4(),
            tournament_id: None,
            name,
            members: Vec::new(),
            adjustments: Vec::new(),
        }
    }

    /// Two team names collide when they are equal ignoring case and surrounding
    /// space — the check both creation and renaming use.
    pub fn same_name(a: &str, b: &str) -> bool {
        a.trim().eq_ignore_ascii_case(b.trim())
    }
}

/// A player's **pairing rating**: their real rating, or the referee-assigned
/// stand-in for an unrated team member (see [`Player::pairing_rating`]).
///
/// This is what team averages, the fold order and team numbering are computed
/// from. It is deliberately *not* what any export reads: the grid's `N` flag and
/// rating column stay honest, and the player remains unrated everywhere
/// user-facing.
pub fn pairing_rating(player: &Player) -> Option<u32> {
    player.rating.or(player.pairing_rating)
}

/// Board-order comparison of two pairing ratings: stronger first, unrated last.
///
/// The one definition of "board order by strength", shared by the sort and by
/// where a newcomer is inserted, so the two can't drift apart.
fn by_pairing_rating(a: Option<u32>, b: Option<u32>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// The average pairing rating over a roster — `None` unless *every* member has
/// one, and for an empty roster.
///
/// A mean over the rated members only would be a number about part of the team
/// presented as a fact about the team: a strong pair plus an unrated third reads
/// as a strong team, and it is the whole three that get paired. So a team with
/// anyone unrated has no average at all, and falls where an unrated player falls
/// in individual mode — last in the fold order and in team numbering, the only
/// two places this feeds without MacMahon. With MacMahon starting points in use,
/// finalization requires a pairing ELO from every member, so a team that reaches
/// the first round always has one.
pub(crate) fn average_pairing_rating(members: &[&Player]) -> Option<u32> {
    if members.is_empty() {
        return None;
    }
    let rated: Vec<u32> = members.iter().filter_map(|p| pairing_rating(p)).collect();
    if rated.len() != members.len() {
        return None;
    }
    mean(&rated)
}

/// Integer mean, rounded to nearest; `None` for an empty slice.
///
/// Integer because a team's average is only ever compared against other averages
/// and against MacMahon thresholds, both integers — and the same rounding serves
/// the average of the members' *live estimates* when estimate-based MacMahon is
/// on, which is the same quantity computed over a different rating.
pub(crate) fn mean(values: &[u32]) -> Option<u32> {
    if values.is_empty() {
        return None;
    }
    let sum: u64 = values.iter().map(|&v| u64::from(v)).sum();
    Some(((sum + values.len() as u64 / 2) / values.len() as u64) as u32)
}

/// The team-roster operations on a tournament. They all reject outside team
/// mode, and all of them are registration-time only: rosters and board order are
/// frozen at finalization, so a later rating edit can never reshuffle who plays
/// board 1.
impl Tournament {
    /// Error unless this is a team tournament whose registration is still open —
    /// the guard every roster mutation starts with.
    fn team_registration_open(&self) -> Result<(), TournamentError> {
        if !self.settings.team_mode() {
            return Err(TournamentError::NotATeamTournament);
        }
        if self.registration_finalized {
            return Err(TournamentError::RegistrationAlreadyFinalized);
        }
        Ok(())
    }

    pub fn team_of(&self, player: Uuid) -> Option<&Team> {
        self.teams.iter().find(|t| t.members.contains(&player))
    }

    /// The players in no team at all — the "unassigned pool" the Players tab
    /// shows, and what finalization requires to be empty.
    pub fn unassigned_players(&self) -> Vec<Uuid> {
        self.players
            .iter()
            .map(|p| p.id)
            .filter(|&id| self.team_of(id).is_none())
            .collect()
    }

    /// A team's members as players, in board order. Empty for an unknown team.
    /// Skips a member whose player was removed — which registration can't leave
    /// behind (removing a player clears their membership), so this is only a
    /// defence against a hand-edited save.
    pub fn team_members(&self, team: Uuid) -> Vec<&Player> {
        let Some(team) = self.teams.iter().find(|t| t.id == team) else {
            return Vec::new();
        };
        team.members
            .iter()
            .filter_map(|id| self.players.iter().find(|p| p.id == *id))
            .collect()
    }

    /// A team's average pairing rating — what its `TeamId`, its fold order and
    /// its MacMahon starting points are computed from. `None` when no member is
    /// rated (see [`average_pairing_rating`]).
    pub fn team_average_rating(&self, team: Uuid) -> Option<u32> {
        average_pairing_rating(&self.team_members(team))
    }

    /// Register a new team under `name`, and return it.
    ///
    /// The name must be non-empty and not collide with an existing team's,
    /// ignoring case — a referee reading a pairing sheet has only the name to go
    /// on, so two teams may not share one.
    pub fn add_team(&mut self, name: &str) -> Result<&Team, TournamentError> {
        self.team_registration_open()?;
        let name = name.trim();
        if name.is_empty() {
            return Err(TournamentError::EmptyTeamName);
        }
        if self.teams.iter().any(|t| Team::same_name(&t.name, name)) {
            return Err(TournamentError::DuplicateTeamName(name.to_string()));
        }
        self.teams.push(Team::new(name.to_string()));
        Ok(self.teams.last().expect("just pushed a team"))
    }

    /// Rename a team, under the same non-empty / unique rules as creation.
    pub fn rename_team(&mut self, team: Uuid, name: &str) -> Result<&Team, TournamentError> {
        self.team_registration_open()?;
        let name = name.trim();
        if name.is_empty() {
            return Err(TournamentError::EmptyTeamName);
        }
        if self
            .teams
            .iter()
            .any(|t| t.id != team && Team::same_name(&t.name, name))
        {
            return Err(TournamentError::DuplicateTeamName(name.to_string()));
        }
        let t = self.team_mut(team)?;
        t.name = name.to_string();
        Ok(t)
    }

    /// Delete a team.
    ///
    /// Allowed for as long as the team has never been paired into a match —
    /// the team-mode reading of the rule on players, since here the team is the
    /// pairing unit. Once it has played, every opponent's score record
    /// references it, so it is refused with
    /// [`TournamentError::CannotRemoveMatchedTeam`] exactly as a player who has
    /// played is.
    ///
    /// What happens to the members depends on where the tournament is. Before
    /// registration is finalized they go back to the unassigned pool, since
    /// being in a team and being registered are separate facts. After it there
    /// *is* no unassigned pool — [`finalize_teams`](Self::finalize_teams)
    /// requires it to be empty, and every roster to be exactly `team_size` —
    /// so they leave the tournament with their team.
    pub fn remove_team(&mut self, team: Uuid) -> Result<(), TournamentError> {
        if !self.settings.team_mode() {
            return Err(TournamentError::NotATeamTournament);
        }
        let Some(t) = self.teams.iter().find(|t| t.id == team) else {
            return Err(TournamentError::TeamNotFound(team));
        };
        let number = t.tournament_id;
        let members = t.members.clone();
        // "Has been matched" reads off the boards, the same way `has_played`
        // does for a player: in team mode a board only ever exists because two
        // teams were paired. A team that merely byed has sit-outs and no board,
        // and is still removable — which is the case this is for.
        let member_tids: Vec<TournamentId> = members
            .iter()
            .filter_map(|m| self.players.iter().find(|p| p.id == *m))
            .filter_map(|p| p.tournament_id)
            .collect();
        let has_played = self
            .rounds
            .iter()
            .flat_map(|r| &r.boards)
            .any(|b| member_tids.contains(&b.player1) || member_tids.contains(&b.player2));
        if has_played {
            return Err(TournamentError::CannotRemoveMatchedTeam);
        }

        self.teams.retain(|t| t.id != team);
        // The open draft names teams by number, and confirming one that pairs a
        // team nobody answers to would fault the same way a stale player entry
        // does (see `remove_player`).
        if let (Some(n), Some(draft)) = (number, &mut self.draft) {
            draft
                .forced_matches
                .retain(|m| m.team1 != n && m.team2 != n);
            draft.forced_team_byes.retain(|&b| b != n);
        }
        if self.registration_finalized {
            for m in members {
                self.remove_player_inner(m)?;
            }
        }
        Ok(())
    }

    /// Add a registered player to a team, at the board their pairing rating
    /// calls for.
    ///
    /// The newcomer goes in front of the first member weaker than them, so a
    /// roster built one player at a time comes out in board order without the
    /// referee ever pressing "sort by rating"; an unrated newcomer lands last,
    /// as unrated sorts everywhere else. It is an *insertion*, not a re-sort:
    /// nothing already on the roster moves relative to anything else, so a
    /// hand-picked order survives a late arrival (and the sort button is still
    /// there to reset one).
    ///
    /// A player belongs to exactly one team, so this rejects a player already in
    /// another (call [`remove_team_member`](Self::remove_team_member) first) and a
    /// team already at its full size. Adding a member the team already has is a
    /// no-op rather than a duplicate.
    pub fn add_team_member(&mut self, team: Uuid, player: Uuid) -> Result<&Team, TournamentError> {
        self.team_registration_open()?;
        let Some(added) = self.players.iter().find(|p| p.id == player) else {
            return Err(TournamentError::PlayerNotFound(player));
        };
        let rating = pairing_rating(added);
        if let Some(other) = self.team_of(player) {
            if other.id != team {
                return Err(TournamentError::PlayerAlreadyInATeam(player));
            }
        }
        let size = self.settings.team_size();
        let roster = self.team_members(team);
        // The first board held by someone the newcomer outranks; past the end
        // when they outrank nobody. Equal ratings keep arrival order, matching
        // the stable sort in `sort_team_by_rating`.
        let at = roster
            .iter()
            .position(|m| by_pairing_rating(pairing_rating(m), rating) == Ordering::Greater)
            .unwrap_or(roster.len());
        let t = self.team_mut(team)?;
        if t.members.contains(&player) {
            return Ok(t);
        }
        if t.members.len() as u32 >= size {
            return Err(TournamentError::TeamIsFull { size });
        }
        // `at` indexes the roster read above, which is this same list minus any
        // member whose player is gone. The two agree as long as every member has
        // a player record; if one ever does not, `at` is an index into a shorter
        // list, and `insert` seats the newcomer a board too early rather than
        // failing — the roster/`players` mismatch has to be caught on load, not
        // here.
        t.members.insert(at, player);
        Ok(t)
    }

    /// Take a player out of a team, back into the unassigned pool.
    pub fn remove_team_member(
        &mut self,
        team: Uuid,
        player: Uuid,
    ) -> Result<&Team, TournamentError> {
        self.team_registration_open()?;
        let t = self.team_mut(team)?;
        let before = t.members.len();
        t.members.retain(|&m| m != player);
        if t.members.len() == before {
            return Err(TournamentError::NotATeamMember { team, player });
        }
        Ok(t)
    }

    /// Set a team's board order (index 0 = board 1).
    ///
    /// `order` must be a permutation of the team's current members — naming a
    /// stranger, dropping a member or repeating one is rejected, so a reorder can
    /// never quietly change *who* is on the team.
    pub fn set_team_board_order(
        &mut self,
        team: Uuid,
        order: Vec<Uuid>,
    ) -> Result<&Team, TournamentError> {
        self.team_registration_open()?;
        let t = self.team_mut(team)?;
        let same_set = order.len() == t.members.len()
            && order.iter().all(|id| t.members.contains(id))
            && order
                .iter()
                .enumerate()
                .all(|(i, id)| !order[..i].contains(id));
        if !same_set {
            return Err(TournamentError::InvalidBoardOrder);
        }
        t.members = order;
        Ok(t)
    }

    /// Sort a team's board order by descending pairing rating — the default
    /// board order, and the "reset" a referee reaches for after editing ratings.
    /// Unrated members sort last, in their current relative order.
    pub fn sort_team_by_rating(&mut self, team: Uuid) -> Result<&Team, TournamentError> {
        self.team_registration_open()?;
        let ranked: Vec<(Uuid, Option<u32>)> = self
            .team_members(team)
            .iter()
            .map(|p| (p.id, pairing_rating(p)))
            .collect();
        let mut order = ranked;
        // `sort_by` is stable, so equally-rated members — and the unrated ones,
        // all equal to each other — keep their relative order.
        order.sort_by(|a, b| by_pairing_rating(a.1, b.1));
        let order: Vec<Uuid> = order.into_iter().map(|(id, _)| id).collect();
        let t = self.team_mut(team)?;
        t.members = order;
        Ok(t)
    }

    /// Set (or clear) a player's referee-assigned **pairing ELO**.
    ///
    /// Only meaningful in team mode with MacMahon starting points in use, so it
    /// is rejected anywhere else rather than quietly stored where nothing would
    /// ever read it. Clearing (`None`) is always allowed, so turning MacMahon off
    /// doesn't strand a value the referee can no longer edit.
    pub fn set_pairing_rating(
        &mut self,
        player: Uuid,
        rating: Option<u32>,
    ) -> Result<&Player, TournamentError> {
        if rating.is_some() && !(self.settings.team_mode() && self.settings.macmahon_in_use()) {
            return Err(TournamentError::PairingRatingNotApplicable);
        }
        let p = self
            .players
            .iter_mut()
            .find(|p| p.id == player)
            .ok_or(TournamentError::PlayerNotFound(player))?;
        p.pairing_rating = rating;
        // A pairing ELO feeds the MacMahon starting points every round's model
        // was built on, and it stays editable after finalization.
        self.invalidate_all_explanations();
        self.players
            .iter()
            .find(|p| p.id == player)
            .ok_or(TournamentError::PlayerNotFound(player))
    }

    /// The board↔team lookup for this tournament's frozen rosters — what every
    /// team-level derivation (matches, scores, pairing) starts from.
    pub(crate) fn team_slots(&self) -> TeamSlots {
        TeamSlots::new(&self.teams, &self.players)
    }

    /// The pairing engine's input in team mode: one unit per team, replayed from
    /// `completed`. The team twin of [`Tournament::pairing_units`].
    pub(crate) fn team_pairing_units(
        &self,
        completed: &[Round],
        slots: &TeamSlots,
    ) -> TiVec<UnitKey, PairingUnit> {
        team_units(&self.teams, &self.players, &self.settings, completed, slots)
    }

    /// The team matches of every round, scored, indexed like [`Self::rounds`] —
    /// empty outside team mode.
    ///
    /// Derived here and shipped to clients rather than re-derived by each of
    /// them: which boards make up a match, what order they are in, and who is
    /// credited each of them follow from the rosters and the scoring rules (the
    /// Wiel rule included), and the round view showing a different grouping or a
    /// different score from the standings would be a disagreement no screen
    /// could settle.
    pub fn team_matches(&self) -> Vec<Vec<TeamMatchView>> {
        if !self.settings.team_mode() {
            return Vec::new();
        }
        let slots = self.team_slots();
        let wiel_rule = self.settings.handicap_wiel_rule();
        self.rounds
            .iter()
            .map(|round| team_match_views(round, &slots, wiel_rule))
            .collect()
    }

    /// Confirm a team round: pair the present *teams*, then expand each matched
    /// pair into its `size` boards (`roster[k]` against `roster[k]`) and each bye
    /// into one sit-out per member.
    ///
    /// The team path is much shorter than the individual one because team mode
    /// rejects both features that complicate it: there is no cup bracket to
    /// splice in and no long board to carry over.
    pub(crate) fn confirm_team_round(&mut self) -> Result<&Round, TournamentError> {
        let draft = self.draft.clone().ok_or(TournamentError::NoDraft)?;
        let slots = self.team_slots();
        // A player is absent at most once. The `absent` set below folds repeats
        // away for the *membership* tests, but the sit-out rows are built from
        // the list itself, so a repeat there would write one row per entry and
        // pay the absence once per row.
        if let Some(player) = crate::tournament::first_repeat(draft.absent.iter().copied()) {
            return Err(TournamentError::PlayerTwiceInRound {
                round: draft.number,
                player,
            });
        }
        let absent: HashSet<TournamentId> = draft.absent.iter().copied().collect();

        // Player-level forcing has no meaning when teams are what get paired.
        if !draft.forced_boards.is_empty() || !draft.forced_byes.is_empty() {
            return Err(TournamentError::PlayerLevelDraftInTeamMode);
        }

        let present: Vec<UnitKey> = self
            .teams
            .iter()
            .filter_map(|t| t.tournament_id)
            .filter(|&number| {
                let members = slots.members_of(number);
                !members.is_empty() && !members.iter().all(|t| absent.contains(t))
            })
            .map(UnitKey::from)
            .collect();
        if present.len() < MIN_TEAMS_PER_ROUND {
            return Err(TournamentError::NotEnoughPresentTeams {
                needed: MIN_TEAMS_PER_ROUND,
                have: present.len(),
            });
        }

        // The referee's fixed matches and byes: every team named must be present,
        // and no team may be spoken for twice — a team cannot play two matches,
        // nor play and bye. Checked before anything is built, so a bad draft
        // names its own problem instead of producing a nonsense round.
        let present_set: HashSet<UnitKey> = present.iter().copied().collect();
        let mut placed: HashSet<UnitKey> = HashSet::new();
        let mut forced_pairs: Vec<(UnitKey, UnitKey)> = Vec::new();
        for m in &draft.forced_matches {
            if m.team1 == m.team2 {
                return Err(TournamentError::InvalidDraft(
                    "a team cannot be forced to play itself".into(),
                ));
            }
            for team in [m.team1, m.team2] {
                let key = UnitKey::from(team);
                if !present_set.contains(&key) {
                    return Err(TournamentError::InvalidDraft(format!(
                        "forced match names team {team}, which is absent this round"
                    )));
                }
                if !placed.insert(key) {
                    return Err(TournamentError::InvalidDraft(format!(
                        "team {team} is in two forced matches, or also forced onto a bye"
                    )));
                }
            }
            forced_pairs.push((UnitKey::from(m.team1), UnitKey::from(m.team2)));
        }
        let mut forced_bye_keys: Vec<UnitKey> = Vec::new();
        for &team in &draft.forced_team_byes {
            let key = UnitKey::from(team);
            if !present_set.contains(&key) {
                return Err(TournamentError::InvalidDraft(format!(
                    "forced bye names team {team}, which is absent this round"
                )));
            }
            if !placed.insert(key) {
                return Err(TournamentError::InvalidDraft(format!(
                    "team {team} is forced twice, or also in a forced match"
                )));
            }
            forced_bye_keys.push(key);
        }
        // No parity check: whatever the forced byes leave over, the engine byes
        // one more team if the count is odd — exactly as in individual mode.

        let units = self.team_pairing_units(&self.rounds, &slots);
        let paired = pair_round_weighted(
            draft.number,
            &self.settings,
            &units,
            &present,
            &forced_pairs,
            &forced_bye_keys,
        );

        // Freeze the explanation against the very `units` the engine was handed —
        // team units here, so the ledgers name team numbers. See
        // [`Round::explanation`].
        let explanation = explain_pairing(
            draft.number,
            &self.settings,
            &units,
            &paired
                .pairs
                .iter()
                .filter(|p| matches!(p.source, PairingSource::Swiss))
                .map(|p| (p.a, p.b))
                .collect::<Vec<_>>(),
            paired.swiss_bye,
        );

        // Each matched pair becomes `size` boards, all carrying the team-level
        // float: it is a fact of the *team* pairing, and the float history
        // replays from it.
        let mut boards: Vec<Board> = Vec::new();
        for p in &paired.pairs {
            boards.extend(match_boards(
                TeamId::from(p.a),
                TeamId::from(p.b),
                p.points_diff,
                p.source,
                &slots,
            ));
        }
        // A member marked absent from a team that still plays does not stop the
        // match: their board exists, and it is created already forfeited with a
        // *justified* absence. That is how a player who falls ill or leaves is
        // recorded — the opponent wins the board, and the `0-` cell says why,
        // without the unjustified-no-show stigma of a `0#`.
        for board in &mut boards {
            let missing = |side: Winner| {
                let player = match side {
                    Winner::Player1 => board.player1,
                    Winner::Player2 => board.player2,
                };
                absent.contains(&player).then_some(AbsenceKind::Justified)
            };
            if let Some(forfeit) = Forfeit::of(missing(Winner::Player1), missing(Winner::Player2)) {
                board.set_outcome(Outcome::Forfeit { absent: forfeit });
            }
        }
        // Display order: by team match (best-ranked team first), then board
        // number within the match — which the expansion above already produced,
        // since the engine hands back pairs in a stable order.

        // A team bye is one sit-out per member, so per-player scoring and the
        // grid see ordinary `0+` cells; at team level it derives as a match win.
        // The referee's byes and the engine's own differ only in their kind —
        // and the kind is what bye-repeat avoidance and re-pairing read.
        let mut sitouts: Vec<Sitout> = Vec::new();
        for (key, kind) in forced_bye_keys
            .iter()
            .map(|&k| (k, SitoutKind::ForcedBye))
            .chain(paired.swiss_bye.map(|k| (k, SitoutKind::Bye)))
        {
            sitouts.extend(
                slots
                    .members_of(TeamId::from(key))
                    .iter()
                    .map(|&player| Sitout {
                        player,
                        kind,
                        value: SitoutValue::Full,
                    }),
            );
        }
        let absent_value = self.absent_sitout_value();
        let playing: HashSet<TournamentId> =
            boards.iter().flat_map(|b| [b.player1, b.player2]).collect();
        let already: HashSet<TournamentId> = sitouts.iter().map(|s| s.player).collect();
        sitouts.extend(
            draft
                .absent
                .iter()
                .filter(|id| !playing.contains(id) && !already.contains(id))
                .map(|&player| Sitout {
                    player,
                    kind: SitoutKind::Absent,
                    value: absent_value,
                }),
        );

        self.rounds.push(Round {
            number: draft.number,
            boards,
            sitouts,
            completed: false,
            explanation,
            // Frozen a few lines ago from the model that paired this very round,
            // so there is nothing it could already be out of date with.
            pairing_explanation_valid: true,
        });
        self.draft = None;
        // Same invariant as the individual path, checked for the same reason —
        // here the partition is "every member of a paired team is on a board,
        // every member of a byed team has a sit-out", and an absent player of a
        // playing team must get the forfeited board rather than a sit-out on top
        // of it. Team mode refuses late registration outright, so here the full
        // rule is not merely assertable but permanently true.
        // See `Tournament::validate_round_membership`.
        #[cfg(debug_assertions)]
        if let Err(e) = self.validate_round_membership() {
            panic!(
                "confirming round {} broke the round records: {e}",
                draft.number
            );
        }
        Ok(self.rounds.last().expect("just pushed a round"))
    }

    /// Apply a manual point bonus (positive `delta`) or malus (negative) to a
    /// team, with a mandatory reason.
    ///
    /// Unlike the roster operations this stays available after finalization —
    /// that is when a referee actually needs it — and takes effect immediately,
    /// on both the standings and the score-gap weight of every later pairing.
    pub fn add_team_adjustment(
        &mut self,
        team: Uuid,
        delta: i32,
        reason: String,
    ) -> Result<&Team, TournamentError> {
        if !self.settings.team_mode() {
            return Err(TournamentError::NotATeamTournament);
        }
        let reason = reason.trim().to_string();
        if reason.is_empty() {
            return Err(TournamentError::EmptyAdjustmentReason);
        }
        if delta == 0 {
            return Err(TournamentError::ZeroPointAdjustment);
        }
        self.team_mut(team)?.adjustments.push(PointAdjustment {
            id: Uuid::new_v4(),
            delta,
            reason,
        });
        // The team's points are what every round was paired on, and an adjustment
        // carries no round of its own, so it lands under all of them.
        self.invalidate_all_explanations();
        self.team_mut(team).map(|t| &*t)
    }

    pub fn remove_team_adjustment(
        &mut self,
        team: Uuid,
        adjustment: Uuid,
    ) -> Result<&Team, TournamentError> {
        if !self.settings.team_mode() {
            return Err(TournamentError::NotATeamTournament);
        }
        let t = self.team_mut(team)?;
        let before = t.adjustments.len();
        t.adjustments.retain(|a| a.id != adjustment);
        if t.adjustments.len() == before {
            return Err(TournamentError::AdjustmentNotFound {
                player: team,
                adjustment,
            });
        }
        self.invalidate_all_explanations();
        self.team_mut(team).map(|t| &*t)
    }

    fn team_mut(&mut self, team: Uuid) -> Result<&mut Team, TournamentError> {
        self.teams
            .iter_mut()
            .find(|t| t.id == team)
            .ok_or(TournamentError::TeamNotFound(team))
    }

    /// Validate the rosters and assign every team its [`TeamId`] — the team half
    /// of finalization, run before players are numbered.
    ///
    /// Every check is loud and specific, because each has a different fix:
    /// at least two teams, no player left unassigned, every roster exactly
    /// `size`, and — with MacMahon starting points in use — a pairing rating for
    /// every member, since an unrated one would contribute nothing to the average
    /// the thresholds are read against.
    ///
    /// Numbers go by descending average pairing rating (ties by creation order),
    /// mirroring player numbering, and the rosters are frozen from here on.
    pub(crate) fn finalize_teams(&mut self) -> Result<(), TournamentError> {
        if self.teams.len() < 2 {
            return Err(TournamentError::NotEnoughTeams {
                have: self.teams.len(),
            });
        }
        let unassigned = self.unassigned_players();
        if !unassigned.is_empty() {
            return Err(TournamentError::PlayersWithoutTeam {
                count: unassigned.len(),
            });
        }
        let size = self.settings.team_size();
        if let Some(t) = self.teams.iter().find(|t| t.members.len() as u32 != size) {
            return Err(TournamentError::IncompleteTeam {
                name: t.name.clone(),
                have: t.members.len(),
                need: size,
            });
        }
        if self.settings.macmahon_in_use() {
            let missing = self
                .players
                .iter()
                .filter(|p| pairing_rating(p).is_none())
                .count();
            if missing > 0 {
                return Err(TournamentError::MembersWithoutPairingRating { count: missing });
            }
        }

        // Number by descending team average, ties by creation order — the same
        // rule as player numbering, so the two orderings read alike.
        let mut ranked: Vec<(usize, Option<u32>)> = self
            .teams
            .iter()
            .enumerate()
            .map(|(i, t)| (i, self.team_average_rating(t.id)))
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (number, (index, _)) in ranked.into_iter().enumerate() {
            self.teams[index].tournament_id = Some(TeamId(number as u32 + 1));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::NewPlayer;
    use crate::round::{AbsenceKind, ForcedMatch, Winner};

    fn player(rating: Option<u32>, pairing: Option<u32>) -> Player {
        let mut p = Player::from_new(NewPlayer {
            last_name: "X".into(),
            rating,
            ..Default::default()
        });
        p.pairing_rating = pairing;
        p
    }

    #[test]
    fn the_pairing_rating_prefers_the_real_one() {
        assert_eq!(pairing_rating(&player(Some(1800), Some(1200))), Some(1800));
        assert_eq!(pairing_rating(&player(None, Some(1200))), Some(1200));
        assert_eq!(pairing_rating(&player(None, None)), None);
    }

    #[test]
    fn the_team_average_needs_every_member_and_rounds_to_nearest() {
        let a = player(Some(2000), None);
        let b = player(Some(1801), None);
        // (2000 + 1801) / 2 = 1900.5 → 1901.
        assert_eq!(average_pairing_rating(&[&a, &b]), Some(1901));
        // A referee-assigned pairing rating counts like a real one.
        let d = player(None, Some(1600));
        assert_eq!(average_pairing_rating(&[&a, &d]), Some(1800));
        // One unrated member and there is no team average: a mean over the rest
        // would describe part of the team as if it were the whole.
        let c = player(None, None);
        assert_eq!(average_pairing_rating(&[&a, &b, &c]), None);
        assert_eq!(average_pairing_rating(&[&c]), None);
        assert_eq!(average_pairing_rating(&[]), None);
    }

    #[test]
    fn names_collide_ignoring_case_and_surrounding_space() {
        assert!(Team::same_name("Paris", " paris "));
        assert!(!Team::same_name("Paris", "Paris 2"));
    }

    // --- Rosters on a tournament -----------------------------------------

    use crate::settings::{MacMahonThreshold, TeamSettings, Tiebreak, TournamentSettings};
    use crate::units::HalfPoints;

    /// A team tournament of `size`, with `n` players rated 2000, 1990, … so the
    /// ordering assertions have something to bite on.
    fn team_tournament(size: u32, n: u32) -> (Tournament, Vec<Uuid>) {
        let mut t = Tournament::new("Teams").unwrap();
        t.update_settings(TournamentSettings {
            teams: Some(TeamSettings { size }),
            ..TournamentSettings::default()
        })
        .unwrap();
        let ids = (0..n)
            .map(|i| {
                t.add_player(NewPlayer {
                    last_name: format!("P{i}"),
                    rating: Some(2000 - i * 10),
                    ..Default::default()
                })
                .unwrap()
                .id
            })
            .collect();
        (t, ids)
    }

    fn fill_teams(t: &mut Tournament, ids: &[Uuid], teams: usize, size: usize) -> Vec<Uuid> {
        (0..teams)
            .map(|k| {
                let team = t.add_team(&format!("T{k}")).unwrap().id;
                for i in 0..size {
                    t.add_team_member(team, ids[k * size + i]).unwrap();
                }
                team
            })
            .collect()
    }

    /// In team mode the team is the pairing unit, so a player is not removable
    /// on their own once rosters are fixed: `finalize_teams` requires every team
    /// to be exactly `team_size`, and taking one member out would leave a roster
    /// that plays a short match.
    #[test]
    fn a_player_cannot_be_removed_individually_from_a_finalized_team_tournament() {
        let (mut t, ids) = team_tournament(2, 4);
        fill_teams(&mut t, &ids, 2, 2);
        // Before finalization it is ordinary registration churn, and the player
        // leaves their team on the way out.
        let mut before = t.clone();
        before.remove_player(ids[0]).unwrap();
        assert!(before.team_of(ids[0]).is_none());
        assert_eq!(before.players.len(), 3);

        t.finalize_registration().unwrap();
        assert_eq!(
            t.remove_player(ids[0]),
            Err(TournamentError::CannotRemoveTeamPlayer)
        );
        assert_eq!(t.players.len(), 4, "nobody left");
    }

    /// The team-level reading of "a player who has played cannot be removed".
    #[test]
    fn a_team_that_has_played_cannot_be_removed() {
        let (mut t, ids) = team_tournament(2, 4);
        let teams = fill_teams(&mut t, &ids, 2, 2);
        t.finalize_registration().unwrap();
        // Two teams, so round 1 pairs them against each other.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        assert!(!t.rounds[0].boards.is_empty());

        assert_eq!(
            t.remove_team(teams[0]),
            Err(TournamentError::CannotRemoveMatchedTeam)
        );
        assert_eq!(t.teams.len(), 2);
    }

    /// A team that only ever byed has sit-outs and no board, so it is still
    /// removable — and after finalization its members go with it, because
    /// finalization is what requires the unassigned pool to be empty.
    #[test]
    fn removing_a_team_after_finalization_takes_its_players() {
        // Three teams: one byes each round, since teams are what get paired.
        let (mut t, ids) = team_tournament(2, 6);
        let teams = fill_teams(&mut t, &ids, 3, 2);
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();

        // Whichever team byed played no board and can still go.
        let byed = teams
            .iter()
            .copied()
            .find(|&team| {
                let members: Vec<_> = t
                    .team_members(team)
                    .iter()
                    .filter_map(|p| p.tournament_id)
                    .collect();
                !t.rounds[0]
                    .boards
                    .iter()
                    .any(|b| members.contains(&b.player1) || members.contains(&b.player2))
            })
            .expect("an odd number of teams byes one of them");
        let gone: Vec<Uuid> = t.team_members(byed).iter().map(|p| p.id).collect();
        assert_eq!(gone.len(), 2);

        t.remove_team(byed).unwrap();

        assert_eq!(t.teams.len(), 2);
        assert!(
            gone.iter().all(|g| !t.players.iter().any(|p| p.id == *g)),
            "its members left with it — there is no pool to return them to"
        );
        // Their sit-outs went too, so nothing references a player who is gone.
        assert!(t.rounds[0].sitouts.is_empty());
        // And the tournament still scores.
        assert_eq!(t.team_standings().expect("team mode").len(), 2);
        assert_eq!(t.standings().len(), 4);
    }

    /// Before finalization the members stay registered: being in a team and
    /// being in the tournament are separate facts, and there is a pool to
    /// return them to.
    #[test]
    fn removing_a_team_before_finalization_only_unassigns_its_players() {
        let (mut t, ids) = team_tournament(2, 4);
        let teams = fill_teams(&mut t, &ids, 2, 2);

        t.remove_team(teams[0]).unwrap();

        assert_eq!(t.teams.len(), 1);
        assert_eq!(t.players.len(), 4, "everyone is still registered");
        assert!(t.team_of(ids[0]).is_none());
        assert!(t.unassigned_players().contains(&ids[0]));
    }

    #[test]
    fn team_operations_are_rejected_outside_team_mode() {
        let mut t = Tournament::new("Individual").unwrap();
        assert!(matches!(
            t.add_team("Paris"),
            Err(TournamentError::NotATeamTournament)
        ));
    }

    #[test]
    fn team_names_must_be_present_and_unique() {
        let (mut t, _) = team_tournament(3, 0);
        t.add_team("Paris").unwrap();
        assert!(matches!(
            t.add_team("  "),
            Err(TournamentError::EmptyTeamName)
        ));
        // Case-insensitively, and after trimming.
        assert!(matches!(
            t.add_team(" paris "),
            Err(TournamentError::DuplicateTeamName(_))
        ));
        // Renaming obeys the same rules, but a team may keep its own name.
        let lyon = t.add_team("Lyon").unwrap().id;
        assert!(matches!(
            t.rename_team(lyon, "PARIS"),
            Err(TournamentError::DuplicateTeamName(_))
        ));
        assert_eq!(t.rename_team(lyon, "Lyon 1").unwrap().name, "Lyon 1");
    }

    #[test]
    fn a_player_belongs_to_exactly_one_team() {
        let (mut t, ids) = team_tournament(2, 4);
        let a = t.add_team("A").unwrap().id;
        let b = t.add_team("B").unwrap().id;
        t.add_team_member(a, ids[0]).unwrap();
        assert!(matches!(
            t.add_team_member(b, ids[0]),
            Err(TournamentError::PlayerAlreadyInATeam(_))
        ));
        // Re-adding to the same team is a no-op, not a duplicate.
        assert_eq!(t.add_team_member(a, ids[0]).unwrap().members.len(), 1);
        // ...and a team stops at its configured size.
        t.add_team_member(a, ids[1]).unwrap();
        assert!(matches!(
            t.add_team_member(a, ids[2]),
            Err(TournamentError::TeamIsFull { size: 2 })
        ));
        // Leaving one frees the player for another.
        t.remove_team_member(a, ids[0]).unwrap();
        assert_eq!(t.team_of(ids[0]), None);
        t.add_team_member(b, ids[0]).unwrap();
        assert_eq!(t.team_of(ids[0]).unwrap().id, b);
    }

    #[test]
    fn removing_a_player_takes_them_out_of_their_team() {
        let (mut t, ids) = team_tournament(2, 2);
        let a = t.add_team("A").unwrap().id;
        t.add_team_member(a, ids[0]).unwrap();
        t.remove_player(ids[0]).unwrap();
        // No roster may reference a player who is no longer registered.
        assert!(t.teams[0].members.is_empty());
        assert!(t.team_of(ids[0]).is_none());
    }

    #[test]
    fn a_new_member_takes_the_board_their_rating_calls_for() {
        let (mut t, ids) = team_tournament(4, 4);
        let a = t.add_team("A").unwrap().id;
        // Registered in no particular order, as a referee would.
        for id in [ids[2], ids[0], ids[3], ids[1]] {
            t.add_team_member(a, id).unwrap();
        }
        // The roster is in board order without anyone sorting it.
        assert_eq!(t.teams[0].members, vec![ids[0], ids[1], ids[2], ids[3]]);
    }

    #[test]
    fn adding_a_member_leaves_the_boards_below_them_alone() {
        let (mut t, ids) = team_tournament(4, 3);
        let unrated = t
            .add_player(NewPlayer {
                last_name: "U".to_string(),
                ..Default::default()
            })
            .unwrap()
            .id;
        let a = t.add_team("A").unwrap().id;
        t.add_team_member(a, ids[0]).unwrap();
        t.add_team_member(a, ids[2]).unwrap();
        // A board order the referee picked themselves: the weaker player first.
        t.set_team_board_order(a, vec![ids[2], ids[0]]).unwrap();
        // P1 (1990) outranks whoever is on board 1 (1980) and takes it; the two
        // below keep the order they were put in rather than being re-sorted.
        t.add_team_member(a, ids[1]).unwrap();
        assert_eq!(t.teams[0].members, vec![ids[1], ids[2], ids[0]]);
        // An unrated newcomer sorts last, whatever is above them.
        t.add_team_member(a, unrated).unwrap();
        assert_eq!(t.teams[0].members.last(), Some(&unrated));
    }

    #[test]
    fn a_board_order_must_be_a_permutation_of_the_roster() {
        let (mut t, ids) = team_tournament(3, 4);
        let a = t.add_team("A").unwrap().id;
        for id in &ids[..3] {
            t.add_team_member(a, *id).unwrap();
        }
        // A stranger, a dropped member and a repeat are all rejected, so a
        // reorder can never quietly change who is on the team.
        for bad in [
            vec![ids[0], ids[1], ids[3]],
            vec![ids[0], ids[1]],
            vec![ids[0], ids[1], ids[0]],
        ] {
            assert!(matches!(
                t.set_team_board_order(a, bad),
                Err(TournamentError::InvalidBoardOrder)
            ));
        }
        let order = vec![ids[2], ids[0], ids[1]];
        assert_eq!(
            t.set_team_board_order(a, order.clone()).unwrap().members,
            order
        );
        // Sorting by rating restores the default board order (P0 is the strongest).
        assert_eq!(
            t.sort_team_by_rating(a).unwrap().members,
            vec![ids[0], ids[1], ids[2]]
        );
    }

    #[test]
    fn finalization_requires_full_rosters_and_at_least_two_teams() {
        let (mut t, ids) = team_tournament(2, 4);
        assert!(matches!(
            t.finalize_registration(),
            Err(TournamentError::NotEnoughTeams { have: 0 })
        ));
        let a = t.add_team("A").unwrap().id;
        let b = t.add_team("B").unwrap().id;
        t.add_team_member(a, ids[0]).unwrap();
        // Two players still unassigned.
        assert!(matches!(
            t.finalize_registration(),
            Err(TournamentError::PlayersWithoutTeam { count: 3 })
        ));
        t.add_team_member(b, ids[1]).unwrap();
        t.add_team_member(b, ids[2]).unwrap();
        t.add_team_member(a, ids[3]).unwrap();
        // Now everyone is assigned and both teams are full.
        t.finalize_registration().unwrap();
        assert!(t.registration_finalized);
    }

    #[test]
    fn finalization_rejects_a_roster_of_the_wrong_size() {
        let (mut t, ids) = team_tournament(3, 5);
        let a = t.add_team("A").unwrap().id;
        let b = t.add_team("B").unwrap().id;
        for id in &ids[..3] {
            t.add_team_member(a, *id).unwrap();
        }
        for id in &ids[3..] {
            t.add_team_member(b, *id).unwrap();
        }
        assert!(matches!(
            t.finalize_registration(),
            Err(TournamentError::IncompleteTeam {
                have: 2,
                need: 3,
                ..
            })
        ));
    }

    #[test]
    fn teams_are_numbered_by_descending_average_rating() {
        // Two teams of two: B is the stronger pair, so it takes number 1 even
        // though A was created first.
        let (mut t, ids) = team_tournament(2, 4);
        let a = t.add_team("A").unwrap().id;
        let b = t.add_team("B").unwrap().id;
        t.add_team_member(a, ids[2]).unwrap(); // 1980
        t.add_team_member(a, ids[3]).unwrap(); // 1970
        t.add_team_member(b, ids[0]).unwrap(); // 2000
        t.add_team_member(b, ids[1]).unwrap(); // 1990
        assert_eq!(t.team_average_rating(a), Some(1975));
        assert_eq!(t.team_average_rating(b), Some(1995));
        t.finalize_registration().unwrap();
        let number = |id: Uuid| t.teams.iter().find(|x| x.id == id).unwrap().tournament_id;
        assert_eq!(number(b), Some(TeamId(1)));
        assert_eq!(number(a), Some(TeamId(2)));
    }

    #[test]
    fn macmahon_requires_a_pairing_rating_for_every_unrated_member() {
        let mut t = Tournament::new("Teams").unwrap();
        t.update_settings(
            TournamentSettings {
                teams: Some(TeamSettings { size: 2 }),
                ..TournamentSettings::default()
            }
            .with_thresholds(vec![MacMahonThreshold::elo(1500)]),
        )
        .unwrap();
        let ids: Vec<Uuid> = (0..4)
            .map(|i| {
                t.add_player(NewPlayer {
                    last_name: format!("P{i}"),
                    // The last player is unrated.
                    rating: (i < 3).then_some(1600),
                    ..Default::default()
                })
                .unwrap()
                .id
            })
            .collect();
        fill_teams(&mut t, &ids, 2, 2);
        assert!(matches!(
            t.finalize_registration(),
            Err(TournamentError::MembersWithoutPairingRating { count: 1 })
        ));
        // A referee-assigned pairing ELO satisfies it — and feeds the average
        // without ever becoming a real rating.
        t.set_pairing_rating(ids[3], Some(1400)).unwrap();
        t.finalize_registration().unwrap();
        assert_eq!(
            t.players.iter().find(|p| p.id == ids[3]).unwrap().rating,
            None
        );
    }

    /// A finalized two-by-two team tournament that loads cleanly, as the base
    /// for the corruption cases below.
    fn finalized_pair() -> Tournament {
        let (mut t, ids) = team_tournament(2, 4);
        fill_teams(&mut t, &ids, 2, 2);
        t.finalize_registration().unwrap();
        assert_eq!(t.validate_loaded(), Ok(()));
        t
    }

    /// A team save's referential integrity. None of these is reachable through
    /// this program — rosters are built through `add_team_member` and frozen at
    /// finalization — but a file can express every one of them, and each is
    /// load-bearing on the read path: the slots table drops an unregistered
    /// member (the team then plays a board short and its average rating is over
    /// the rest), the pairing engine panics on rosters of different sizes, and
    /// two teams on one number share a row of every score table.
    #[test]
    fn validate_loaded_rejects_a_corrupt_team_roster() {
        // A member nobody registered.
        let mut t = finalized_pair();
        let ghost = Uuid::new_v4();
        t.teams[0].members[1] = ghost;
        assert_eq!(
            t.validate_loaded(),
            Err(TournamentError::UnknownTeamMember {
                team: t.teams[0].id,
                player: ghost,
            })
        );

        // The same player on two rosters: they would play twice in one round.
        let mut t = finalized_pair();
        let shared = t.teams[0].members[0];
        t.teams[1].members[0] = shared;
        assert_eq!(
            t.validate_loaded(),
            Err(TournamentError::PlayerAlreadyInATeam(shared))
        );

        // A roster short of the configured size — the shape that panics the
        // pairing engine with "different numbers of boards".
        let mut t = finalized_pair();
        t.teams[0].members.pop();
        assert!(matches!(
            t.validate_loaded(),
            Err(TournamentError::IncompleteTeam {
                have: 1,
                need: 2,
                ..
            })
        ));
    }

    #[test]
    fn validate_loaded_rejects_duplicate_or_missing_team_identity() {
        // Two teams sharing a registration id.
        let mut t = finalized_pair();
        let id = t.teams[0].id;
        t.teams[1].id = id;
        assert_eq!(
            t.validate_loaded(),
            Err(TournamentError::DuplicateTeamId { team: id })
        );

        // Two teams sharing a team number: every score table is indexed by it.
        let mut t = finalized_pair();
        let number = t.teams[0].tournament_id.expect("finalized");
        t.teams[1].tournament_id = Some(number);
        assert_eq!(
            t.validate_loaded(),
            Err(TournamentError::DuplicateTeamNumber { number })
        );

        // Finalized but unnumbered, like an unnumbered player.
        let mut t = finalized_pair();
        let team = t.teams[1].id;
        t.teams[1].tournament_id = None;
        assert_eq!(
            t.validate_loaded(),
            Err(TournamentError::UnnumberedTeam { team })
        );

        // Before finalization, though, no team has a number and none is due.
        let (mut t, ids) = team_tournament(2, 4);
        fill_teams(&mut t, &ids, 2, 2);
        assert!(t.teams.iter().all(|x| x.tournament_id.is_none()));
        assert_eq!(t.validate_loaded(), Ok(()));
    }

    #[test]
    fn a_settings_change_never_silently_discards_hand_entered_pairing_elos() {
        let macmahon = |teams| {
            TournamentSettings {
                teams,
                ..TournamentSettings::default()
            }
            .with_thresholds(vec![MacMahonThreshold::elo(1500)])
        };
        let mut t = Tournament::new("Teams").unwrap();
        t.update_settings(macmahon(Some(TeamSettings { size: 2 })))
            .unwrap();
        let id = t
            .add_player(NewPlayer {
                last_name: "Unrated".into(),
                ..Default::default()
            })
            .unwrap()
            .id;
        t.set_pairing_rating(id, Some(1400)).unwrap();

        // Dropping MacMahon would strand the value. The client PUTs the whole
        // settings object for any one field, so wiping here would destroy it on
        // an edit that had nothing to do with pairing ELOs.
        assert!(matches!(
            t.update_settings(TournamentSettings {
                teams: Some(TeamSettings { size: 2 }),
                ..TournamentSettings::default()
            }),
            Err(TournamentError::PairingRatingsWouldBeDiscarded { count: 1 })
        ));
        assert_eq!(
            t.players
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .pairing_rating,
            Some(1400)
        );
        // An unrelated change under the same configuration is unaffected.
        let mut renamed = macmahon(Some(TeamSettings { size: 2 }));
        renamed.city = Some("Lyon".into());
        t.update_settings(renamed).unwrap();
        assert_eq!(
            t.players
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .pairing_rating,
            Some(1400)
        );
        // Clearing it deliberately is what unblocks the change.
        t.set_pairing_rating(id, None).unwrap();
        t.update_settings(TournamentSettings {
            teams: Some(TeamSettings { size: 2 }),
            ..TournamentSettings::default()
        })
        .unwrap();
    }

    #[test]
    fn a_pairing_rating_is_rejected_where_nothing_would_read_it() {
        // Plain Swiss team mode: no MacMahon starting points, so no fake ELO.
        let (mut t, ids) = team_tournament(2, 2);
        assert!(matches!(
            t.set_pairing_rating(ids[0], Some(1400)),
            Err(TournamentError::PairingRatingNotApplicable)
        ));
        // Clearing is always allowed, so a value can't be stranded.
        assert!(t.set_pairing_rating(ids[0], None).is_ok());
    }

    /// Neither side of a conflicting pair is auto-disabled: the update is
    /// rejected and the referee decides which one to drop.
    #[test]
    fn conflicting_settings_are_rejected_from_either_direction() {
        // Turning team mode on while the cup is enabled.
        let mut t = Tournament::new("Teams").unwrap();
        t.update_settings(TournamentSettings {
            cup_enabled: true,
            ..TournamentSettings::default()
        })
        .unwrap();
        assert!(matches!(
            t.update_settings(TournamentSettings {
                cup_enabled: true,
                teams: Some(TeamSettings { size: 3 }),
                ..TournamentSettings::default()
            }),
            Err(TournamentError::TeamModeConflict(_))
        ));
        // The cup is still on — nothing was silently switched off.
        assert!(t.settings.cup_enabled);

        // ...and the other way round: enabling the cup on a team tournament.
        let (mut t, _) = team_tournament(3, 0);
        assert!(matches!(
            t.update_settings(TournamentSettings {
                cup_enabled: true,
                teams: Some(TeamSettings { size: 3 }),
                ..TournamentSettings::default()
            }),
            Err(TournamentError::TeamModeConflict(_))
        ));
        assert!(t.settings.team_mode());
    }

    /// The estimated-ELO tie-break can't rank a team, and `normalized` drops a
    /// criterion that can't rank — so without checking the conflict *first*, the
    /// referee's setting would be quietly edited instead of refused.
    #[test]
    fn the_est_elo_conflict_is_reported_not_normalized_away() {
        let (mut t, _) = team_tournament(3, 0);
        assert!(matches!(
            t.update_settings(TournamentSettings {
                teams: Some(TeamSettings { size: 3 }),
                tiebreaks: vec![Tiebreak::Points, Tiebreak::EstElo],
                ..TournamentSettings::default()
            }),
            Err(TournamentError::TeamModeConflict(
                crate::settings::TeamModeConflict::EstEloTiebreak
            ))
        ));
    }

    /// A per-player bonus would move nothing a referee can see in a team
    /// tournament, so it is refused rather than stored where nothing reads it.
    #[test]
    fn player_point_adjustments_are_rejected_in_team_mode() {
        let (mut t, ids) = team_tournament(2, 4);
        assert!(matches!(
            t.add_point_adjustment(ids[0], 1, "fair play".into()),
            Err(TournamentError::PlayerAdjustmentInTeamMode)
        ));
    }

    #[test]
    fn the_team_size_is_validated_and_frozen_at_finalization() {
        let mut t = Tournament::new("Teams").unwrap();
        assert!(matches!(
            t.update_settings(TournamentSettings {
                teams: Some(TeamSettings { size: 1 }),
                ..TournamentSettings::default()
            }),
            Err(TournamentError::InvalidTeamSize { size: 1 })
        ));

        let (mut t, ids) = team_tournament(2, 4);
        fill_teams(&mut t, &ids, 2, 2);
        t.finalize_registration().unwrap();
        // Both the size and team mode itself are structural once rosters exist.
        assert!(matches!(
            t.update_settings(TournamentSettings {
                teams: Some(TeamSettings { size: 3 }),
                ..TournamentSettings::default()
            }),
            Err(TournamentError::TeamSettingsLocked)
        ));
        assert!(matches!(
            t.update_settings(TournamentSettings::default()),
            Err(TournamentError::TeamSettingsLocked)
        ));
    }

    // --- Pairing ----------------------------------------------------------

    fn start_round(t: &mut Tournament, absent: Vec<TournamentId>) {
        t.prepare_round().unwrap();
        t.update_draft(absent, Vec::new(), Vec::new()).unwrap();
        t.confirm_round().unwrap();
    }

    fn tid(t: &Tournament, id: Uuid) -> TournamentId {
        t.players
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .tournament_id
            .unwrap()
    }

    #[test]
    fn a_team_match_becomes_one_board_per_position() {
        // Two teams of three: one match, three boards, board k against board k.
        let (mut t, ids) = team_tournament(3, 6);
        let teams = fill_teams(&mut t, &ids, 2, 3);
        t.finalize_registration().unwrap();
        start_round(&mut t, Vec::new());

        let round = &t.rounds[0];
        assert_eq!(round.boards.len(), 3);
        assert!(round.sitouts.is_empty());
        let slots = t.team_slots();
        for (k, board) in round.boards.iter().enumerate() {
            // Both sides sit on the same board number, in different teams.
            let (team1, b1) = slots.slot_of(board.player1).unwrap();
            let (team2, b2) = slots.slot_of(board.player2).unwrap();
            assert_eq!((b1, b2), (k as u32, k as u32));
            assert_ne!(team1, team2);
        }
        // One match, holding all three boards.
        let matches = t.team_matches().remove(0);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].boards, vec![0, 1, 2]);
        assert_eq!(t.team_members(teams[0]).len(), 3);
    }

    #[test]
    fn an_odd_field_gives_one_team_the_bye_as_member_sitouts() {
        // Three teams of two: one match plus a bye team, whose two members each
        // get an ordinary full-point sit-out.
        let (mut t, ids) = team_tournament(2, 6);
        fill_teams(&mut t, &ids, 3, 2);
        t.finalize_registration().unwrap();
        start_round(&mut t, Vec::new());

        let round = &t.rounds[0];
        assert_eq!(round.boards.len(), 2);
        assert_eq!(round.sitouts.len(), 2);
        assert!(round
            .sitouts
            .iter()
            .all(|s| s.kind == SitoutKind::Bye && s.value == SitoutValue::Full));
        // Both sit-outs belong to the same team — a team byes as a unit.
        let slots = t.team_slots();
        let teams: HashSet<TeamId> = round
            .sitouts
            .iter()
            .map(|s| slots.team_of(s.player).unwrap())
            .collect();
        assert_eq!(teams.len(), 1);
    }

    #[test]
    fn an_absent_team_is_left_out_of_the_pairing() {
        let (mut t, ids) = team_tournament(2, 6);
        fill_teams(&mut t, &ids, 3, 2);
        t.finalize_registration().unwrap();
        // Mark the whole third team absent: the other two play, nobody byes.
        let away: Vec<TournamentId> = ids[4..].iter().map(|&id| tid(&t, id)).collect();
        start_round(&mut t, away.clone());

        let round = &t.rounds[0];
        assert_eq!(round.boards.len(), 2);
        assert_eq!(round.sitouts.len(), 2);
        assert!(round.sitouts.iter().all(|s| s.kind == SitoutKind::Absent));
        assert!(round.sitouts.iter().all(|s| away.contains(&s.player)));
    }

    /// A half-absent team still plays: the missing member's board is created
    /// already forfeited, with a *justified* absence — the record for a player
    /// who fell ill or left, without the unjustified-no-show stigma.
    #[test]
    fn a_partly_absent_team_plays_with_its_missing_boards_forfeited() {
        let (mut t, ids) = team_tournament(2, 4);
        fill_teams(&mut t, &ids, 2, 2);
        t.finalize_registration().unwrap();
        let away = tid(&t, ids[0]);
        start_round(&mut t, vec![away]);

        let round = &t.rounds[0];
        // The team is paired as usual — both boards exist.
        assert_eq!(round.boards.len(), 2);
        // ...and the absent member's board is already decided against them,
        // which is what makes the round completable without further input.
        let board = round
            .boards
            .iter()
            .find(|b| b.player1 == away || b.player2 == away)
            .expect("the absent member still has a board");
        let side = if board.player1 == away {
            Winner::Player1
        } else {
            Winner::Player2
        };
        assert_eq!(board.absence_kind(side), Some(AbsenceKind::Justified));
        assert_eq!(board.absence_kind(other(side)), None);
        // The opponent takes the board, so the match is 1–1 rather than 2–0.
        assert_eq!(board.no_show_opponent(), Some(other_player(board, away)));
        // An absent player who has a board gets no sit-out — the board scores
        // them, exactly as for an absent cup player.
        assert!(round.sitout(away).is_none());
    }

    /// The other side of the coin: the *whole* team absent is still a team
    /// absence, so nobody is paired and every member gets a sit-out.
    #[test]
    fn a_wholly_absent_team_is_still_left_out() {
        let (mut t, ids) = team_tournament(2, 6);
        fill_teams(&mut t, &ids, 3, 2);
        t.finalize_registration().unwrap();
        let away: Vec<TournamentId> = ids[4..].iter().map(|&id| tid(&t, id)).collect();
        start_round(&mut t, away.clone());
        assert_eq!(t.rounds[0].boards.len(), 2);
        assert!(away.iter().all(|&p| t.rounds[0].sitout(p).is_some()));
    }

    /// The other side of a board.
    fn other(side: Winner) -> Winner {
        match side {
            Winner::Player1 => Winner::Player2,
            Winner::Player2 => Winner::Player1,
        }
    }

    /// The other player on a board.
    fn other_player(board: &Board, player: TournamentId) -> TournamentId {
        if board.player1 == player {
            board.player2
        } else {
            board.player1
        }
    }

    #[test]
    fn a_team_round_needs_two_present_teams() {
        let (mut t, ids) = team_tournament(2, 4);
        fill_teams(&mut t, &ids, 2, 2);
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        let away: Vec<TournamentId> = ids[2..].iter().map(|&id| tid(&t, id)).collect();
        t.update_draft(away, Vec::new(), Vec::new()).unwrap();
        assert!(matches!(
            t.confirm_round(),
            Err(TournamentError::NotEnoughPresentTeams { needed: 2, have: 1 })
        ));
    }

    /// A forced pairing names two players, which is not a thing a team round
    /// pairs — so it is refused as soon as the draft is submitted, not after
    /// the referee has confirmed a round built around it.
    #[test]
    fn player_level_forcing_is_rejected_in_team_mode() {
        let (mut t, ids) = team_tournament(2, 4);
        fill_teams(&mut t, &ids, 2, 2);
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        let (a, b) = (tid(&t, ids[0]), tid(&t, ids[2]));
        assert!(matches!(
            t.update_draft(
                Vec::new(),
                vec![Board::pending(a, b, 0, crate::round::PairingSource::Swiss)],
                Vec::new(),
            ),
            Err(TournamentError::PlayerLevelDraftInTeamMode)
        ));
        // ...and the team-level draft is refused on an individual tournament,
        // for the mirror-image reason.
        let mut individual = Tournament::new("Solo").unwrap();
        for name in ["A", "B"] {
            individual
                .add_player(NewPlayer {
                    last_name: name.into(),
                    ..Default::default()
                })
                .unwrap();
        }
        individual.finalize_registration().unwrap();
        individual.prepare_round().unwrap();
        assert!(matches!(
            individual.update_team_draft(Vec::new(), Vec::new(), Vec::new()),
            Err(TournamentError::NotATeamTournament)
        ));
    }

    // --- Adjustments -------------------------------------------------------

    /// A team bonus moves the team's points, which is the whole point of it
    /// being team-level: the ranking is by team, so a per-player delta (refused
    /// in team mode) would move nothing a referee can see.
    #[test]
    fn a_team_adjustment_moves_the_teams_points() {
        let (mut t, ids) = team_tournament(2, 4);
        let teams = fill_teams(&mut t, &ids, 2, 2);
        t.finalize_registration().unwrap();

        let before = t.team_standings().unwrap();
        assert!(before.iter().all(|s| s.points == 0));

        // Available *after* finalization — that is when a referee needs it.
        t.add_team_adjustment(teams[0], 1, "fair play".into())
            .unwrap();
        let after = t.team_standings().unwrap();
        let bonused = after.iter().find(|s| s.team_id == teams[0]).unwrap();
        assert_eq!(bonused.points, HalfPoints::from_whole(1));
        // The delta lands *in* the MacMahon start rather than beside it, so it
        // behaves as an ordinary MacMahon point everywhere — the standings, the
        // score-gap weight, and the airtight-groups rule alike — and the Results
        // tab's "Wins + MacMahon points" stays a sum that adds up.
        assert_eq!(bonused.macmahon, HalfPoints::from_whole(1));
        assert_eq!(bonused.points, bonused.macmahon + bonused.victories.into());
        // ...and it lifts the team above the others in the ranking.
        assert_eq!(after[0].team_id, teams[0]);

        // A malus cannot push a team below zero.
        t.add_team_adjustment(teams[0], -5, "penalty".into())
            .unwrap();
        let after = t.team_standings().unwrap();
        assert_eq!(
            after.iter().find(|s| s.team_id == teams[0]).unwrap().points,
            HalfPoints::ZERO
        );
    }

    #[test]
    fn a_team_adjustment_needs_a_reason_and_a_non_zero_delta() {
        let (mut t, ids) = team_tournament(2, 4);
        let teams = fill_teams(&mut t, &ids, 2, 2);
        assert!(matches!(
            t.add_team_adjustment(teams[0], 1, "  ".into()),
            Err(TournamentError::EmptyAdjustmentReason)
        ));
        assert!(matches!(
            t.add_team_adjustment(teams[0], 0, "nothing".into()),
            Err(TournamentError::ZeroPointAdjustment)
        ));
    }

    #[test]
    fn a_team_adjustment_can_be_taken_back() {
        let (mut t, ids) = team_tournament(2, 4);
        let teams = fill_teams(&mut t, &ids, 2, 2);
        let id = t
            .add_team_adjustment(teams[0], 2, "bonus".into())
            .unwrap()
            .adjustments
            .last()
            .unwrap()
            .id;
        t.remove_team_adjustment(teams[0], id).unwrap();
        assert!(t.teams[0].adjustments.is_empty());
        // Removing it twice is a named error, not a silent no-op.
        assert!(matches!(
            t.remove_team_adjustment(teams[0], id),
            Err(TournamentError::AdjustmentNotFound { .. })
        ));
    }

    #[test]
    fn team_adjustments_are_rejected_outside_team_mode() {
        let mut t = Tournament::new("Solo").unwrap();
        assert!(matches!(
            t.add_team_adjustment(Uuid::new_v4(), 1, "x".into()),
            Err(TournamentError::NotATeamTournament)
        ));
    }

    // --- Team-level draft operations --------------------------------------

    fn team_number(t: &Tournament, player: Uuid) -> TeamId {
        t.team_of(player).unwrap().tournament_id.unwrap()
    }

    #[test]
    fn a_forced_match_is_honoured_and_the_rest_paired_around_it() {
        // Four teams of two. Left alone, the engine folds 1-3 / 2-4 (all level
        // on points, so the fold decides); force 1 against 4 instead.
        let (mut t, ids) = team_tournament(2, 8);
        fill_teams(&mut t, &ids, 4, 2);
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        t.update_team_draft(
            Vec::new(),
            vec![ForcedMatch {
                team1: TeamId(1),
                team2: TeamId(4),
            }],
            Vec::new(),
        )
        .unwrap();
        t.confirm_round().unwrap();

        let round = t.rounds.last().unwrap().clone();
        let matches = t.team_matches().pop().unwrap();
        let pairs: HashSet<(TeamId, TeamId)> = matches
            .iter()
            .map(|m| (m.team1.min(m.team2), m.team1.max(m.team2)))
            .collect();
        assert!(pairs.contains(&(TeamId(1), TeamId(4))), "{pairs:?}");
        assert_eq!(pairs.len(), 2);
        // The forced match's boards say so, and the engine's don't.
        let forced: Vec<&TeamMatchView> = matches
            .iter()
            .filter(|m| round.boards[m.boards[0]].source == crate::round::PairingSource::Forced)
            .collect();
        assert_eq!(forced.len(), 1);
        assert_eq!(
            (
                forced[0].team1.min(forced[0].team2),
                forced[0].team1.max(forced[0].team2)
            ),
            (TeamId(1), TeamId(4))
        );
    }

    #[test]
    fn a_forced_team_bye_sits_that_team_out_as_a_unit() {
        // Five teams: one byes anyway, but the referee picks which.
        let (mut t, ids) = team_tournament(2, 10);
        fill_teams(&mut t, &ids, 5, 2);
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();
        t.update_team_draft(Vec::new(), Vec::new(), vec![TeamId(1)])
            .unwrap();
        t.confirm_round().unwrap();

        let round = t.rounds.last().unwrap();
        assert_eq!(round.sitouts.len(), 2, "one sit-out per member");
        let slots = t.team_slots();
        assert!(
            round
                .sitouts
                .iter()
                .all(|s| slots.team_of(s.player) == Some(TeamId(1))
                    && s.kind == SitoutKind::ForcedBye)
        );
        assert_eq!(round.boards.len(), 4, "the other four teams play");
    }

    #[test]
    fn a_forced_draft_that_cannot_hold_is_refused_naming_the_problem() {
        let (mut t, ids) = team_tournament(2, 8);
        fill_teams(&mut t, &ids, 4, 2);
        t.finalize_registration().unwrap();
        t.prepare_round().unwrap();

        // A team in two forced matches cannot play both.
        t.update_team_draft(
            Vec::new(),
            vec![
                ForcedMatch {
                    team1: TeamId(1),
                    team2: TeamId(2),
                },
                ForcedMatch {
                    team1: TeamId(1),
                    team2: TeamId(3),
                },
            ],
            Vec::new(),
        )
        .unwrap();
        assert!(matches!(
            t.confirm_round(),
            Err(TournamentError::InvalidDraft(_))
        ));

        // ...nor play and bye.
        t.update_team_draft(
            Vec::new(),
            vec![ForcedMatch {
                team1: TeamId(1),
                team2: TeamId(2),
            }],
            vec![TeamId(1)],
        )
        .unwrap();
        assert!(matches!(
            t.confirm_round(),
            Err(TournamentError::InvalidDraft(_))
        ));

        // ...nor play itself.
        t.update_team_draft(
            Vec::new(),
            vec![ForcedMatch {
                team1: TeamId(1),
                team2: TeamId(1),
            }],
            Vec::new(),
        )
        .unwrap();
        assert!(matches!(
            t.confirm_round(),
            Err(TournamentError::InvalidDraft(_))
        ));

        // ...and an absent team cannot be forced to do either.
        let away: Vec<TournamentId> = ids[..2].iter().map(|&id| tid(&t, id)).collect();
        let absent_team = team_number(&t, ids[0]);
        t.update_team_draft(
            away,
            vec![ForcedMatch {
                team1: absent_team,
                team2: TeamId(2),
            }],
            Vec::new(),
        )
        .unwrap();
        assert!(matches!(
            t.confirm_round(),
            Err(TournamentError::InvalidDraft(_))
        ));
    }

    /// The counterfactual, at team level: probing a match the engine did not
    /// make reports what it would cost, keyed by team number.
    #[test]
    fn the_counterfactual_probes_teams_in_team_mode() {
        let (mut t, ids) = team_tournament(2, 8);
        fill_teams(&mut t, &ids, 4, 2);
        t.finalize_registration().unwrap();
        start_round(&mut t, Vec::new());

        let paired: HashSet<(TeamId, TeamId)> = t.team_matches()[0]
            .iter()
            .map(|m| (m.team1.min(m.team2), m.team1.max(m.team2)))
            .collect();
        // Some pair the engine did *not* make.
        let (a, b) = (1..=4u32)
            .flat_map(|x| (x + 1..=4).map(move |y| (TeamId(x), TeamId(y))))
            .find(|pair| !paired.contains(pair))
            .expect("four teams cannot all be paired in one round");

        let cf = t
            .explain_counterfactual(
                1,
                UnitKey::from(a),
                UnitKey::from(b),
                crate::pairing::CounterfactualMode::Force,
            )
            .unwrap();
        assert!(cf.scoped_out.is_none());
        // Forcing it changes at least the two matches it disturbs.
        assert!(!cf.changed.is_empty());
        // ...and applying it really does pair them.
        t.force_pairing(UnitKey::from(a), UnitKey::from(b)).unwrap();
        let now: HashSet<(TeamId, TeamId)> = t
            .team_matches()
            .last()
            .unwrap()
            .iter()
            .map(|m| (m.team1.min(m.team2), m.team1.max(m.team2)))
            .collect();
        assert!(now.contains(&(a, b)), "{now:?}");
    }

    /// Forcing a team onto the bye is the same operation with the phantom on
    /// one side — the team analogue of forcing a player to sit out.
    #[test]
    fn forcing_a_team_onto_the_bye_re_pairs_around_it() {
        let (mut t, ids) = team_tournament(2, 10);
        fill_teams(&mut t, &ids, 5, 2);
        t.finalize_registration().unwrap();
        start_round(&mut t, Vec::new());

        let slots = t.team_slots();
        let byed = slots.team_of(t.rounds[0].sitouts[0].player).unwrap();
        let other_team = (1..=5u32).map(TeamId).find(|&x| x != byed).unwrap();

        t.force_pairing(UnitKey::from(other_team), UnitKey::PHANTOM)
            .unwrap();
        let round = t.rounds.last().unwrap();
        let slots = t.team_slots();
        assert!(round
            .sitouts
            .iter()
            .all(|s| slots.team_of(s.player) == Some(other_team)));
        assert_eq!(round.sitouts.len(), 2);
    }

    /// The whole point of pairing teams: two teams never meet twice, even though
    /// their players are what the boards name.
    #[test]
    fn teams_do_not_meet_twice() {
        let (mut t, ids) = team_tournament(2, 8);
        fill_teams(&mut t, &ids, 4, 2);
        t.finalize_registration().unwrap();

        let mut met: HashSet<(TeamId, TeamId)> = HashSet::new();
        for _ in 0..3 {
            start_round(&mut t, Vec::new());
            for m in t.team_matches().pop().unwrap() {
                let key = (m.team1.min(m.team2), m.team1.max(m.team2));
                assert!(met.insert(key), "a rematch: {key:?}");
            }
            // Decide every board so the round completes and the next can start.
            for i in 0..t.rounds.last().unwrap().boards.len() {
                let number = t.rounds.last().unwrap().number;
                t.toggle_board_winner(number, i, Winner::Player1).unwrap();
            }
        }
        // A round-robin of four teams over three rounds: every pair met once.
        assert_eq!(met.len(), 6);
    }

    #[test]
    fn rosters_freeze_at_finalization() {
        let (mut t, ids) = team_tournament(2, 4);
        let teams = fill_teams(&mut t, &ids, 2, 2);
        t.finalize_registration().unwrap();
        // `remove_team` is deliberately *not* in this list: a team that has not
        // been paired into a match yet is still removable, matching the rule on
        // players. See `a_team_that_has_played_cannot_be_removed`.
        for op in [
            t.clone().add_team("Late").err(),
            t.clone().remove_team_member(teams[0], ids[0]).err(),
            t.clone()
                .set_team_board_order(teams[0], vec![ids[1], ids[0]])
                .err(),
        ] {
            assert!(matches!(
                op,
                Some(TournamentError::RegistrationAlreadyFinalized)
            ));
        }
        // ...and so does registration itself: a late player would be teamless.
        assert!(matches!(
            t.add_player(NewPlayer {
                last_name: "Late".into(),
                ..Default::default()
            }),
            Err(TournamentError::NoLateRegistrationInTeamMode)
        ));
    }
}
