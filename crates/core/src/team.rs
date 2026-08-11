//! Teams: the unit of pairing and ranking in a team tournament.
//!
//! A team tournament (the format that traditionally precedes the European
//! championships) pairs *teams*, but the games themselves stay ordinary
//! individual boards: a match between two teams of `size` players is `size`
//! boards, board k against board k. See `docs/team-tournaments.md`.
//!
//! Only the roster is stored. Everything team-level that could be derived —
//! match results, team scores, the board↔match grouping — is recomputed by
//! replay from the boards, the same way the cup replays its bracket, so editing
//! a past board result re-derives every team outcome that depends on it.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::player::Player;
use crate::tournament::{Tournament, TournamentError};
use crate::units::TeamId;

/// A registered team. Stored on [`Tournament`](crate::Tournament); present (and
/// non-empty) only in team mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
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
}

impl Team {
    /// A new team with no members yet.
    pub fn new(name: String) -> Self {
        Team {
            id: Uuid::new_v4(),
            tournament_id: None,
            name,
            members: Vec::new(),
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

/// The average pairing rating over a roster, or `None` when no member has one.
///
/// Unrated members are simply left out of the average rather than dragging it
/// down: without MacMahon starting points a team may legitimately carry unrated
/// players, and the average then only feeds soft uses (fold order, team
/// numbering, the default board order) where "unrated sorts last" is the same
/// convention individual mode already uses. With MacMahon in play, finalization
/// requires every member to have a pairing rating, so nothing is missing there.
pub fn average_pairing_rating(members: &[&Player]) -> Option<u32> {
    let rated: Vec<u32> = members.iter().filter_map(|p| pairing_rating(p)).collect();
    if rated.is_empty() {
        return None;
    }
    // Integer mean, rounded to nearest — a team's average is only ever compared
    // against other averages and MacMahon thresholds, both integers.
    let sum: u64 = rated.iter().map(|&r| u64::from(r)).sum();
    Some(((sum + rated.len() as u64 / 2) / rated.len() as u64) as u32)
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

    /// The team a player belongs to, if any.
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

    /// Delete a team. Its members go back to the unassigned pool; they are *not*
    /// removed from the tournament, since being in a team and being registered
    /// are separate facts.
    pub fn remove_team(&mut self, team: Uuid) -> Result<(), TournamentError> {
        self.team_registration_open()?;
        let before = self.teams.len();
        self.teams.retain(|t| t.id != team);
        if self.teams.len() == before {
            return Err(TournamentError::TeamNotFound(team));
        }
        Ok(())
    }

    /// Add a registered player to a team, at the end of its board order.
    ///
    /// A player belongs to exactly one team, so this rejects a player already in
    /// another (call [`remove_team_member`](Self::remove_team_member) first) and a
    /// team already at its full size. Adding a member the team already has is a
    /// no-op rather than a duplicate.
    pub fn add_team_member(&mut self, team: Uuid, player: Uuid) -> Result<&Team, TournamentError> {
        self.team_registration_open()?;
        if !self.players.iter().any(|p| p.id == player) {
            return Err(TournamentError::PlayerNotFound(player));
        }
        if let Some(other) = self.team_of(player) {
            if other.id != team {
                return Err(TournamentError::PlayerAlreadyInATeam(player));
            }
        }
        let size = self.settings.team_size();
        let t = self.team_mut(team)?;
        if t.members.contains(&player) {
            return Ok(t);
        }
        if t.members.len() as u32 >= size {
            return Err(TournamentError::TeamIsFull { size });
        }
        t.members.push(player);
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
        // Descending rating, unrated last; `sort_by` is stable, so members with
        // no rating keep their relative order rather than being shuffled.
        order.sort_by(|a, b| match (a.1, b.1) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
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
        Ok(p)
    }

    /// Mutable access to a team by id.
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
    fn the_team_average_skips_unrated_members_and_rounds_to_nearest() {
        let a = player(Some(2000), None);
        let b = player(Some(1801), None);
        let c = player(None, None);
        // (2000 + 1801) / 2 = 1900.5 → 1901; the unrated member is left out.
        assert_eq!(average_pairing_rating(&[&a, &b, &c]), Some(1901));
        // A referee-assigned pairing rating counts like a real one.
        let d = player(None, Some(1600));
        assert_eq!(average_pairing_rating(&[&a, &d]), Some(1800));
        // No rated member at all: no average, rather than a fake zero.
        assert_eq!(average_pairing_rating(&[&c]), None);
    }

    #[test]
    fn names_collide_ignoring_case_and_surrounding_space() {
        assert!(Team::same_name("Paris", " paris "));
        assert!(!Team::same_name("Paris", "Paris 2"));
    }

    // --- Rosters on a tournament -----------------------------------------

    use crate::settings::{MacMahonThreshold, TeamSettings, TournamentSettings};

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

    /// Fill `teams` teams of `size` from `ids`, in order.
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

    /// Until team pairing lands, a finalized team tournament refuses to pair
    /// rather than silently pairing its players as individuals.
    #[test]
    fn preparing_a_round_is_refused_in_team_mode_for_now() {
        let (mut t, ids) = team_tournament(2, 4);
        fill_teams(&mut t, &ids, 2, 2);
        t.finalize_registration().unwrap();
        assert!(matches!(
            t.prepare_round(),
            Err(TournamentError::TeamPairingNotImplemented)
        ));
    }

    #[test]
    fn rosters_freeze_at_finalization() {
        let (mut t, ids) = team_tournament(2, 4);
        let teams = fill_teams(&mut t, &ids, 2, 2);
        t.finalize_registration().unwrap();
        for op in [
            t.clone().add_team("Late").err(),
            t.clone().remove_team(teams[0]).err(),
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
