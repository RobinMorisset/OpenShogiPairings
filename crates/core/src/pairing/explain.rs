//! Explaining a round: which rules fired on each board, and what they cost.
//!
//! Everything here is scored against the very [`PairingModel`] the round was
//! paired from, so an explanation can never drift from the pairing it explains.
//! The result is frozen onto the [`Round`](crate::round::Round) when it is
//! confirmed — it records a past decision, and the inputs behind that decision
//! all stay editable.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use typed_index_collections::TiSlice;

use crate::settings::TournamentSettings;
use crate::units::UnitKey;

use super::model::{PairingModel, PairingUnit};
use super::rules::{Rule, RuleId};

/// One rule's contribution to a single board (or the bye): the rule and the
/// penalty *units* it emitted, before the priority multiplier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct RuleContribution {
    pub rule: RuleId,
    /// Small penalty count; serialized as a JSON number (fits in a JS number).
    #[ts(type = "number")]
    pub units: i64,
}

/// The rule ledger for one pairing: every rule that fired on it (units > 0), in
/// priority order, plus the highest-priority one — the rule that "bound" the
/// pairing. `player2` is `None` for the bye.
///
/// The two sides are [`UnitKey`]s, so this reads as player numbers in individual
/// mode and team numbers in team mode — the pairing it explains is a pairing of
/// whatever the engine was given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct BoardLedger {
    pub player1: UnitKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub player2: Option<UnitKey>,
    pub contributions: Vec<RuleContribution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub binding: Option<RuleId>,
}

/// How often one rule had to be relaxed across a whole round, and the total units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct RuleTotal {
    pub rule: RuleId,
    pub boards: u32,
    #[ts(type = "number")]
    pub units: i64,
}

/// A human-facing explanation of one round's Swiss pairings: a per-board ledger,
/// the bye's ledger, and the per-rule round totals (in priority order).
///
/// Frozen onto the `Round` when it is confirmed, because it is a
/// record of a past event rather than a function of the present state: the model
/// it scores against is the one that actually paired the round, and that model's
/// inputs (results, ratings, settings) are all editable afterwards. See the
/// appendix of `docs/public-access.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct RoundExplanation {
    pub round: u32,
    pub boards: Vec<BoardLedger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bye: Option<BoardLedger>,
    pub report: Vec<RuleTotal>,
}

impl RoundExplanation {
    /// The explanation of a round the engine chose nothing in: every board was
    /// referee-forced or drawn from the cup bracket, so there is no Swiss
    /// decision to account for. A real value, not a placeholder — a round of
    /// nothing but forced boards is explained by exactly this.
    pub fn empty(round: u32) -> Self {
        Self {
            round,
            boards: Vec::new(),
            bye: None,
            report: Vec::new(),
        }
    }
}

/// Turn a per-rule unit vector (aligned with `rules`, priority order) into a
/// ledger: keep only the rules that fired, and note the highest-priority one.
pub(super) fn ledger(
    player1: UnitKey,
    player2: Option<UnitKey>,
    rules: &[Rule],
    units: &[i128],
) -> BoardLedger {
    let contributions: Vec<RuleContribution> = rules
        .iter()
        .zip(units)
        .filter(|(_, &u)| u > 0)
        .map(|(r, &u)| RuleContribution {
            rule: r.id(),
            units: u as i64,
        })
        .collect();
    // `rules` is priority-ordered and the filter preserves order, so the first
    // surviving contribution is the binding (highest-priority) rule.
    let binding = contributions.first().map(|c| c.rule);
    BoardLedger {
        player1,
        player2,
        contributions,
        binding,
    }
}

/// Add one pairing's units into the running round totals.
fn accumulate(totals: &mut HashMap<RuleId, (u32, i64)>, rules: &[Rule], units: &[i128]) {
    for (r, &u) in rules.iter().zip(units) {
        if u > 0 {
            let entry = totals.entry(r.id()).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += u as i64;
        }
    }
}

/// Explain the Swiss pairings of one round: score each `swiss_boards` pair (and
/// the `bye`, if any) against the exact model the round was paired from, and roll
/// the per-rule units up into a round report.
///
/// `swiss_boards` must be the engine-paired boards only (forced/cup boards aren't
/// engine decisions and carry no explanation). The bye is treated as matched to
/// the phantom vertex, exactly as during pairing.
pub(crate) fn explain_pairing(
    number: u32,
    settings: &TournamentSettings,
    units: &TiSlice<UnitKey, PairingUnit>,
    swiss_boards: &[(UnitKey, UnitKey)],
    bye: Option<UnitKey>,
) -> RoundExplanation {
    // The Swiss free set the round was paired from: both sides of every Swiss
    // board, plus the bye. With a bye the count is odd, so a phantom participates.
    let mut free: Vec<UnitKey> = swiss_boards.iter().flat_map(|&(a, b)| [a, b]).collect();
    if let Some(b) = bye {
        free.push(b);
    }
    let need_phantom = bye.is_some();
    let model = PairingModel::build(number, settings, units, &free, need_phantom);
    let rules = model.rules();

    let mut totals: HashMap<RuleId, (u32, i64)> = HashMap::new();
    let mut boards = Vec::with_capacity(swiss_boards.len());
    for &(a, b) in swiss_boards {
        let units = model.edge_units(a, b);
        accumulate(&mut totals, rules, &units);
        boards.push(ledger(a, Some(b), rules, &units));
    }
    let bye_ledger = bye.map(|player| {
        let units = model.bye_units(player);
        accumulate(&mut totals, rules, &units);
        ledger(player, None, rules, &units)
    });

    // Report in priority order, keeping only rules that actually fired.
    let report: Vec<RuleTotal> = rules
        .iter()
        .filter_map(|r| {
            totals.get(&r.id()).map(|&(boards, units)| RuleTotal {
                rule: r.id(),
                boards,
                units,
            })
        })
        .collect();

    RoundExplanation {
        round: number,
        boards,
        bye: bye_ledger,
        report,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::model::player_units;
    use crate::pairing::test_support::*;
    use crate::player::Player;
    use crate::settings::MacMahonThreshold;
    use crate::units::TournamentId;

    fn contribution(ledger: &BoardLedger, rule: RuleId) -> Option<i64> {
        ledger
            .contributions
            .iter()
            .find(|c| c.rule == rule)
            .map(|c| c.units)
    }

    #[test]
    fn explain_flags_fold_deviation_as_binding() {
        // Round 1, one score group of four by rating p0>p1>p2>p3. The fold ideal
        // is p0-p2 / p1-p3; explaining the *non-ideal* p0-p1 / p2-p3 pairing shows
        // Fold as the (only, hence binding) rule that fired on each board.
        let p: Vec<Player> = (1..=4)
            .map(|i| player(i, Some(2000 - i * 10), None))
            .collect();
        let boards = [
            (p[0].tournament_id.unwrap(), p[1].tournament_id.unwrap()),
            (p[2].tournament_id.unwrap(), p[3].tournament_id.unwrap()),
        ];

        let ex = explain_players(1, &p, &TournamentSettings::default(), &[], &boards, None);

        assert_eq!(ex.boards.len(), 2);
        for board in &ex.boards {
            assert_eq!(board.binding, Some(RuleId::Fold));
            // p0-p1: (1−ideal(0))² + (0−ideal(1))² = (1−2)² + (0−3)² = 1 + 9 = 10.
            // p2-p3: (3−ideal(2))² + (2−ideal(3))² = (3−0)² + (2−1)² = 9 + 1 = 10.
            assert_eq!(contribution(board, RuleId::Fold), Some(10));
            // Nothing higher-priority fired: same score, no clubs, no floats.
            assert_eq!(board.contributions.len(), 1);
        }
        // The report rolls the two boards up: Fold, 2 boards, 20 units total.
        assert_eq!(ex.report.len(), 1);
        assert_eq!(ex.report[0].rule, RuleId::Fold);
        assert_eq!(ex.report[0].boards, 2);
        assert_eq!(ex.report[0].units, 20);
    }

    #[test]
    fn explain_clean_pairing_has_no_contributions() {
        // The fold-ideal pairing of the same group deviates from nothing, so every
        // rule emits zero units: empty ledgers, no binding rule, empty report.
        let p: Vec<Player> = (1..=4)
            .map(|i| player(i, Some(2000 - i * 10), None))
            .collect();
        let boards = [
            (p[0].tournament_id.unwrap(), p[2].tournament_id.unwrap()),
            (p[1].tournament_id.unwrap(), p[3].tournament_id.unwrap()),
        ];

        let ex = explain_players(1, &p, &TournamentSettings::default(), &[], &boards, None);

        for board in &ex.boards {
            assert!(board.contributions.is_empty());
            assert_eq!(board.binding, None);
        }
        assert!(ex.report.is_empty());
    }

    #[test]
    fn explain_ledger_matches_the_engine_units() {
        // Cross-group board: p0 (1 pt) floats down to meet a lower-group player.
        // Explaining the exact pairing the engine would produce, the per-board
        // ledger must equal what `edge_units` reports for that board — the whole
        // faithfulness guarantee of sharing the model.
        let p = vec![
            player(1, Some(2000), None), // 1 point after r1
            player(2, Some(1900), None),
            player(3, Some(1000), None),
            player(4, Some(900), None),
        ];
        let settings =
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let round = pair_players(1, &p, &settings, &[], &present);

        let boards: Vec<(TournamentId, TournamentId)> = round
            .boards
            .iter()
            .map(|b| (b.player1, b.player2))
            .collect();
        let ex = explain_players(1, &p, &settings, &[], &boards, round.swiss_bye());

        // Re-derive the units independently through a fresh model and compare.
        let mut free: Vec<TournamentId> = boards.iter().flat_map(|&(a, b)| [a, b]).collect();
        if let Some(b) = round.swiss_bye() {
            free.push(b);
        }
        let units = player_units(&p, &settings, &[], &[]);
        let free: Vec<UnitKey> = free.into_iter().map(UnitKey::from).collect();
        let model = PairingModel::build(1, &settings, &units, &free, round.swiss_bye().is_some());
        for (ledger, &(a, b)) in ex.boards.iter().zip(&boards) {
            let units = model.edge_units(UnitKey::from(a), UnitKey::from(b));
            let expected: i64 = model
                .rules()
                .iter()
                .zip(&units)
                .filter(|(r, _)| r.id() == RuleId::Fold)
                .map(|(_, &u)| u as i64)
                .sum();
            assert_eq!(contribution(ledger, RuleId::Fold).unwrap_or(0), expected);
        }
    }
}
