//! "Why not pair A and B?" — and its mirror, "why *did* you pair A and B?".
//!
//! Both answers are produced the same way: re-solve the round with the probed
//! edge forced in (or forbidden), tie-broken toward the pairing the referee is
//! looking at so the alternative differs in as few boards as it can, then diff
//! the two matchings into the rings of players that had to move and the net
//! per-rule cost of moving them.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use typed_index_collections::TiSlice;

use crate::settings::TournamentSettings;
use crate::units::{TournamentId, UnitKey};

use super::explain::{ledger, BoardLedger};
use super::matching::solve_matching;
use super::model::{PairingModel, PairingUnit};
use super::rules::{ladder_add, ladder_mul, RuleId};

/// The wire value meaning "the bye" in a counterfactual probe — the caller-facing
/// spelling of [`UnitKey::PHANTOM`]. Real tournament numbers are `>= 1`, so `0`
/// can never collide with a player.
pub(crate) const PHANTOM: TournamentId = TournamentId(0);

/// Normalized (order-independent) edge, so `(a, b)` and `(b, a)` are one key.
fn unord_pair(a: UnitKey, b: UnitKey) -> (UnitKey, UnitKey) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Why a probed player is out of the engine's hands — its board wasn't chosen by
/// the Swiss matching, so the counterfactual has nothing to reason about.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum ScopeReason {
    /// The player's board was fixed by the referee.
    Forced,
    /// The player's board comes from the cup bracket.
    Cup,
    /// The player is finishing a long game carried from the previous round. The
    /// engine did not pair them here — it paired them one round earlier — so
    /// there is no decision in *this* round to ask about.
    MidLongGame,
    /// The player sat this round out (absent or the bye is not being probed).
    Absent,
}

/// One board's share of a rule's change: the units that rule scores on it,
/// signed as [`RuleDelta::units`] is — positive for a board the counterfactual
/// *adds*, negative for one it takes away. `player2` is `None` for a bye.
///
/// This is what makes a net delta readable: "worse by 1 on repeat float" is a
/// number, while "this new board costs 2, and the board it replaces cost 1" is
/// an explanation.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(deny_unknown_fields)]
pub struct BoardDelta {
    pub player1: UnitKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub player2: Option<UnitKey>,
    #[ts(type = "number")]
    pub units: i64,
}

/// One rule's net change between the confirmed pairing and the counterfactual:
/// signed penalty units, where a positive value means the alternative is *worse*
/// on this rule.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct RuleDelta {
    pub rule: RuleId,
    #[ts(type = "number")]
    pub units: i64,
    /// Where that net came from, board by board: the boards the counterfactual
    /// adds first, then the ones it drops, each in a stable order. Only boards
    /// this rule actually scores on appear, so the list is never empty and its
    /// units always sum to [`Self::units`].
    pub boards: Vec<BoardDelta>,
}

/// A ring of players who must reshuffle to honour the probe — a vertex-disjoint
/// alternating cycle of (baseline △ counterfactual), ordered for storytelling.
/// The bye appears as the nil sentinel so clients can render it as "(bye)".
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct AffectedCycle {
    pub players: Vec<UnitKey>,
}

/// The consequence of forcing a pairing the engine didn't choose: which boards
/// change, the rings of affected players, and the net per-rule cost.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct Counterfactual {
    /// Set when the probe can't be reasoned about (a probed player isn't an
    /// engine-paired Swiss player); the other fields are then empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scoped_out: Option<ScopeReason>,
    /// Per-rule net change (priority order), only rules that actually moved.
    pub cost_delta: Vec<RuleDelta>,
    /// The affected player rings.
    pub cycles: Vec<AffectedCycle>,
    /// The new boards (those that differ from the confirmed pairing), each with
    /// its rule ledger. A board with no `player2` is a new bye.
    pub changed: Vec<BoardLedger>,
}

/// Minimum-cost perfect matching over `verts` (real players, plus a trailing
/// [`PHANTOM`] when a bye participates), tie-broken toward `baseline` so among
/// equal-cost solutions the one closest to the baseline wins (fewest boards
/// changed), and never using any edge in `forbidden`. Returned as normalized
/// pairs.
///
/// The stability tier is folded in arithmetically rather than as an extra
/// [`Rule`](super::rules::Rule): each edge costs `real_cost · (edges + 1) + (edge not in baseline)`.
/// Since `real_cost` is already the correct lexicographic scalar for the real
/// rules and the stability term is at most `edges < edges + 1`, this is exactly
/// the lexicographic order `(real rules, then boards changed)` — the same
/// ordering appending a lowest-priority rule to [`scale_ladder`](super::rules::scale_ladder) would give.
///
/// A forbidden edge is priced above any whole matching that avoids it (`max ·
/// edges + 1`), so it is never chosen when an alternative perfect matching
/// exists — which it always does on a complete graph of ≥ 4 vertices.
fn solve_stable(
    model: &PairingModel,
    verts: &[UnitKey],
    baseline: &HashSet<(UnitKey, UnitKey)>,
    forbidden: &HashSet<(UnitKey, UnitKey)>,
) -> Vec<(UnitKey, UnitKey)> {
    let n = verts.len();
    if n < 2 {
        return Vec::new();
    }
    // Canonical vertex order by seed, matching `pair_round_weighted`, so the
    // counterfactual re-solve breaks ties the same way the real pairing does.
    let mut verts = verts.to_vec();
    verts.sort_unstable();
    let stab = (n / 2) as i128 + 1; // strictly above the largest stability total
    let base = |a: UnitKey, b: UnitKey| -> i128 {
        if a == UnitKey::PHANTOM {
            model.bye_cost(b)
        } else if b == UnitKey::PHANTOM {
            model.bye_cost(a)
        } else {
            model.edge_cost(a, b)
        }
    };
    // Row-major `n × n`, upper triangle only: `min_weight_perfect_matching` reads
    // the matrix as symmetric and never looks below the diagonal, so mirroring
    // each cost would be a store nothing reads — as in `pair_round_weighted`'s
    // own fill.
    let mut cost = vec![0i128; n * n];
    let mut max_c = 0i128;
    for i in 0..n {
        for j in (i + 1)..n {
            let stray = i128::from(!baseline.contains(&unord_pair(verts[i], verts[j])));
            // `base·stab` compounds the ladder magnitude by ~n/2, so this path
            // overflows at smaller fields than plain pairing — checked, same as
            // the ladder itself (see [`ladder_mul`]).
            let c = ladder_add(ladder_mul(base(verts[i], verts[j]), stab), stray);
            cost[i * n + j] = c;
            max_c = max_c.max(c);
        }
    }
    if !forbidden.is_empty() {
        // Above the total of any perfect matching that avoids the edge.
        let prohibitive = ladder_add(ladder_mul(max_c, n as i128 / 2), 1);
        for i in 0..n {
            for j in (i + 1)..n {
                if forbidden.contains(&unord_pair(verts[i], verts[j])) {
                    cost[i * n + j] = prohibitive;
                }
            }
        }
    }
    let mate = solve_matching(&cost, n);
    let mut seen = vec![false; n];
    let mut pairs = Vec::new();
    for i in 0..n {
        if seen[i] {
            continue;
        }
        let j = mate[i];
        seen[i] = true;
        seen[j] = true;
        pairs.push(unord_pair(verts[i], verts[j]));
    }
    pairs
}

/// Decompose the symmetric difference of two perfect matchings into its
/// alternating cycles — the disjoint rings of players whose partners differ.
fn alternating_cycles(
    m0: &HashSet<(UnitKey, UnitKey)>,
    m1: &HashSet<(UnitKey, UnitKey)>,
) -> Vec<AffectedCycle> {
    // Adjacency over the changed edges. Every vertex in the symmetric difference
    // has exactly one edge from each matching, so its degree here is exactly 2.
    let mut adj: HashMap<UnitKey, Vec<UnitKey>> = HashMap::new();
    for &(a, b) in m0.symmetric_difference(m1) {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    }

    let mut visited: HashSet<UnitKey> = HashSet::new();
    let mut cycles = Vec::new();
    let mut starts: Vec<UnitKey> = adj.keys().copied().collect();
    starts.sort(); // deterministic cycle order
    for start in starts {
        if visited.contains(&start) {
            continue;
        }
        let mut order = Vec::new();
        let mut cur = start;
        let mut prev: Option<UnitKey> = None;
        loop {
            visited.insert(cur);
            order.push(cur);
            let nbrs = &adj[&cur];
            // Both matchings are perfect over the same vertices, so every vertex
            // of their symmetric difference has exactly one edge from each — the
            // degree-2 property this walk indexes into. Degree 1 would panic on
            // `nbrs[1]`, degree > 2 would silently walk the wrong ring; either
            // means the two matchings aren't both perfect, which only this crate
            // can get wrong.
            debug_assert_eq!(
                nbrs.len(),
                2,
                "vertex {cur:?} has degree {} in the symmetric difference, not 2",
                nbrs.len()
            );
            let next = if Some(nbrs[0]) != prev {
                nbrs[0]
            } else {
                nbrs[1]
            };
            if next == start {
                break;
            }
            prev = Some(cur);
            cur = next;
        }
        cycles.push(AffectedCycle { players: order });
    }
    cycles
}

/// Which alternative a referee is probing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum CounterfactualMode {
    /// "Why aren't A and B paired?" — force the edge, re-solve the rest.
    Force,
    /// "Why did you pair A and B?" — forbid the edge, re-solve.
    Forbid,
}

/// The Swiss free set, the pairing model, and the confirmed matching `M0` (with
/// the bye as a phantom edge) — the shared setup for either counterfactual.
fn baseline_matching<'u>(
    number: u32,
    settings: &TournamentSettings,
    units: &'u TiSlice<UnitKey, PairingUnit>,
    swiss_boards: &[(UnitKey, UnitKey)],
    bye: Option<UnitKey>,
) -> (
    PairingModel<'u>,
    Vec<UnitKey>,
    bool,
    HashSet<(UnitKey, UnitKey)>,
) {
    let mut free: Vec<UnitKey> = swiss_boards.iter().flat_map(|&(x, y)| [x, y]).collect();
    if let Some(p) = bye {
        free.push(p);
    }
    let need_phantom = bye.is_some();
    let model = PairingModel::build(number, settings, units, &free, need_phantom);
    let mut m0: HashSet<(UnitKey, UnitKey)> = swiss_boards
        .iter()
        .map(|&(x, y)| unord_pair(x, y))
        .collect();
    if let Some(p) = bye {
        m0.insert(unord_pair(p, UnitKey::PHANTOM));
    }
    (model, free, need_phantom, m0)
}

/// Diff the confirmed matching `m0` against the counterfactual `m1` into a
/// [`Counterfactual`]: the net per-rule cost, the affected rings, and the new
/// boards as ledgers.
fn diff_matchings(
    model: &PairingModel,
    m0: &HashSet<(UnitKey, UnitKey)>,
    m1: &HashSet<(UnitKey, UnitKey)>,
) -> Counterfactual {
    let rules = model.rules();
    let units_of = |e: &(UnitKey, UnitKey)| -> Vec<i128> {
        let (x, y) = *e;
        if x == UnitKey::PHANTOM {
            model.bye_units(y)
        } else if y == UnitKey::PHANTOM {
            model.bye_units(x)
        } else {
            model.edge_units(x, y)
        }
    };
    let ledger_of = |e: &(UnitKey, UnitKey)| -> BoardLedger {
        let (x, y) = *e;
        if x == UnitKey::PHANTOM {
            ledger(y, None, rules, &model.bye_units(y))
        } else if y == UnitKey::PHANTOM {
            ledger(x, None, rules, &model.bye_units(x))
        } else {
            ledger(x, Some(y), rules, &model.edge_units(x, y))
        }
    };

    // Sorted, so both the sums below and the per-rule board lists come out in
    // the same order every time a probe is repeated.
    let mut added: Vec<(UnitKey, UnitKey)> = m1.difference(m0).copied().collect();
    added.sort();
    let mut removed: Vec<(UnitKey, UnitKey)> = m0.difference(m1).copied().collect();
    removed.sort();

    // Net per-rule delta: added boards contribute +units, removed boards −units;
    // boards common to both matchings cancel. Each board that scores at all is
    // also kept against its rule, which is what turns the net into something
    // that can be read board by board.
    let mut delta: HashMap<RuleId, i64> = HashMap::new();
    let mut boards: HashMap<RuleId, Vec<BoardDelta>> = HashMap::new();
    for (edges, sign) in [(&added, 1i64), (&removed, -1i64)] {
        for e in edges {
            let (x, y) = *e;
            let (player1, player2) = if x == UnitKey::PHANTOM {
                (y, None)
            } else if y == UnitKey::PHANTOM {
                (x, None)
            } else {
                (x, Some(y))
            };
            for (r, &u) in rules.iter().zip(&units_of(e)) {
                if u == 0 {
                    continue;
                }
                let units = sign * u as i64;
                *delta.entry(r.id()).or_insert(0) += units;
                boards.entry(r.id()).or_default().push(BoardDelta {
                    player1,
                    player2,
                    units,
                });
            }
        }
    }
    let cost_delta: Vec<RuleDelta> = rules
        .iter()
        .filter_map(|r| {
            let u = delta.get(&r.id()).copied().unwrap_or(0);
            // A rule can score on both sides and come out even — that is not a
            // change, and reporting it as one would read as "this costs
            // something" when it costs nothing.
            (u != 0).then(|| RuleDelta {
                rule: r.id(),
                units: u,
                boards: boards.remove(&r.id()).unwrap_or_default(),
            })
        })
        .collect();

    // The new boards (added edges) as ledgers.
    let changed: Vec<BoardLedger> = added.iter().map(ledger_of).collect();

    Counterfactual {
        scoped_out: None,
        cost_delta,
        cycles: alternating_cycles(m0, m1),
        changed,
    }
}

/// Explain what forcing the pairing `a`–`b` would cost, relative to the round's
/// confirmed Swiss pairing. Both must be engine-paired players of this round (the
/// caller checks scope). Re-solves the rest with the forced edge pre-placed and a
/// stability tie-break toward the confirmed pairing, then diffs the two.
pub(crate) fn counterfactual_force(
    number: u32,
    settings: &TournamentSettings,
    units: &TiSlice<UnitKey, PairingUnit>,
    swiss_boards: &[(UnitKey, UnitKey)],
    bye: Option<UnitKey>,
    a: UnitKey,
    b: UnitKey,
) -> Counterfactual {
    let (model, free, need_phantom, m0) =
        baseline_matching(number, settings, units, swiss_boards, bye);

    // Re-solve everyone but the forced pair, then add the forced edge back for
    // the full counterfactual matching. The phantom stays in play for the rest
    // *unless* it's one side of the forced pair itself (forcing someone onto the
    // bye) — it's already spoken for then, not up for grabs again.
    let mut verts: Vec<UnitKey> = free.iter().copied().filter(|&v| v != a && v != b).collect();
    if need_phantom && a != UnitKey::PHANTOM && b != UnitKey::PHANTOM {
        verts.push(UnitKey::PHANTOM);
    }
    let no_forbidden = HashSet::new();
    let mut m1: HashSet<(UnitKey, UnitKey)> = solve_stable(&model, &verts, &m0, &no_forbidden)
        .into_iter()
        .collect();
    m1.insert(unord_pair(a, b));

    diff_matchings(&model, &m0, &m1)
}

/// Explain why the engine paired `a`–`b` rather than something else: forbid that
/// edge, re-solve the whole free set with a stability tie-break toward the
/// confirmed pairing, and diff. If `a`–`b` wasn't the engine's choice anyway, the
/// diff is empty.
pub(crate) fn counterfactual_forbid(
    number: u32,
    settings: &TournamentSettings,
    units: &TiSlice<UnitKey, PairingUnit>,
    swiss_boards: &[(UnitKey, UnitKey)],
    bye: Option<UnitKey>,
    a: UnitKey,
    b: UnitKey,
) -> Counterfactual {
    let (model, free, need_phantom, m0) =
        baseline_matching(number, settings, units, swiss_boards, bye);

    let mut verts = free.clone();
    if need_phantom {
        verts.push(UnitKey::PHANTOM);
    }
    let forbidden: HashSet<(UnitKey, UnitKey)> = [unord_pair(a, b)].into_iter().collect();
    let m1: HashSet<(UnitKey, UnitKey)> = solve_stable(&model, &verts, &m0, &forbidden)
        .into_iter()
        .collect();

    diff_matchings(&model, &m0, &m1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::test_support::*;
    use crate::player::Player;

    fn changed_pairs(cf: &Counterfactual) -> HashSet<(TournamentId, TournamentId)> {
        cf.changed
            .iter()
            .map(|b| {
                unord(
                    TournamentId::from(b.player1),
                    b.player2.map_or(PHANTOM, TournamentId::from),
                )
            })
            .collect()
    }

    #[test]
    fn forcing_a_pairing_swaps_a_minimal_ring() {
        // Round 1, one group of four by rating p0>p1>p2>p3. The fold pairs
        // p0-p2 / p1-p3. Force p0-p1: the only consistent completion is p2-p3, so
        // exactly those two boards change and the affected ring is all four.
        let p: Vec<Player> = (1..=4)
            .map(|i| player(i, Some(2000 - i * 10), None))
            .collect();
        let boards = [
            (p[0].tournament_id.unwrap(), p[2].tournament_id.unwrap()),
            (p[1].tournament_id.unwrap(), p[3].tournament_id.unwrap()),
        ];

        let cf = force_players(
            1,
            &p,
            &TournamentSettings::default(),
            &boards,
            None,
            p[0].tournament_id.unwrap(),
            p[1].tournament_id.unwrap(),
        );

        assert!(cf.scoped_out.is_none());
        let changed = changed_pairs(&cf);
        assert!(
            changed.contains(&unord(
                p[0].tournament_id.unwrap(),
                p[1].tournament_id.unwrap()
            )),
            "the forced board appears"
        );
        assert!(
            changed.contains(&unord(
                p[2].tournament_id.unwrap(),
                p[3].tournament_id.unwrap()
            )),
            "its forced completion appears"
        );
        assert_eq!(changed.len(), 2);
        assert_eq!(cf.cycles.len(), 1);
        assert_eq!(cf.cycles[0].players.len(), 4);
    }

    #[test]
    fn forcing_the_status_quo_changes_nothing() {
        // Probing a pairing the engine already made yields an empty diff.
        let p: Vec<Player> = (1..=4)
            .map(|i| player(i, Some(2000 - i * 10), None))
            .collect();
        let boards = [
            (p[0].tournament_id.unwrap(), p[2].tournament_id.unwrap()),
            (p[1].tournament_id.unwrap(), p[3].tournament_id.unwrap()),
        ];

        let cf = force_players(
            1,
            &p,
            &TournamentSettings::default(),
            &boards,
            None,
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap(),
        );

        assert!(cf.changed.is_empty());
        assert!(cf.cycles.is_empty());
        assert!(cf.cost_delta.is_empty());
    }

    #[test]
    fn forcing_a_worse_pairing_reports_the_cost() {
        // Force the non-fold pairing p0-p1: the deviation from the fold ideal is
        // strictly worse, so the net delta on the fold rule is positive.
        let p: Vec<Player> = (1..=4)
            .map(|i| player(i, Some(2000 - i * 10), None))
            .collect();
        let boards = [
            (p[0].tournament_id.unwrap(), p[2].tournament_id.unwrap()),
            (p[1].tournament_id.unwrap(), p[3].tournament_id.unwrap()),
        ];

        let cf = force_players(
            1,
            &p,
            &TournamentSettings::default(),
            &boards,
            None,
            p[0].tournament_id.unwrap(),
            p[1].tournament_id.unwrap(),
        );

        let fold = cf.cost_delta.iter().find(|d| d.rule == RuleId::Fold);
        assert!(
            matches!(fold, Some(d) if d.units > 0),
            "forcing the worse fold costs units"
        );
    }

    #[test]
    fn a_rule_delta_is_its_boards_added_and_dropped() {
        // Same forced pairing as above, read board by board: every rule that
        // moved says which boards moved it, the signs say which way (added
        // boards positive, dropped ones negative), and they add up to the net.
        let p: Vec<Player> = (1..=4)
            .map(|i| player(i, Some(2000 - i * 10), None))
            .collect();
        let boards = [
            (p[0].tournament_id.unwrap(), p[2].tournament_id.unwrap()),
            (p[1].tournament_id.unwrap(), p[3].tournament_id.unwrap()),
        ];

        let cf = force_players(
            1,
            &p,
            &TournamentSettings::default(),
            &boards,
            None,
            p[0].tournament_id.unwrap(),
            p[1].tournament_id.unwrap(),
        );

        assert!(!cf.cost_delta.is_empty(), "the forced fold has a cost");
        let added: HashSet<(UnitKey, UnitKey)> = cf
            .changed
            .iter()
            .map(|b| unord_pair(b.player1, b.player2.unwrap_or(UnitKey::PHANTOM)))
            .collect();
        for delta in &cf.cost_delta {
            assert!(
                !delta.boards.is_empty(),
                "{:?} moved, so some board moved it",
                delta.rule
            );
            assert_eq!(
                delta.units,
                delta.boards.iter().map(|b| b.units).sum::<i64>(),
                "{:?}'s net is what its boards add up to",
                delta.rule
            );
            for board in &delta.boards {
                assert_ne!(board.units, 0, "a board that scores nothing is not listed");
                let pair = unord_pair(board.player1, board.player2.unwrap_or(UnitKey::PHANTOM));
                assert_eq!(
                    board.units > 0,
                    added.contains(&pair),
                    "a board is positive exactly when the counterfactual adds it"
                );
            }
        }
    }

    #[test]
    fn forcing_across_a_bye_reassigns_the_sit_out() {
        // Three equal players: one byes. Force the current bye-taker to play the
        // player they'd otherwise sit behind, and someone else must take the bye —
        // surfaced as a changed board with no player2.
        let p: Vec<Player> = (1..=3).map(|i| player(i, Some(1500), None)).collect();
        // Engine round: p0-p1 play, p2 byes (deterministic by tournament number).
        let round = pair_players(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &p.iter()
                .map(|x| x.tournament_id.unwrap())
                .collect::<Vec<_>>(),
        );
        let boards: Vec<(TournamentId, TournamentId)> = round
            .boards
            .iter()
            .map(|b| (b.player1, b.player2))
            .collect();
        let bye = round.swiss_bye().expect("odd count byes someone");
        let opponent = boards[0].0;

        let cf = force_players(
            1,
            &p,
            &TournamentSettings::default(),
            &boards,
            Some(bye),
            bye,
            opponent,
        );

        assert!(cf.scoped_out.is_none());
        assert!(
            cf.changed.iter().any(|b| b.player2.is_none()),
            "someone new takes the bye"
        );
    }

    #[test]
    fn forbidding_an_engine_board_repairs_without_it() {
        // Round 1, group of four: the engine's fold ideal is p0-p2 / p1-p3.
        // Forbid p0-p2 and the only alternative (p0-p1 / p2-p3) must be chosen,
        // so p0-p2 is gone and the fold cost rises.
        let p: Vec<Player> = (1..=4)
            .map(|i| player(i, Some(2000 - i * 10), None))
            .collect();
        let boards = [
            (p[0].tournament_id.unwrap(), p[2].tournament_id.unwrap()),
            (p[1].tournament_id.unwrap(), p[3].tournament_id.unwrap()),
        ];

        let cf = forbid_players(
            1,
            &p,
            &TournamentSettings::default(),
            &boards,
            None,
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap(),
        );

        let changed = changed_pairs(&cf);
        assert!(
            !changed.contains(&unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )),
            "the forbidden board is gone"
        );
        assert!(!cf.changed.is_empty());
        let fold = cf.cost_delta.iter().find(|d| d.rule == RuleId::Fold);
        assert!(
            matches!(fold, Some(d) if d.units > 0),
            "avoiding the ideal costs fold units"
        );
    }

    #[test]
    fn forbidding_an_unused_pairing_changes_nothing() {
        // p0-p1 was never the engine's choice, so forbidding it is a no-op.
        let p: Vec<Player> = (1..=4)
            .map(|i| player(i, Some(2000 - i * 10), None))
            .collect();
        let boards = [
            (p[0].tournament_id.unwrap(), p[2].tournament_id.unwrap()),
            (p[1].tournament_id.unwrap(), p[3].tournament_id.unwrap()),
        ];

        let cf = forbid_players(
            1,
            &p,
            &TournamentSettings::default(),
            &boards,
            None,
            p[0].tournament_id.unwrap(),
            p[1].tournament_id.unwrap(),
        );

        assert!(cf.changed.is_empty());
        assert!(cf.cost_delta.is_empty());
    }

    #[test]
    fn forcing_a_player_onto_the_bye_reassigns_the_sit_out() {
        // Three equal players: p0-p1 play, p2 byes. Force p0 onto the bye
        // instead (PHANTOM stands for the bye slot) — p2 must then play the
        // freed-up p1, and the new bye (p0) has no player2.
        let p: Vec<Player> = (1..=3).map(|i| player(i, Some(1500), None)).collect();
        let round = pair_players(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &p.iter()
                .map(|x| x.tournament_id.unwrap())
                .collect::<Vec<_>>(),
        );
        let boards: Vec<(TournamentId, TournamentId)> = round
            .boards
            .iter()
            .map(|b| (b.player1, b.player2))
            .collect();
        let bye = round.swiss_bye().expect("odd count byes someone");
        let (playing_a, playing_b) = boards[0];
        assert_ne!(playing_a, bye);

        let cf = force_players(
            1,
            &p,
            &TournamentSettings::default(),
            &boards,
            Some(bye),
            playing_a,
            PHANTOM,
        );

        assert!(cf.scoped_out.is_none());
        assert!(
            cf.changed
                .iter()
                .any(|b| TournamentId::from(b.player1) == playing_a && b.player2.is_none()),
            "the forced player now takes the bye"
        );
        assert!(
            changed_pairs(&cf).contains(&unord(bye, playing_b)),
            "the old bye-taker plays the freed-up opponent"
        );
    }

    #[test]
    fn forbidding_the_bye_forces_the_sit_out_to_play() {
        // Same setup, but forbid the current bye instead of forcing a new one:
        // the bye-taker must now play, and someone else sits out.
        let p: Vec<Player> = (1..=3).map(|i| player(i, Some(1500), None)).collect();
        let round = pair_players(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &p.iter()
                .map(|x| x.tournament_id.unwrap())
                .collect::<Vec<_>>(),
        );
        let boards: Vec<(TournamentId, TournamentId)> = round
            .boards
            .iter()
            .map(|b| (b.player1, b.player2))
            .collect();
        let bye = round.swiss_bye().expect("odd count byes someone");

        let cf = forbid_players(
            1,
            &p,
            &TournamentSettings::default(),
            &boards,
            Some(bye),
            bye,
            PHANTOM,
        );

        assert!(cf.scoped_out.is_none());
        assert!(
            !cf.changed
                .iter()
                .any(|b| TournamentId::from(b.player1) == bye && b.player2.is_none()),
            "the old bye-taker no longer sits out"
        );
        assert!(
            cf.changed.iter().any(|b| b.player2.is_none()),
            "someone else takes the bye instead"
        );
    }
}
