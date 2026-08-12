//! Helpers shared by the pairing tests.
//!
//! The engine speaks in [`UnitKey`]s, but the rules it implements are about
//! players and boards — so these wrappers drive it through the individual-mode
//! path and hand the tests back a [`Round`], which is what the assertions are
//! really about.

use std::collections::HashSet;

use uuid::Uuid;

use crate::player::Player;
use crate::round::{Board, Outcome, PairingSource, Round, Sitout, SitoutKind, SitoutValue, Winner};
use crate::settings::TournamentSettings;
use crate::units::{TournamentId, UnitKey};

use super::counterfactual::{counterfactual_forbid, counterfactual_force, Counterfactual};
use super::explain::{explain_pairing, RoundExplanation};
use super::matching::pair_round_weighted;
use super::model::player_units;

/// The engine seen through the individual-mode wrapper: build the player
/// units, pair them, and reassemble the `Round` the caller would — so these
/// tests keep speaking in players and boards, which is what the rules are
/// actually about. Mirrors `Tournament::confirm_round_inner`'s Swiss half.
pub(super) fn pair_players(
    number: u32,
    players: &[Player],
    settings: &TournamentSettings,
    completed_rounds: &[Round],
    present: &[TournamentId],
) -> Round {
    pair_players_forced(number, players, settings, completed_rounds, present, &[])
}

/// [`pair_players`] for the one test that pins boards in advance. Forced byes
/// and prequalified players are not reachable here: they are exercised a level
/// up, through `Tournament::update_draft`.
pub(super) fn pair_players_forced(
    number: u32,
    players: &[Player],
    settings: &TournamentSettings,
    completed_rounds: &[Round],
    present: &[TournamentId],
    forced_boards: &[Board],
) -> Round {
    let units = player_units(players, settings, completed_rounds, &[]);
    let present: Vec<UnitKey> = present.iter().copied().map(UnitKey::from).collect();
    let forced_pairs: Vec<(UnitKey, UnitKey)> = forced_boards
        .iter()
        .map(|b| (UnitKey::from(b.player1), UnitKey::from(b.player2)))
        .collect();
    let paired = pair_round_weighted(number, settings, &units, &present, &forced_pairs, &[]);
    let sitouts: Vec<Sitout> = paired
        .swiss_bye
        .map(|key| Sitout {
            player: TournamentId::from(key),
            kind: SitoutKind::Bye,
            value: SitoutValue::Full,
        })
        .into_iter()
        .collect();
    Round {
        number,
        explanation: RoundExplanation::empty(number),
        boards: paired
            .pairs
            .iter()
            .map(|p| {
                Board::pending(
                    TournamentId::from(p.a),
                    TournamentId::from(p.b),
                    p.points_diff,
                    p.source,
                )
            })
            .collect(),
        sitouts,
        completed: false,
    }
}

/// `explain_pairing` seen through the individual-mode wrapper (see
/// [`pair_players`]), so these tests keep speaking in players.
pub(super) fn explain_players(
    number: u32,
    players: &[Player],
    settings: &TournamentSettings,
    completed_rounds: &[Round],
    swiss_boards: &[(TournamentId, TournamentId)],
    bye: Option<TournamentId>,
) -> RoundExplanation {
    let units = player_units(players, settings, completed_rounds, &[]);
    explain_pairing(
        number,
        settings,
        &units,
        &keys(swiss_boards),
        bye.map(UnitKey::from),
    )
}

/// The counterfactual probes, likewise wrapped for the player path. Every
/// counterfactual test probes a first round, so there is no prior-round
/// history to pass; add a `completed_rounds` parameter here if one ever needs
/// it.
pub(super) fn force_players(
    number: u32,
    players: &[Player],
    settings: &TournamentSettings,
    swiss_boards: &[(TournamentId, TournamentId)],
    bye: Option<TournamentId>,
    a: TournamentId,
    b: TournamentId,
) -> Counterfactual {
    let units = player_units(players, settings, &[], &[]);
    counterfactual_force(
        number,
        settings,
        &units,
        &keys(swiss_boards),
        bye.map(UnitKey::from),
        UnitKey::from(a),
        UnitKey::from(b),
    )
}

pub(super) fn forbid_players(
    number: u32,
    players: &[Player],
    settings: &TournamentSettings,
    swiss_boards: &[(TournamentId, TournamentId)],
    bye: Option<TournamentId>,
    a: TournamentId,
    b: TournamentId,
) -> Counterfactual {
    let units = player_units(players, settings, &[], &[]);
    counterfactual_forbid(
        number,
        settings,
        &units,
        &keys(swiss_boards),
        bye.map(UnitKey::from),
        UnitKey::from(a),
        UnitKey::from(b),
    )
}

/// Player-number pairs as engine unit keys.
pub(super) fn keys(boards: &[(TournamentId, TournamentId)]) -> Vec<(UnitKey, UnitKey)> {
    boards
        .iter()
        .map(|&(a, b)| (UnitKey::from(a), UnitKey::from(b)))
        .collect()
}

pub(super) fn player(tid: u32, rating: Option<u32>, club: Option<&str>) -> Player {
    player_nat(tid, rating, club, None)
}

/// [`player`] plus a nationality, for the nationality-protection tests.
pub(super) fn player_nat(
    tid: u32,
    rating: Option<u32>,
    club: Option<&str>,
    nationality: Option<&str>,
) -> Player {
    Player {
        id: Uuid::new_v4(),
        tournament_id: Some(TournamentId(tid)),
        last_name: format!("P{tid}"),
        first_name: String::new(),
        rating,
        pairing_rating: None,
        grade: None,
        fesa_games: None,
        nationality: nationality.map(|n| n.to_string()),
        club: club.map(|c| c.to_string()),
        eligible: false,
        categories: Vec::new(),
        adjustments: Vec::new(),
    }
}

pub(super) fn completed_round(
    number: u32,
    boards: &[(TournamentId, TournamentId, Winner)],
    bye: Option<TournamentId>,
) -> Round {
    Round {
        number,
        explanation: RoundExplanation::empty(number),
        boards: boards
            .iter()
            .map(|&(a, b, w)| Board {
                outcome: Outcome::won(w),
                ..Board::pending(a, b, 0, PairingSource::Swiss)
            })
            .collect(),
        sitouts: bye
            .map(|player| Sitout {
                player,
                kind: SitoutKind::Bye,
                value: SitoutValue::Full,
            })
            .into_iter()
            .collect(),
        completed: true,
    }
}

pub(super) fn unord(a: TournamentId, b: TournamentId) -> (TournamentId, TournamentId) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

pub(super) fn board_pairs(round: &Round) -> HashSet<(TournamentId, TournamentId)> {
    round
        .boards
        .iter()
        .map(|b| unord(b.player1, b.player2))
        .collect()
}
