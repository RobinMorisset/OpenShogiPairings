//! The pairing rules, and the ladder that turns them into one scalar weight.
//!
//! Each [`Rule`] emits penalty *units* for an edge (or the bye); the priority
//! multipliers are derived from each rule's worst-case units per round by
//! [`scale_ladder`], so one unit of any rule outweighs every lower-priority rule
//! combined. The active set and its order live in [`active_rules`] — the single
//! source of truth for priority. The module docs of [`super`] describe the rules
//! themselves.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use typed_index_collections::{TiSlice, TiVec};

use crate::settings::{FloaterStyle, TournamentSettings};
use crate::units::{HalfPoints, UnitKey};

use super::model::PairingUnit;

/// Numerator of the float-repeat penalty, divided by the number of rounds since
/// the player last floated the same way. Chosen with many small divisors so the
/// decay reads smoothly.
const FLOAT_BASE: i128 = 720;

/// The pairing rules. The active subset and its priority order depend on the
/// mode (see [`active_rules`]); that ordering is the single source of truth for
/// priority, and the scalar multipliers are derived from it (see
/// [`scale_ladder`]).
#[derive(Clone, Copy)]
pub(super) enum Rule {
    /// Never play the same opponent twice / never take the bye twice.
    Rematch,
    /// (Qualifier cup, round 1 only) Never pair two **pre-qualified** cup players
    /// with each other. They sit out the qualification round and play the open
    /// instead, but the bracket is about to pair them against qualifiers, so a
    /// game between two of them now would be a preview of a bracket match — and
    /// the loser would carry a defeat into a bracket they were seeded straight
    /// into. Only in the rule set when there *are* pre-qualified players in the
    /// pool, which is the qualifier format's first round and nothing else.
    ///
    /// A penalty rather than a hard exclusion, deliberately: with 1.5×`size`
    /// eligible players and nobody else entered, the pre-qualified are the *only*
    /// players in the round-1 Swiss pool and must face each other. Like
    /// [`Rule::Rematch`], the engine then takes the fewest such pairings it can.
    CupPrequalified,
    /// The bye should go to the lowest-scoring free player; penalty is the
    /// square of the points gap to the lowest score among free players. A
    /// bye-only rule (real boards are neutral).
    ByeGroup,
    /// (Optional, first N rounds) Avoid pairing players with a different number
    /// of MacMahon points; penalty grows with the square of the gap. A no-op
    /// outside its configured round window.
    AirtightGroups,
    /// Prefer equal scores; penalty grows with the square of the points gap.
    ScoreGap,
    /// Avoid repeating a float in the same direction; decays with rounds since.
    FloatRepeat,
    /// When a pairing floats across groups, choose the right players to float:
    /// classic Swiss sends the weakest of the upper group down and the first of
    /// the lower group up; median Swiss sends the median of each group instead.
    FloaterSelection,
    /// Avoid pairing club-mates (ignored when a club is unknown).
    Club,
    /// (Optional, off by default) Avoid pairing compatriots (ignored when a
    /// nationality is unknown) — club protection's weaker sibling, sitting
    /// directly below it.
    Nationality,
    /// Fold within a score group (top half meets bottom half), by squared deviation.
    Fold,
    /// (Pure ELO mode only) Choose *who* takes the bye — the weakest present
    /// player by estimated ELO. A bye-only rule, sitting above [`Rule::EloGap`]
    /// (which is indifferent to the bye), so the sit-out is decided before the
    /// rest is optimized.
    ByeSelection,
    /// (Pure ELO mode) Prefer opponents of equal estimated ELO; penalty grows with
    /// the square of the ELO gap, replacing the whole Swiss score/float/fold family.
    EloGap,
}

/// A serializable identity for a [`Rule`], surfaced to clients so a pairing can be
/// explained in the vocabulary of its rules. Mirrors [`Rule`] exactly (minus any
/// explanation-internal tiebreakers, which carry no meaning to a referee).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
#[serde(rename_all = "snake_case")]
pub enum RuleId {
    Rematch,
    CupPrequalified,
    ByeGroup,
    AirtightGroups,
    ScoreGap,
    FloatRepeat,
    FloaterSelection,
    Club,
    Nationality,
    Fold,
    ByeSelection,
    EloGap,
}

/// The rules in effect, highest priority first, for the active mode. Swiss/
/// MacMahon is the default. The experimental (pure) ELO mode swaps the whole
/// score/float/fold/club family for a bye-selection rule and a squared-ELO-gap
/// rule, keeping only no-rematch above them.
pub(super) fn active_rules(settings: &TournamentSettings) -> &'static [Rule] {
    const SWISS: [Rule; 10] = [
        Rule::Rematch,
        Rule::CupPrequalified,
        Rule::ByeGroup,
        Rule::AirtightGroups,
        Rule::ScoreGap,
        Rule::FloatRepeat,
        Rule::FloaterSelection,
        Rule::Club,
        Rule::Nationality,
        Rule::Fold,
    ];
    const ELO: [Rule; 4] = [
        Rule::Rematch,
        Rule::CupPrequalified,
        Rule::ByeSelection,
        Rule::EloGap,
    ];
    if settings.elo_estimate_needed() {
        &ELO
    } else {
        &SWISS
    }
}

/// Everything the rules need to score an edge, plus the per-round quantities their
/// worst-case bounds (and hence multipliers) are derived from.
pub(super) struct Ctx<'a> {
    /// The units being paired, indexed by their key (gaps hold a default).
    pub(super) units: &'a TiSlice<UnitKey, PairingUnit>,
    /// Fold placement per free unit, indexed by key (`None` for a non-free unit,
    /// whose key still indexes the slice).
    pub(super) fold: &'a TiSlice<UnitKey, Option<FoldInfo>>,
    pub(super) round: u32,
    /// Which unit each lower group sends up as its ascending floater.
    pub(super) floater_style: FloaterStyle,
    /// Clubs exempt from protection, in normalized form (see
    /// [`TournamentSettings::normalize_club`]).
    pub(super) exempt_clubs: &'a HashSet<String>,
    /// Nationalities exempt from protection, in normalized form (see
    /// [`TournamentSettings::normalize_nationality`]).
    pub(super) exempt_nationalities: &'a HashSet<String>,
    /// Edges in a perfect matching over the vertices (= vertices / 2).
    pub(super) edges: i128,
    /// Largest points gap between any two vertices (bounds the score rule).
    pub(super) max_gap: i128,
    /// Lowest points among the free units (the bye's target group).
    pub(super) min_points: i128,
    /// Largest MacMahon-points gap between any two vertices (bounds the airtight
    /// groups rule).
    pub(super) max_mm_gap: i128,
    /// Largest score-group size among the free units (bounds the fold rule).
    pub(super) max_group: i128,
    /// Number of free units (bounds the bye-selection rule).
    pub(super) free_count: i128,
    /// Most board positions any free unit has (1 for a player, the team size for
    /// a team) — the club rule's per-edge maximum.
    pub(super) max_boards: i128,
    /// (ELO mode) Ascending ELO rank per free unit, 0 = weakest, indexed by key;
    /// all zero in Swiss mode.
    pub(super) elo_rank: &'a TiSlice<UnitKey, i128>,
    /// (ELO mode) Largest rounded-ELO gap among free units (bounds the ELO-gap
    /// rule).
    pub(super) max_elo_gap: i128,
}

/// Float-repeat units for one player/direction: 0 if they never floated that way,
/// else `FLOAT_BASE` decayed by the rounds since (at least 1, so `≤ FLOAT_BASE`).
fn float_units(last: Option<u32>, round: u32) -> i128 {
    match last {
        Some(k) => {
            // A float is recorded by replaying *completed* rounds, so it always
            // predates the round being paired. `k == round` would divide by zero
            // and `k > round` would underflow; both mean the score replay and the
            // round number disagree, which only this crate can get wrong.
            debug_assert!(
                k < round,
                "a float recorded in round {k} does not predate round {round}"
            );
            FLOAT_BASE / (round - k) as i128
        }
        None => 0,
    }
}

/// Floater-selection units for one floater: how far its in-group rank is from the
/// ideal position for its float direction. In classic Swiss, a descending floater
/// ideally sits last (weakest) in its group and an ascending floater first; in
/// median Swiss, both ideally sit at the median. 0 if the player has no fold info
/// (shouldn't happen for free players) or its group is a singleton.
fn floater_units(ctx: &Ctx, id: UnitKey, descending: bool) -> i128 {
    let Some(f) = &ctx.fold[id] else {
        // `fold_ranks` fills an entry for every free unit, and this rule only ever
        // scores free units (the edges and the bye are built from `free`), so a
        // missing entry means the free set and the fold table were built from
        // different lists. Answering "indifferent" would silently drop the
        // floater-selection tier for that unit instead of saying so.
        debug_assert!(
            false,
            "no fold info for free unit {id:?} — the free set and the fold table disagree"
        );
        return 0;
    };
    let ideal = match ctx.floater_style {
        FloaterStyle::Classic => {
            if descending {
                f.group_size.saturating_sub(1)
            } else {
                0
            }
        }
        FloaterStyle::Median => f.group_size / 2,
    };
    (f.rank as i128 - ideal as i128).abs()
}

/// Penalty units for one "shared affiliation" rule — [`Rule::Club`] over the
/// units' clubs, [`Rule::Nationality`] over their nationalities: how many
/// **aligned** board positions carry the same, non-exempt value on both sides.
///
/// Aligned, because board `k` of one unit only ever meets board `k` of the
/// other: a shared club sitting on different boards never actually plays, and
/// costs nothing. So within the rule's ladder tier the matching minimizes the
/// clashing *games* of the round, not some team-level notion of a shared
/// affiliation. A player holds one position, so this degenerates to the
/// individual mode's 0/1. Both values are already normalized (case-folded), and
/// an unknown value on either side is never a clash.
#[inline]
fn shared_affiliation_units(
    a: &[Option<String>],
    b: &[Option<String>],
    exempt: &HashSet<String>,
) -> i128 {
    a.iter()
        .zip(b)
        .filter(|(x, y)| match (x, y) {
            (Some(nx), Some(ny)) => nx == ny && !exempt.contains(nx),
            _ => false,
        })
        .count() as i128
}

impl Rule {
    /// The serializable identity of this rule, for explanations.
    pub(super) fn id(self) -> RuleId {
        match self {
            Rule::Rematch => RuleId::Rematch,
            Rule::CupPrequalified => RuleId::CupPrequalified,
            Rule::ByeGroup => RuleId::ByeGroup,
            Rule::AirtightGroups => RuleId::AirtightGroups,
            Rule::ScoreGap => RuleId::ScoreGap,
            Rule::FloatRepeat => RuleId::FloatRepeat,
            Rule::FloaterSelection => RuleId::FloaterSelection,
            Rule::Club => RuleId::Club,
            Rule::Nationality => RuleId::Nationality,
            Rule::Fold => RuleId::Fold,
            Rule::ByeSelection => RuleId::ByeSelection,
            Rule::EloGap => RuleId::EloGap,
        }
    }

    /// Penalty units for pairing `a` against `b` (before the priority multiplier).
    ///
    /// `#[inline]` so that when the caller holds a *constant* rule (the per-rule
    /// specialization in [`accumulate_edge_rule`]), the `match self` folds to that
    /// one arm and the body inlines into the O(k²) fill loop.
    #[inline]
    pub(super) fn edge_units(self, ctx: &Ctx, a: UnitKey, b: UnitKey) -> i128 {
        let sa = &ctx.units[a];
        let sb = &ctx.units[b];
        match self {
            Rule::Rematch => i128::from(sa.opponents.contains(&b)),
            // Rule 1b: only in the rule set when the set is non-empty, so the
            // lookup is free in every other round and every other format.
            Rule::CupPrequalified => i128::from(sa.prequalified && sb.prequalified),
            Rule::ByeGroup => 0,
            // Rule 3: only in the rule set when active (inactive rules are
            // filtered out upstream), so no `airtight_active` check.
            Rule::AirtightGroups => {
                let gap = sa.macmahon.halves() as i128 - sb.macmahon.halves() as i128;
                gap * gap
            }
            Rule::ScoreGap => {
                let gap = sa.points.halves() as i128 - sb.points.halves() as i128;
                gap * gap
            }
            // Rule 5: the lower-scored player is the ascender, the higher-scored
            // the descender.
            Rule::FloatRepeat => match sa.points.cmp(&sb.points) {
                Ordering::Less => {
                    float_units(sa.last_ascended, ctx.round)
                        + float_units(sb.last_descended, ctx.round)
                }
                Ordering::Greater => {
                    float_units(sa.last_descended, ctx.round)
                        + float_units(sb.last_ascended, ctx.round)
                }
                Ordering::Equal => 0,
            },
            // Rule 6: same-group edges aren't floats, so no penalty.
            Rule::FloaterSelection => match sa.points.cmp(&sb.points) {
                Ordering::Equal => 0,
                // The higher-scored player is the descender, the lower the ascender;
                // the comparison already told us which is which.
                Ordering::Greater => floater_units(ctx, a, true) + floater_units(ctx, b, false),
                Ordering::Less => floater_units(ctx, b, true) + floater_units(ctx, a, false),
            },
            // Rule 7: only in the rule set when protection is active this round
            // (inactive rules are filtered out upstream), so no `club_active`
            // check.
            Rule::Club => shared_affiliation_units(&sa.clubs, &sb.clubs, ctx.exempt_clubs),
            // Rule 8: also filtered out upstream when inactive.
            Rule::Nationality => shared_affiliation_units(
                &sa.nationalities,
                &sb.nationalities,
                ctx.exempt_nationalities,
            ),
            // Rule 9: squaring (rather than |·|) spreads an unavoidable deviation
            // across boards instead of dumping it all on one, so no single player
            // faces an opponent far from the fold's intent — and it matches the
            // squared ScoreGap / EloGap rules. See `docs/reference/swiss-fold.md`.
            Rule::Fold => {
                if sa.points != sb.points {
                    return 0;
                }
                match (&ctx.fold[a], &ctx.fold[b]) {
                    (Some(fa), Some(fb)) => {
                        let ia = ideal_rank(fa.rank, fa.group_size) as i128;
                        let ib = ideal_rank(fb.rank, fb.group_size) as i128;
                        let da = fb.rank as i128 - ia;
                        let db = fa.rank as i128 - ib;
                        da * da + db * db
                    }
                    // Same invariant as `floater_units`: both endpoints of an edge
                    // are free, so both have fold info. Without it the fold tier is
                    // silently indifferent on this edge.
                    _ => {
                        debug_assert!(
                            false,
                            "no fold info for free unit {a:?} or {b:?} — the free set \
                             and the fold table disagree"
                        );
                        0
                    }
                }
            }
            Rule::ByeSelection => 0,
            Rule::EloGap => {
                let gap = (sa.elo - sb.elo) as i128;
                gap * gap
            }
        }
    }

    /// Penalty units for giving `player` the bye (before the priority multiplier).
    /// A bye repeats the rematch rule (never bye twice) and counts as a downfloat.
    pub(super) fn bye_units(self, ctx: &Ctx, unit: UnitKey) -> i128 {
        let s = &ctx.units[unit];
        match self {
            Rule::Rematch => i128::from(s.had_bye),
            // A sit-out isn't a pairing, so two pre-qualified players can't clash
            // on it.
            Rule::CupPrequalified => 0,
            Rule::ByeGroup => {
                let gap = s.points.halves() as i128 - ctx.min_points;
                gap * gap
            }
            Rule::FloatRepeat => float_units(s.last_descended, ctx.round),
            // A bye is a downfloat, so it is scored as one (the "descending"
            // direction of floater_units).
            Rule::FloaterSelection => floater_units(ctx, unit, true),
            Rule::ByeSelection => ctx.elo_rank[unit],
            Rule::AirtightGroups
            | Rule::ScoreGap
            | Rule::Club
            | Rule::Nationality
            | Rule::Fold
            | Rule::EloGap => 0,
        }
    }

    /// A safe upper bound on the total units this rule can emit across one round's
    /// matching: (largest units on any single edge or bye) × (number of edges).
    pub(super) fn max_total_units(self, ctx: &Ctx) -> i128 {
        // The bye-selection rule fires on a single bye per round, not on every
        // edge, so its total is bounded by the largest rank (free_count − 1) once.
        if let Rule::ByeSelection = self {
            return (ctx.free_count - 1).max(0);
        }
        // Likewise, the bye-group rule fires once, bounded by the squared gap
        // between the lowest and highest score among free players.
        if let Rule::ByeGroup = self {
            return ctx.max_gap * ctx.max_gap;
        }
        // `AirtightGroups`, `Club` and `Nationality` only reach here when active —
        // `build` filters out the inactive ones (see its rule filter) — so no
        // `active` factor is needed on their bounds.
        let per_edge = match self {
            Rule::Rematch => 1,
            Rule::CupPrequalified => 1,
            Rule::AirtightGroups => ctx.max_mm_gap * ctx.max_mm_gap,
            Rule::ScoreGap => ctx.max_gap * ctx.max_gap,
            Rule::FloatRepeat => 2 * FLOAT_BASE, // two directions, each ≤ FLOAT_BASE
            // A descender and an ascender term, each a rank distance ≤ group_size − 1.
            Rule::FloaterSelection => 2 * (ctx.max_group - 1).max(0),
            // One unit per aligned board position that could clash: 1 for players,
            // the team size in team mode. Reading it off the *instance* rather
            // than assuming 1 is what keeps the ladder's separation exact when a
            // team edge can emit several units at once. Same bound for both
            // affiliation rules — a unit has as many nationalities as clubs.
            Rule::Club | Rule::Nationality => ctx.max_boards,
            // Two squared terms, each a rank deviation ≤ (group_size − 1)².
            Rule::Fold => 2 * (ctx.max_group - 1).max(0).pow(2),
            // The squared gap between the widest-separated free players.
            Rule::EloGap => ctx.max_elo_gap * ctx.max_elo_gap,
            Rule::ByeSelection | Rule::ByeGroup => unreachable!("handled above"),
        };
        per_edge * ctx.edges
    }
}

/// The cost ladder lives in `i128`, and its lexicographic separation only holds
/// while every multiplier and running total stays in range. Overflow would wrap
/// *silently* in release (the workspace sets no `overflow-checks`), producing a
/// non-lexicographic cost — a wrong pairing with no error, the exact failure the
/// derived-ladder design exists to rule out. So the ladder arithmetic goes
/// through these checked helpers, turning the (extreme) overflow into a loud,
/// precise abort instead of a silently miscosted draw. It is reachable only at
/// the edges: many hundreds of players combined with a very wide score/MacMahon
/// spread, or the counterfactual re-solve (`counterfactual::solve_stable`) near a thousand.
#[cold]
#[track_caller]
fn ladder_overflow() -> ! {
    panic!(
        "pairing weight ladder overflowed i128: the field is too large or the \
         score/MacMahon spread too wide for the exact lexicographic ladder. \
         This is a known scale limit; pairing was aborted rather than emit a \
         silently miscosted draw."
    );
}

#[track_caller]
pub(super) fn ladder_mul(a: i128, b: i128) -> i128 {
    a.checked_mul(b).unwrap_or_else(|| ladder_overflow())
}

#[track_caller]
pub(super) fn ladder_add(a: i128, b: i128) -> i128 {
    a.checked_add(b).unwrap_or_else(|| ladder_overflow())
}

/// Derive the priority multipliers from each rule's worst-case total units, given
/// in priority order (highest first). Bottom-up, `mult[i] = 1 + Σ_{j>i}
/// mult[j]·max_total[j]`, so one unit of rule `i` strictly exceeds the largest
/// possible sum of all lower-priority rules combined — a correct lexicographic
/// scalarization with no hand-tuned gaps.
pub(super) fn scale_ladder(max_total: &[i128]) -> Vec<i128> {
    let mut mult = vec![0i128; max_total.len()];
    let mut lower = 0i128; // Σ over the already-assigned lower-priority rules
    for i in (0..max_total.len()).rev() {
        mult[i] = ladder_add(1, lower);
        lower = ladder_add(lower, ladder_mul(mult[i], max_total[i]));
    }
    mult
}

/// Total edge weight for pairing `a` against `b`: `Σ mult[rule] · units`, over the
/// active rules for this mode. Used off the hot path (explanations, the
/// alternative-pairing search); the O(k²) cost-matrix fill uses the per-rule
/// [`accumulate_edge_rule`] instead.
pub(super) fn edge_cost(ctx: &Ctx, rules: &[Rule], mult: &[i128], a: UnitKey, b: UnitKey) -> i128 {
    rules
        .iter()
        .zip(mult)
        .map(|(rule, m)| m * rule.edge_units(ctx, a, b))
        .sum()
}

/// Add one rule's contribution to the (upper-triangle) real edges of the cost
/// matrix: `cost[i*vcount + j] += m · units(ctx, key_i, key_j)` for every `i < j`.
///
/// `units` is generic and each call site passes a closure over a *constant* rule,
/// so it monomorphizes to that rule's arm of [`Rule::edge_units`] with the enum
/// `match` folded away — the O(k²) inner loop carries no per-edge branch or call.
/// The dispatch on which rule happens once per rule (see the fill in
/// [`pair_round_weighted`](super::matching::pair_round_weighted)), not per edge. Only the upper triangle is written:
/// [`min_weight_perfect_matching`](crate::matching::min_weight_perfect_matching) reads the matrix as symmetric, taking just
/// `cost[i*n + j]` for `i < j`.
#[inline]
pub(super) fn accumulate_edge_rule<F: Fn(&Ctx, UnitKey, UnitKey) -> i128>(
    cost: &mut [i128],
    vcount: usize,
    k: usize,
    free: &[UnitKey],
    ctx: &Ctx,
    m: i128,
    units: F,
) {
    for i in 0..k {
        let base_i = i * vcount;
        let ki = free[i];
        for j in (i + 1)..k {
            cost[base_i + j] += m * units(ctx, ki, free[j]);
        }
    }
}

/// Total edge weight for giving `unit` the bye, over the active rules.
pub(super) fn bye_cost(ctx: &Ctx, rules: &[Rule], mult: &[i128], unit: UnitKey) -> i128 {
    rules
        .iter()
        .zip(mult)
        .map(|(rule, m)| m * rule.bye_units(ctx, unit))
        .sum()
}

/// Within-group fold placement of a unit: its rank in the score group (by
/// rating, descending) and the group's size.
#[derive(Clone, Copy)]
pub(super) struct FoldInfo {
    pub(super) rank: usize,
    pub(super) group_size: usize,
}

/// The rank a player of `rank` in a group of `group_size` should ideally meet:
/// top half folds onto bottom half.
fn ideal_rank(rank: usize, group_size: usize) -> usize {
    let half = group_size / 2;
    if rank < half {
        rank + half
    } else {
        rank - half
    }
}

/// Fold ranks for the `free` units, grouped by points and sorted within each
/// group by rating (descending; unrated = 1), ties broken by unit key for a
/// stable, reproducible ordering.
pub(super) fn fold_ranks(
    units: &TiSlice<UnitKey, PairingUnit>,
    free: &[UnitKey],
) -> TiVec<UnitKey, Option<FoldInfo>> {
    let mut groups: HashMap<HalfPoints, Vec<UnitKey>> = HashMap::new();
    for &k in free {
        groups.entry(units[k].points).or_default().push(k);
    }
    // `None` for a non-free unit.
    let mut info: TiVec<UnitKey, Option<FoldInfo>> = vec![None; units.len()].into();
    for group in groups.values_mut() {
        group.sort_by(|&x, &y| {
            units[y]
                .fold_rating()
                .cmp(&units[x].fold_rating())
                .then(x.cmp(&y))
        });
        let m = group.len();
        for (rank, &k) in group.iter().enumerate() {
            info[k] = Some(FoldInfo {
                rank,
                group_size: m,
            });
        }
    }
    info
}

#[cfg(test)]
mod tests;
