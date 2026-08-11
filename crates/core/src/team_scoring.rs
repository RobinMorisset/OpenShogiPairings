//! Team-level replay: the board↔match grouping, match outcomes, and the team
//! scores the pairing engine and the standings are built on.
//!
//! Nothing here is stored. Every team-level fact is *derived* from the rosters
//! and the ordinary boards, exactly as the cup derives its bracket — so editing
//! a past board result re-derives the match it belongs to, the team's points,
//! and every tie-break that reads them. See `docs/team-tournaments.md`.

use std::cmp::Ordering;
use std::collections::HashMap;

use typed_index_collections::TiVec;

use crate::player::Player;
use crate::round::{Board, Round, SitoutKind, SitoutValue, Winner};
use crate::settings::{Tiebreak, TournamentSettings};
use crate::standings::{rank_groups, sum_dropping_lowest, DcRow};
use crate::team::{average_pairing_rating, Team};
use crate::units::{HalfPoints, TeamId, TournamentId, Wins};

/// Where each player sits in the team structure, and the rosters by team number
/// — the lookup every team-level derivation starts from.
///
/// Built from the frozen rosters once registration is finalized, so both a
/// player's team and their board number are just an index away. A tournament
/// number no team claims (number 0, a gap) maps to `None`.
pub(crate) struct TeamSlots {
    by_tid: TiVec<TournamentId, Option<(TeamId, u32)>>,
    members: TiVec<TeamId, Vec<TournamentId>>,
}

impl TeamSlots {
    /// Derive the slots from the finalized teams. Players without a number, and
    /// teams without one, are skipped — neither can appear on a board.
    pub(crate) fn new(teams: &[Team], players: &[Player]) -> Self {
        let tid_of: HashMap<uuid::Uuid, TournamentId> = players
            .iter()
            .filter_map(|p| p.tournament_id.map(|t| (p.id, t)))
            .collect();
        let max_tid = players
            .iter()
            .filter_map(|p| p.tournament_id)
            .map(usize::from)
            .max()
            .unwrap_or(0);
        let max_team = teams
            .iter()
            .filter_map(|t| t.tournament_id)
            .map(usize::from)
            .max()
            .unwrap_or(0);

        let mut by_tid: TiVec<TournamentId, Option<(TeamId, u32)>> = vec![None; max_tid + 1].into();
        let mut members: TiVec<TeamId, Vec<TournamentId>> = vec![Vec::new(); max_team + 1].into();
        for team in teams {
            let Some(number) = team.tournament_id else {
                continue;
            };
            for (board, member) in team.members.iter().enumerate() {
                let Some(&tid) = tid_of.get(member) else {
                    continue;
                };
                by_tid[tid] = Some((number, board as u32));
                members[number].push(tid);
            }
        }
        TeamSlots { by_tid, members }
    }

    /// The team a player plays for, and their board number (0 = board 1).
    pub(crate) fn slot_of(&self, player: TournamentId) -> Option<(TeamId, u32)> {
        self.by_tid.get(player).copied().flatten()
    }

    /// The team a player plays for.
    pub(crate) fn team_of(&self, player: TournamentId) -> Option<TeamId> {
        self.slot_of(player).map(|(team, _)| team)
    }

    /// A team's members, by tournament number, in board order.
    pub(crate) fn members_of(&self, team: TeamId) -> &[TournamentId] {
        self.members.get(team).map_or(&[], Vec::as_slice)
    }

    /// Every team number in play, ascending.
    pub(crate) fn teams(&self) -> impl Iterator<Item = TeamId> + '_ {
        (1..self.members.len()).map(TeamId::from)
    }

    /// The number of team-number slots, so a caller can size a parallel table.
    pub(crate) fn capacity(&self) -> usize {
        self.members.len()
    }
}

/// One team match in a round: the two teams, and the boards it is made of.
///
/// `team1` is the team of every board's `player1`, so a board's frozen
/// `points_diff` reads as `points(team1) − points(team2)` without a sign flip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamMatch {
    pub team1: TeamId,
    pub team2: TeamId,
    /// Indices into `round.boards`, in board order.
    pub boards: Vec<usize>,
}

/// How a match stands: board wins on each side, and whether every board of it is
/// decided (an undecided board leaves the match without an outcome).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MatchResult {
    pub wins1: Wins,
    pub wins2: Wins,
    pub decided: bool,
}

impl MatchResult {
    /// The match points for `team1`'s side: a full point for strictly more board
    /// wins, half a point for a tie, nothing for fewer. `None` while any board is
    /// undecided.
    ///
    /// The tie is reachable even with an odd team size — a board where neither
    /// side turned up decides no winner — so the rule is defined for every size.
    pub(crate) fn points1(self) -> Option<HalfPoints> {
        if !self.decided {
            return None;
        }
        Some(match self.wins1.cmp(&self.wins2) {
            Ordering::Greater => HalfPoints::from_whole(1),
            Ordering::Equal => HalfPoints::from_halves(1),
            Ordering::Less => HalfPoints::ZERO,
        })
    }

    /// The same from `team2`'s side.
    pub(crate) fn points2(self) -> Option<HalfPoints> {
        MatchResult {
            wins1: self.wins2,
            wins2: self.wins1,
            decided: self.decided,
        }
        .points1()
    }

    /// Whether this is a match *win* (as opposed to a half) for `team1` — what
    /// the victory-based tie-breaks count.
    pub(crate) fn is_win1(self) -> bool {
        self.decided && self.wins1 > self.wins2
    }
}

/// The team matches of a round, in a deterministic order (by the lower team
/// number, then the higher), each with its boards in board order.
///
/// A board whose players aren't both in known teams is skipped: it cannot belong
/// to a match. That only arises from a hand-edited save — rosters are frozen
/// before any board exists.
pub(crate) fn matches_in_round(round: &Round, slots: &TeamSlots) -> Vec<TeamMatch> {
    // Keyed by the unordered team pair, so the two directions land in one match.
    let mut by_pair: HashMap<(TeamId, TeamId), TeamMatch> = HashMap::new();
    for (index, board) in round.boards.iter().enumerate() {
        let (Some(t1), Some(t2)) = (slots.team_of(board.player1), slots.team_of(board.player2))
        else {
            continue;
        };
        let key = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
        by_pair
            .entry(key)
            .or_insert_with(|| TeamMatch {
                // Orientation follows the boards, not the key: `team1` is
                // whichever team is on `player1`, so `points_diff` keeps its sign.
                team1: t1,
                team2: t2,
                boards: Vec::new(),
            })
            .boards
            .push(index);
    }
    let mut matches: Vec<TeamMatch> = by_pair.into_values().collect();
    for m in &mut matches {
        m.boards.sort_by_key(|&i| {
            slots
                .slot_of(round.boards[i].player1)
                .map_or(u32::MAX, |(_, board)| board)
        });
    }
    matches.sort_by_key(|m| {
        let (a, b) = (m.team1.min(m.team2), m.team1.max(m.team2));
        (a, b)
    });
    matches
}

/// The team the *engine* gave the bye to, if any — the team analogue of
/// [`Round::swiss_bye`]. A team byes as a unit, so any one of its member
/// sit-outs identifies it.
pub(crate) fn swiss_bye_team(round: &Round, slots: &TeamSlots) -> Option<TeamId> {
    slots.team_of(round.swiss_bye()?)
}

/// Count a match's board wins on each side, and whether it is fully decided.
///
/// A board win is the board's **effective** winner (so the Wiel handicap rule
/// applies exactly as it does to a player's own score), plus a forfeit where
/// only one side turned up — which is a board win for that side, whether the
/// missing player was a no-show or absent for cause.
pub(crate) fn match_result(round: &Round, m: &TeamMatch, wiel_rule: bool) -> MatchResult {
    let mut result = MatchResult {
        decided: true,
        ..MatchResult::default()
    };
    for &i in &m.boards {
        let board = &round.boards[i];
        if !board.is_decided() {
            result.decided = false;
            continue;
        }
        // A forfeit credits the side that turned up; `no_show_opponent` is
        // already `None` when neither did, so `Both` scores nobody.
        let winner_side = match board.outcome.forfeit() {
            Some(_) => board.no_show_opponent().map(|id| {
                if id == board.player1 {
                    Winner::Player1
                } else {
                    Winner::Player2
                }
            }),
            None => board.effective_winner(wiel_rule),
        };
        match winner_side {
            Some(Winner::Player1) => result.wins1 += Wins(1),
            Some(Winner::Player2) => result.wins2 += Wins(1),
            None => {}
        }
    }
    result
}

/// One team's accumulated state going into the next round — the team twin of
/// [`PlayerScore`](crate::scoring::PlayerScore), and what the pairing engine's
/// [`PairingUnit`](crate::pairing::PairingUnit)s are built from.
#[derive(Default, Clone)]
pub(crate) struct TeamScore {
    /// MacMahon starting points (from the team average) plus match points plus
    /// whatever each round the team sat out was worth to it.
    pub points: HalfPoints,
    /// Match *wins* only — a drawn match scores half a point but no victory.
    pub victories: Wins,
    /// Total games won by the team's players: the new team tie-break.
    pub board_wins: Wins,
    /// MacMahon starting points alone.
    pub macmahon: HalfPoints,
    /// Opposing teams faced, one entry per match.
    pub opponents: Vec<TeamId>,
    /// Opposing teams beaten, one entry per match won.
    pub defeated: Vec<TeamId>,
    pub cuss_m: HalfPoints,
    pub cuss_w: Wins,
    pub running_points: Vec<HalfPoints>,
    pub running_wins: Vec<Wins>,
    /// Whether the team has already had a bye of some kind.
    pub had_bye: bool,
    pub last_ascended: Option<u32>,
    pub last_descended: Option<u32>,
}

/// Per-team scores, indexed by team number. Number 0 is a gap holding a default.
pub(crate) struct TeamScores {
    by_id: TiVec<TeamId, TeamScore>,
}

impl TeamScores {
    /// The accumulated state at a team number.
    pub(crate) fn get(&self, team: TeamId) -> &TeamScore {
        &self.by_id[team]
    }

    /// The number of team-number slots.
    pub(crate) fn capacity(&self) -> usize {
        self.by_id.len()
    }
}

/// What a round was worth to a team that played no match, if it sat the round
/// out as a whole: the kind (a bye or an absence) and the value.
///
/// Every member of a team sits out together — a team bye is `size` member
/// sit-outs, a team absence likewise — and the value is edited at team level, so
/// the members' entries are uniform by construction.
fn team_sitout(round: &Round, members: &[TournamentId]) -> Option<(SitoutKind, SitoutValue)> {
    let first = round.sitout(*members.first()?)?;
    debug_assert!(
        members
            .iter()
            .filter_map(|&m| round.sitout(m))
            .all(|s| s.kind == first.kind && s.value == first.value),
        "a team's members sit out together, with one kind and one value"
    );
    // Only a whole-team sit-out is a team sit-out: a single member with no board
    // is an individual absence, which does not stop the team from playing.
    members
        .iter()
        .all(|&m| round.sitout(m).is_some())
        .then_some((first.kind, first.value))
}

/// Replay the **completed** rounds to accumulate each team's points, board wins,
/// opponents, bye status and float history — the team analogue of
/// [`compute_scores`](crate::scoring::compute_scores).
///
/// Float direction is read from the frozen `points_diff` of a match's boards,
/// which the pairing wrote as `points(team1) − points(team2)` at the time, so
/// the float history stays a fact of how each round was actually paired.
pub(crate) fn compute_team_scores(
    teams: &[Team],
    players: &[Player],
    settings: &TournamentSettings,
    rounds: &[Round],
    slots: &TeamSlots,
) -> TeamScores {
    let rounds_played = rounds.iter().filter(|r| r.completed).count() as u32;
    // MacMahon from the live estimate reads each member's estimate in place of
    // their rating, then averages as usual — so the thresholds still apply to a
    // team average, just a live one. Only computed when that mode is on: it
    // replays every game.
    let estimated_elos = settings
        .macmahon_from_estimate_active()
        .then(|| crate::elo::estimate_elos(players, settings, rounds));
    let by_uuid: HashMap<uuid::Uuid, &Player> = players.iter().map(|p| (p.id, p)).collect();

    let mut by_id: TiVec<TeamId, TeamScore> = std::iter::repeat_with(TeamScore::default)
        .take(slots.capacity())
        .collect();
    let mut real_teams: Vec<TeamId> = Vec::with_capacity(teams.len());
    for team in teams {
        let Some(number) = team.tournament_id else {
            continue;
        };
        let members: Vec<&Player> = team
            .members
            .iter()
            .filter_map(|id| by_uuid.get(id).copied())
            .collect();
        // The rating the ELO thresholds see: the team's average pairing rating,
        // over live estimates when estimate-based MacMahon is on.
        let mm_rating = match &estimated_elos {
            Some(est) => {
                let estimates: Vec<u32> = members
                    .iter()
                    .map(|p| {
                        est.get(&p.id)
                            .copied()
                            .unwrap_or(crate::elo::UNRATED_PRIOR_MEAN)
                            .round() as u32
                    })
                    .collect();
                mean(&estimates)
            }
            None => average_pairing_rating(&members),
        };
        // Grade thresholds are rejected in team mode (a team has an average
        // rating, not a grade), so there is no grade to pass.
        let macmahon =
            HalfPoints::from_whole(settings.macmahon_points_at(mm_rating, None, rounds_played));
        // Manual bonuses/maluses fold in alongside the MacMahon start, before
        // any round is replayed, so they shape both the standings and the
        // score-gap pairing weight from here on. They are whole-point deltas;
        // the signed floor at zero needs raw units, so this one computation
        // drops into halves. A team's effective score can't go below zero.
        let adjustment: i32 = team.adjustments.iter().map(|a| a.delta).sum();
        let points =
            HalfPoints::from_halves((macmahon.halves() as i32 + adjustment * 2).max(0) as u32);
        real_teams.push(number);
        by_id[number] = TeamScore {
            points,
            macmahon,
            opponents: Vec::with_capacity(rounds_played as usize),
            defeated: Vec::with_capacity(rounds_played as usize),
            ..TeamScore::default()
        };
    }

    for round in rounds.iter().filter(|r| r.completed) {
        let matches = matches_in_round(round, slots);
        // Opponents and floats are read from the state going *into* the round, so
        // they are recorded before this round's results are applied.
        for m in &matches {
            by_id[m.team1].opponents.push(m.team2);
            by_id[m.team2].opponents.push(m.team1);
            // Every board of a match carries the same team-level float, so any of
            // them answers for the match.
            let diff = m.boards.first().map_or(0, |&i| round.boards[i].points_diff);
            match diff.cmp(&0) {
                Ordering::Greater => {
                    by_id[m.team1].last_descended = Some(round.number);
                    by_id[m.team2].last_ascended = Some(round.number);
                }
                Ordering::Less => {
                    by_id[m.team1].last_ascended = Some(round.number);
                    by_id[m.team2].last_descended = Some(round.number);
                }
                Ordering::Equal => {}
            }
        }
        // A bye is a downfloat that shouldn't be handed out twice — which follows
        // the *kind*, never the score the referee gave the cell.
        for team in slots.teams() {
            if let Some((kind, _)) = team_sitout(round, slots.members_of(team)) {
                if kind.is_bye() {
                    by_id[team].had_bye = true;
                    by_id[team].last_descended = Some(round.number);
                }
            }
        }

        // Apply this round's results.
        for m in &matches {
            let result = match_result(round, m, settings.handicap_wiel_rule());
            by_id[m.team1].board_wins += result.wins1;
            by_id[m.team2].board_wins += result.wins2;
            let (Some(p1), Some(p2)) = (result.points1(), result.points2()) else {
                continue; // an undecided board leaves the match without an outcome
            };
            by_id[m.team1].points += p1;
            by_id[m.team2].points += p2;
            if result.is_win1() {
                by_id[m.team1].victories += Wins(1);
                by_id[m.team1].defeated.push(m.team2);
            } else if result.wins2 > result.wins1 {
                by_id[m.team2].victories += Wins(1);
                by_id[m.team2].defeated.push(m.team1);
            }
        }
        // Every whole-team sit-out scores whatever its cell says — a full point
        // for the usual bye, nothing for the usual absence. Only a full point is
        // a victory, exactly as for a player.
        for team in slots.teams() {
            if let Some((_, value)) = team_sitout(round, slots.members_of(team)) {
                by_id[team].points += value.points();
                if value.is_victory() {
                    by_id[team].victories += Wins(1);
                }
            }
        }

        for &team in &real_teams {
            let s = &mut by_id[team];
            s.cuss_m += s.points;
            s.cuss_w += s.victories;
            s.running_points.push(s.points);
            s.running_wins.push(s.victories);
        }
    }

    TeamScores { by_id }
}

/// Integer mean, rounded to nearest; `None` for an empty slice.
fn mean(values: &[u32]) -> Option<u32> {
    if values.is_empty() {
        return None;
    }
    let sum: u64 = values.iter().map(|&v| u64::from(v)).sum();
    Some(((sum + values.len() as u64 / 2) / values.len() as u64) as u32)
}

/// Build the pairing engine's input for a **team** tournament: one unit per
/// team, keyed by team number — the team twin of
/// [`player_units`](crate::pairing::player_units).
///
/// The clubs are the members' clubs *in board order*, which is what makes the
/// club rule count the same-club games a match would really create: board k only
/// ever meets board k.
pub(crate) fn team_units(
    teams: &[Team],
    players: &[Player],
    settings: &TournamentSettings,
    completed_rounds: &[Round],
    slots: &TeamSlots,
) -> TiVec<crate::units::UnitKey, crate::pairing::PairingUnit> {
    use crate::pairing::PairingUnit;
    use crate::units::UnitKey;

    let scores = compute_team_scores(teams, players, settings, completed_rounds, slots);
    let by_uuid: HashMap<uuid::Uuid, &Player> = players.iter().map(|p| (p.id, p)).collect();
    let mut units: TiVec<UnitKey, PairingUnit> =
        vec![PairingUnit::default(); scores.capacity()].into();
    for team in teams {
        let Some(number) = team.tournament_id else {
            continue;
        };
        let members: Vec<&Player> = team
            .members
            .iter()
            .filter_map(|id| by_uuid.get(id).copied())
            .collect();
        let s = scores.get(number);
        units[UnitKey::from(number)] = PairingUnit {
            points: s.points,
            macmahon: s.macmahon,
            opponents: s.opponents.iter().copied().map(UnitKey::from).collect(),
            had_bye: s.had_bye,
            last_ascended: s.last_ascended,
            last_descended: s.last_descended,
            // The fold orders teams by average strength, as it orders players by
            // rating; a member's referee-set pairing ELO counts here and nowhere
            // user-facing.
            rating: average_pairing_rating(&members),
            clubs: members
                .iter()
                .map(|p| {
                    p.club
                        .as_ref()
                        .map(|c| TournamentSettings::normalize_club(c))
                })
                .collect(),
            // Neither the cup nor ELO pairing exists in team mode.
            prequalified: false,
            elo: 0,
        };
    }
    units
}

/// The boards a team match expands to: roster[k] of `team1` against roster[k] of
/// `team2`, all carrying the match's frozen team-level float and source.
///
/// `team1` is always on `player1`, which is what makes `points_diff` readable
/// without a sign flip when the match is replayed.
pub(crate) fn match_boards(
    team1: TeamId,
    team2: TeamId,
    points_diff: i32,
    source: crate::round::PairingSource,
    slots: &TeamSlots,
) -> Vec<Board> {
    slots
        .members_of(team1)
        .iter()
        .zip(slots.members_of(team2))
        .map(|(&a, &b)| Board::pending(a, b, points_diff, source))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::NewPlayer;
    use crate::round::{AbsenceKind, Forfeit, Outcome};
    use crate::settings::{MacMahonThreshold, TeamSettings};
    use crate::tournament::Tournament;
    use uuid::Uuid;

    /// A finalized team tournament of `teams` teams of `size`, players rated
    /// 2000, 1990, … in registration order.
    fn tournament(
        size: u32,
        teams: usize,
        settings: TournamentSettings,
    ) -> (Tournament, Vec<Uuid>) {
        let mut t = Tournament::new("Teams").unwrap();
        t.update_settings(TournamentSettings {
            teams: Some(TeamSettings { size }),
            ..settings
        })
        .unwrap();
        let n = teams * size as usize;
        let ids: Vec<Uuid> = (0..n)
            .map(|i| {
                t.add_player(NewPlayer {
                    last_name: format!("P{i}"),
                    rating: Some(2000 - i as u32 * 10),
                    ..Default::default()
                })
                .unwrap()
                .id
            })
            .collect();
        for k in 0..teams {
            let team = t.add_team(&format!("T{k}")).unwrap().id;
            for i in 0..size as usize {
                t.add_team_member(team, ids[k * size as usize + i]).unwrap();
            }
        }
        t.finalize_registration().unwrap();
        (t, ids)
    }

    /// One completed round holding a single match of `outcomes`, board k first.
    fn one_match(t: &mut Tournament, outcomes: &[Outcome]) {
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let round = t.rounds.last_mut().unwrap();
        assert_eq!(round.boards.len(), outcomes.len());
        for (board, &outcome) in round.boards.iter_mut().zip(outcomes) {
            board.outcome = outcome;
        }
        round.completed = round.is_complete();
    }

    fn scores(t: &Tournament) -> TeamScores {
        let slots = t.team_slots();
        compute_team_scores(&t.teams, &t.players, &t.settings, &t.rounds, &slots)
    }

    #[test]
    fn a_match_is_won_by_the_side_with_more_board_wins() {
        // Three boards: 2–1 to the first team, so a full point and a victory for
        // it, nothing for the other. Both keep their own board-win totals.
        let (mut t, _) = tournament(3, 2, TournamentSettings::default());
        one_match(
            &mut t,
            &[
                Outcome::won(Winner::Player1),
                Outcome::won(Winner::Player2),
                Outcome::won(Winner::Player1),
            ],
        );
        let s = scores(&t);
        let (one, two) = (TeamId(1), TeamId(2));
        assert_eq!(s.get(one).points, HalfPoints::from_whole(1));
        assert_eq!(s.get(two).points, HalfPoints::ZERO);
        assert_eq!(s.get(one).victories, Wins(1));
        assert_eq!(s.get(two).victories, Wins(0));
        assert_eq!(s.get(one).board_wins, Wins(2));
        assert_eq!(s.get(two).board_wins, Wins(1));
        assert_eq!(s.get(one).defeated, vec![two]);
        assert_eq!(s.get(one).opponents, vec![two]);
    }

    /// The tie is reachable with an odd team size too, when a board has no
    /// winner at all — which is why the half-point rule is defined for every N.
    #[test]
    fn an_odd_sized_match_can_still_be_drawn() {
        let (mut t, _) = tournament(3, 2, TournamentSettings::default());
        one_match(
            &mut t,
            &[
                Outcome::won(Winner::Player1),
                Outcome::won(Winner::Player2),
                // Neither side turned up: a decided board with no winner.
                Outcome::Forfeit {
                    absent: Forfeit::Both(AbsenceKind::NoShow, AbsenceKind::NoShow),
                },
            ],
        );
        let s = scores(&t);
        assert_eq!(s.get(TeamId(1)).points, HalfPoints::from_halves(1));
        assert_eq!(s.get(TeamId(2)).points, HalfPoints::from_halves(1));
        // A half is not a victory, for a team as for a player.
        assert_eq!(s.get(TeamId(1)).victories, Wins(0));
        assert_eq!(s.get(TeamId(1)).board_wins, Wins(1));
    }

    #[test]
    fn a_forfeited_board_is_a_board_win_for_the_side_that_turned_up() {
        let (mut t, _) = tournament(2, 2, TournamentSettings::default());
        one_match(
            &mut t,
            &[
                Outcome::Forfeit {
                    absent: Forfeit::Player2(AbsenceKind::NoShow),
                },
                Outcome::won(Winner::Player2),
            ],
        );
        let s = scores(&t);
        // 1–1: a drawn match, but each side's board win is its own.
        assert_eq!(s.get(TeamId(1)).board_wins, Wins(1));
        assert_eq!(s.get(TeamId(2)).board_wins, Wins(1));
        assert_eq!(s.get(TeamId(1)).points, HalfPoints::from_halves(1));
    }

    #[test]
    fn an_undecided_board_leaves_the_match_without_an_outcome() {
        let (mut t, _) = tournament(2, 2, TournamentSettings::default());
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        // Only the first board played; the round stays open, so the replay skips
        // it entirely — but the derivation itself must also refuse to score it.
        let number = t.rounds[0].number;
        t.toggle_board_winner(number, 0, Winner::Player1).unwrap();
        assert!(!t.rounds[0].completed);
        let slots = t.team_slots();
        let matches = matches_in_round(&t.rounds[0], &slots);
        let result = match_result(&t.rounds[0], &matches[0], false);
        assert!(!result.decided);
        assert_eq!(result.points1(), None);
        assert_eq!(result.points2(), None);
    }

    #[test]
    fn a_team_bye_scores_a_match_win_and_bars_a_second_bye() {
        // Three teams of two: one team byes, and its bye is worth a full point
        // at team level — the analogue of a player's full-point bye.
        let (mut t, _) = tournament(2, 3, TournamentSettings::default());
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let number = t.rounds[0].number;
        for i in 0..t.rounds[0].boards.len() {
            t.toggle_board_winner(number, i, Winner::Player1).unwrap();
        }
        let slots = t.team_slots();
        let bye_team = slots.team_of(t.rounds[0].sitouts[0].player).unwrap();
        let s = scores(&t);
        assert_eq!(s.get(bye_team).points, HalfPoints::from_whole(1));
        assert_eq!(s.get(bye_team).victories, Wins(1));
        assert!(s.get(bye_team).had_bye);
        // ...and it doesn't count as a game: no opponent, no board win.
        assert!(s.get(bye_team).opponents.is_empty());
        assert_eq!(s.get(bye_team).board_wins, Wins(0));

        // The next round must not hand the same team the bye again.
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let slots = t.team_slots();
        let second = slots.team_of(t.rounds[1].sitouts[0].player).unwrap();
        assert_ne!(second, bye_team, "a team took two byes");
    }

    #[test]
    fn macmahon_starting_points_come_from_the_team_average() {
        // Team 1 averages (2000+1990)/2 = 1995, team 2 (1980+1970)/2 = 1975.
        // A single threshold at 1990 separates them.
        let (t, _) = tournament(
            2,
            2,
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1990)]),
        );
        let s = scores(&t);
        assert_eq!(s.get(TeamId(1)).macmahon, HalfPoints::from_whole(1));
        assert_eq!(s.get(TeamId(2)).macmahon, HalfPoints::ZERO);
        // ...and the starting points are already in the running total.
        assert_eq!(s.get(TeamId(1)).points, HalfPoints::from_whole(1));
    }

    /// The float frozen on a match's boards is the *team* difference, so the
    /// replay reads a team float straight off any board of the match.
    #[test]
    fn a_matchs_boards_all_carry_the_team_level_float() {
        let (mut t, _) = tournament(
            2,
            2,
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1990)]),
        );
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let round = &t.rounds[0];
        // Team 1 starts a point up on team 2 (see the MacMahon test above), and
        // every board of the match records that same difference.
        let diffs: Vec<i32> = round.boards.iter().map(|b| b.points_diff).collect();
        assert_eq!(diffs, vec![2, 2]);
        // The boards face team 1 on player1, which is what makes the sign read
        // without a flip.
        let slots = t.team_slots();
        for board in &round.boards {
            assert_eq!(slots.team_of(board.player1), Some(TeamId(1)));
        }
    }

    #[test]
    fn the_grouping_is_derived_from_the_rosters_alone() {
        let (mut t, _) = tournament(3, 4, TournamentSettings::default());
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let slots = t.team_slots();
        let matches = matches_in_round(&t.rounds[0], &slots);
        // Four teams of three: two matches of three boards each, and every board
        // of a match pairs the same two teams at aligned positions.
        assert_eq!(matches.len(), 2);
        for m in &matches {
            assert_eq!(m.boards.len(), 3);
            for (k, &i) in m.boards.iter().enumerate() {
                let board = &t.rounds[0].boards[i];
                assert_eq!(slots.slot_of(board.player1), Some((m.team1, k as u32)));
                assert_eq!(slots.slot_of(board.player2), Some((m.team2, k as u32)));
            }
        }
        // Matches come back in a stable order, lowest team number first.
        assert!(matches[0].team1.min(matches[0].team2) < matches[1].team1.min(matches[1].team2));
    }

    // --- Standings --------------------------------------------------------

    /// Play `rounds` rounds where the *first* board of every match is won by
    /// player 1 and the rest by player 2 — enough to spread the teams out.
    fn play(t: &mut Tournament, rounds: u32) {
        for _ in 0..rounds {
            t.prepare_round().unwrap();
            t.confirm_round().unwrap();
            let number = t.rounds.last().unwrap().number;
            let count = t.rounds.last().unwrap().boards.len();
            for i in 0..count {
                let side = if i % 2 == 0 {
                    Winner::Player1
                } else {
                    Winner::Player2
                };
                t.toggle_board_winner(number, i, side).unwrap();
            }
        }
    }

    #[test]
    fn team_standings_rank_by_match_points_then_board_wins() {
        let (mut t, _) = tournament(3, 2, TournamentSettings::default());
        // 2–1 to whichever team is on player1, so it takes the match.
        one_match(
            &mut t,
            &[
                Outcome::won(Winner::Player1),
                Outcome::won(Winner::Player2),
                Outcome::won(Winner::Player1),
            ],
        );
        let table = compute_team_standings(&t.teams, &t.players, &t.settings, &t.rounds);
        assert_eq!(table.len(), 2);
        assert_eq!(table[0].tournament_id, TeamId(1));
        assert_eq!(table[0].points, HalfPoints::from_whole(1));
        assert_eq!(table[0].board_wins, Wins(2));
        assert_eq!(table[0].victories, Wins(1));
        assert_eq!(table[1].board_wins, Wins(1));
        // The members come back in board order, ready for the breakdown rows.
        assert_eq!(table[0].members.len(), 3);
        // Opponents and defeated face callers by team id, as `Standing` does.
        assert_eq!(table[0].opponents, vec![table[1].team_id]);
        assert_eq!(table[0].defeated, vec![table[1].team_id]);
    }

    /// Board wins break a tie on match points — the established second criterion
    /// in team events, and the reason the tie-break exists.
    #[test]
    fn board_wins_break_a_tie_on_match_points() {
        let settings = TournamentSettings {
            tiebreaks: Tiebreak::default_team_order(),
            ..TournamentSettings::default()
        };
        // Four teams of two over one round: both matches drawn 1–1 would leave
        // everyone level, so make one match 2–0 and the other 1–1.
        let (mut t, _) = tournament(2, 4, settings);
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let number = t.rounds[0].number;
        let slots = t.team_slots();
        let matches = matches_in_round(&t.rounds[0], &slots);
        // First match: a clean sweep for team1. Second: one each.
        for (m, sides) in matches.iter().zip([
            [Winner::Player1, Winner::Player1],
            [Winner::Player1, Winner::Player2],
        ]) {
            for (&i, side) in m.boards.iter().zip(sides) {
                t.toggle_board_winner(number, i, side).unwrap();
            }
        }
        let table = compute_team_standings(&t.teams, &t.players, &t.settings, &t.rounds);
        // The sweeping team leads on match points; the two drawn teams are level
        // on points and split by board wins (1 each) — so the loser of the sweep
        // (0 board wins) comes last.
        assert_eq!(table[0].points, HalfPoints::from_whole(1));
        assert_eq!(table.last().unwrap().board_wins, Wins(0));
        assert!(table[1].points == table[2].points);
        assert!(table[1].board_wins >= table[2].board_wins);
    }

    #[test]
    fn the_sos_family_sums_opposing_teams_team_scores() {
        let (mut t, _) = tournament(2, 4, TournamentSettings::default());
        play(&mut t, 3);
        let table = compute_team_standings(&t.teams, &t.players, &t.settings, &t.rounds);
        let by_id: HashMap<uuid::Uuid, &TeamStanding> =
            table.iter().map(|s| (s.team_id, s)).collect();
        for s in &table {
            // Three rounds of four teams: everyone played everyone once.
            assert_eq!(s.opponents.len(), 3);
            let expected: u32 = s.opponents.iter().map(|o| by_id[o].points.halves()).sum();
            assert_eq!(
                s.sosm.halves(),
                expected,
                "SOSM sums opposing teams' points"
            );
            let expected_w: u32 = s.opponents.iter().map(|o| by_id[o].victories.get()).sum();
            assert_eq!(s.sosw.get(), expected_w);
            // ...and the running columns have one entry per completed round.
            assert_eq!(s.running_points.len(), 3);
        }
    }

    #[test]
    fn direct_confrontation_ranks_teams_that_all_played_each_other() {
        let settings = TournamentSettings {
            tiebreaks: vec![Tiebreak::Points, Tiebreak::Dc],
            ..TournamentSettings::default()
        };
        let (mut t, _) = tournament(2, 4, settings);
        play(&mut t, 3);
        let table = compute_team_standings(&t.teams, &t.players, &t.settings, &t.rounds);
        // Every team met every other, so any tied group is a complete subgraph
        // and DC applies: a team's DC never exceeds the matches it won.
        for s in &table {
            assert!(s.dc <= s.victories);
        }
    }

    #[test]
    fn standings_are_empty_before_finalization_and_absent_outside_team_mode() {
        let mut t = Tournament::new("Teams").unwrap();
        t.update_settings(TournamentSettings {
            teams: Some(TeamSettings { size: 2 }),
            ..TournamentSettings::default()
        })
        .unwrap();
        assert_eq!(t.team_standings(), Some(Vec::new()));
        let individual = Tournament::new("Solo").unwrap();
        assert_eq!(individual.team_standings(), None);
    }

    #[test]
    fn the_wiel_rule_decides_board_wins_as_it_decides_a_players_score() {
        let (mut t, _) = tournament(
            2,
            2,
            TournamentSettings {
                handicap_policy: crate::settings::HandicapPolicy::Enabled {
                    display: crate::settings::HandicapDisplay::Allowed,
                    wiel_rule: true,
                },
                ..TournamentSettings::default()
            },
        );
        t.prepare_round().unwrap();
        t.confirm_round().unwrap();
        let number = t.rounds[0].number;
        // Board 1: the receiver actually wins, but the Wiel rule credits the
        // giver, so the board win follows the giver's team.
        t.set_board_handicap(number, 0, Some(crate::round::Handicap::Rook))
            .unwrap();
        let giver = t.rounds[0].boards[0].handicap.unwrap().giver;
        let loser = match giver {
            Winner::Player1 => Winner::Player2,
            Winner::Player2 => Winner::Player1,
        };
        t.toggle_board_winner(number, 0, loser).unwrap();
        t.toggle_board_winner(number, 1, Winner::Player2).unwrap();

        let slots = t.team_slots();
        let matches = matches_in_round(&t.rounds[0], &slots);
        let result = match_result(&t.rounds[0], &matches[0], true);
        // The giver is player1 (the stronger team is on player1 and rated above),
        // so the Wiel rule makes it 1–1 rather than 0–2.
        assert_eq!(giver, Winner::Player1);
        assert_eq!((result.wins1, result.wins2), (Wins(1), Wins(1)));
    }
}

/// One team's standing: score, board wins and every tie-break metric — the team
/// twin of [`Standing`](crate::standings::Standing). Its position in the
/// [`compute_team_standings`] vector is the team's rank.
///
/// Faces callers by team [`Uuid`], as `Standing` does by player id; the members
/// are listed in board order so a client can expand a team row into its players
/// without re-deriving the roster.
///
/// There is no `estimated_elo`: the estimate is a per-player quantity, and the
/// tie-break that ranks by it is rejected in team mode.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct TeamStanding {
    pub team_id: uuid::Uuid,
    /// Dense team number, for the cross-table's opponent column.
    pub tournament_id: TeamId,
    pub name: String,
    /// Matches won (a drawn match scores half a point but no victory).
    pub victories: Wins,
    /// Games won by the team's players — the team tie-break.
    pub board_wins: Wins,
    /// MacMahon starting points, from the team's average pairing rating.
    pub macmahon: HalfPoints,
    /// Total score: MacMahon start + match points + whatever the rounds the team
    /// sat out were worth.
    pub points: HalfPoints,
    pub sosm: HalfPoints,
    pub sosw: Wins,
    pub sodosm: HalfPoints,
    pub sodosw: Wins,
    pub sososm: HalfPoints,
    pub sososw: Wins,
    pub sosm1: HalfPoints,
    pub sosm2: HalfPoints,
    pub sosw1: Wins,
    pub sosw2: Wins,
    pub cussm: HalfPoints,
    pub cussw: Wins,
    /// Direct confrontation among the teams still tied on every earlier
    /// criterion (see [`Tiebreak::Dc`]).
    pub dc: Wins,
    /// Opposing teams faced, one entry per match, in round order.
    pub opponents: Vec<uuid::Uuid>,
    /// Opposing teams beaten, the subset of `opponents` SODOS sums over.
    pub defeated: Vec<uuid::Uuid>,
    pub running_points: Vec<HalfPoints>,
    pub running_wins: Vec<Wins>,
    /// The team's members, by player id, in board order.
    pub members: Vec<uuid::Uuid>,
}

impl TeamStanding {
    /// This team's value for a ranking criterion, as a plain ordinal (see
    /// [`Standing::tiebreak`](crate::standings::Standing::tiebreak)).
    pub fn tiebreak(&self, tb: Tiebreak) -> u32 {
        match tb {
            Tiebreak::Points => self.points.halves(),
            Tiebreak::BoardWins => self.board_wins.get(),
            Tiebreak::SosM => self.sosm.halves(),
            Tiebreak::SosW => self.sosw.get(),
            Tiebreak::SodosM => self.sodosm.halves(),
            Tiebreak::SodosW => self.sodosw.get(),
            Tiebreak::SososM => self.sososm.halves(),
            Tiebreak::SososW => self.sososw.get(),
            Tiebreak::SosM1 => self.sosm1.halves(),
            Tiebreak::SosM2 => self.sosm2.halves(),
            Tiebreak::SosW1 => self.sosw1.get(),
            Tiebreak::SosW2 => self.sosw2.get(),
            Tiebreak::CussM => self.cussm.halves(),
            Tiebreak::CussW => self.cussw.get(),
            Tiebreak::Dc => self.dc.get(),
            // Rejected in team mode by settings validation, and dropped from the
            // order by `normalized`, so it never reaches the ranking.
            Tiebreak::EstElo => {
                debug_assert!(
                    false,
                    "est_elo ranks no team; settings validation rejects it"
                );
                0
            }
        }
    }
}

/// Compute the ranked team standings from the completed rounds — the team
/// analogue of [`compute_standings`](crate::standings::compute_standings), over
/// the same criteria with team semantics: the SOS family sums opposing *teams'*
/// team scores, and direct confrontation counts matches won.
pub fn compute_team_standings(
    teams: &[Team],
    players: &[Player],
    settings: &TournamentSettings,
    rounds: &[Round],
) -> Vec<TeamStanding> {
    let slots = TeamSlots::new(teams, players);
    let scores = compute_team_scores(teams, players, settings, rounds, &slots);
    let score_m = |o: &TeamId| scores.get(*o).points;
    let score_w = |o: &TeamId| scores.get(*o).victories;
    // Team number → its registration id, so the output can face callers by id
    // the way `Standing` does.
    let mut uuid_of: TiVec<TeamId, uuid::Uuid> = vec![uuid::Uuid::nil(); scores.capacity()].into();
    for team in teams {
        if let Some(number) = team.tournament_id {
            uuid_of[number] = team.id;
        }
    }

    // SOSM/SOSW first: the SOSOS metrics are sums of them over opponents.
    let mut sosm: TiVec<TeamId, HalfPoints> = vec![HalfPoints::ZERO; scores.capacity()].into();
    let mut sosw: TiVec<TeamId, Wins> = vec![Wins::ZERO; scores.capacity()].into();
    for number in slots.teams() {
        let s = scores.get(number);
        sosm[number] = s.opponents.iter().map(score_m).sum();
        sosw[number] = s.opponents.iter().map(score_w).sum();
    }

    let ranked: Vec<&Team> = teams.iter().filter(|t| t.tournament_id.is_some()).collect();
    let mut standings: Vec<TeamStanding> = ranked
        .iter()
        .map(|team| {
            let number = team.tournament_id.expect("filtered above");
            let s = scores.get(number);
            let opp_m: Vec<HalfPoints> = s.opponents.iter().map(&score_m).collect();
            let opp_w: Vec<Wins> = s.opponents.iter().map(&score_w).collect();
            TeamStanding {
                team_id: team.id,
                tournament_id: number,
                name: team.name.clone(),
                victories: s.victories,
                board_wins: s.board_wins,
                macmahon: s.macmahon,
                points: s.points,
                sosm: sosm[number],
                sosw: sosw[number],
                sodosm: s.defeated.iter().map(&score_m).sum(),
                sodosw: s.defeated.iter().map(&score_w).sum(),
                sososm: s.opponents.iter().map(|&o| sosm[o]).sum(),
                sososw: s.opponents.iter().map(|&o| sosw[o]).sum(),
                sosm1: sum_dropping_lowest(opp_m.clone(), 1),
                sosm2: sum_dropping_lowest(opp_m, 2),
                sosw1: sum_dropping_lowest(opp_w.clone(), 1),
                sosw2: sum_dropping_lowest(opp_w, 2),
                cussm: s.cuss_m,
                cussw: s.cuss_w,
                // Filled in by the ranking, which is what decides who is tied.
                dc: Wins::ZERO,
                opponents: s.opponents.iter().map(|&o| uuid_of[o]).collect(),
                defeated: s.defeated.iter().map(|&o| uuid_of[o]).collect(),
                running_points: s.running_points.clone(),
                running_wins: s.running_wins.clone(),
                members: team.members.clone(),
            }
        })
        .collect();

    let rows: Vec<DcRow> = standings
        .iter()
        .map(|s| DcRow {
            id: s.team_id,
            opponents: s.opponents.clone(),
            defeated: s.defeated.clone(),
        })
        .collect();
    let mut seed_order: Vec<usize> = (0..standings.len()).collect();
    seed_order.sort_by_key(|&i| standings[i].tournament_id);

    let (order, dc) = rank_groups(
        seed_order,
        &settings.tiebreaks,
        // The estimate is per player and ranks no team; settings validation
        // rejects it in team mode, and this is the belt to that braces.
        |tb| tb == Tiebreak::EstElo,
        |i, tb| standings[i].tiebreak(tb),
        &rows,
    );
    for (i, wins) in dc.into_iter().enumerate() {
        standings[i].dc = Wins(wins);
    }
    order.into_iter().map(|i| standings[i].clone()).collect()
}
