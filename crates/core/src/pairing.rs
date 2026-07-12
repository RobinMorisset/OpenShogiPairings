//! Round pairing.
//!
//! A round is modeled as a **minimum-weight perfect matching** over a complete
//! graph of the present players (solved by [`crate::matching`]). Each candidate
//! pairing is scored by a fixed set of rules; a rule emits a small number of
//! penalty *units*, and the edge weight is `Σ multiplier[rule] · units`. The
//! multipliers are **derived** from each rule's worst-case units per round rather
//! than hand-tuned, so one unit of any rule always outweighs the largest possible
//! sum of every lower-priority rule combined — i.e. the scalar weight is a correct
//! lexicographic ordering by rule priority, by construction. The rules, most
//! important first:
//!
//! 1. **No rematch / no repeat bye** — two players never meet twice, and no one
//!    takes the bye twice.
//! 2. **Bye in the lowest group** — the bye should go to the lowest-scoring free
//!    player, not (say) the tournament leader; the penalty grows with the
//!    *square* of the points gap to the lowest score among free players. A
//!    bye-only rule, sitting right below no-rematch so it's decided before any
//!    other pairing preference.
//! 3. **Airtight groups** (optional, off by default) — for the first N
//!    configured rounds, avoid pairing players with a different number of
//!    MacMahon points; the penalty grows with the *square* of the gap, same as
//!    the score-gap rule below. Costs nothing when unset.
//! 4. **Equal scores** — prefer opponents with the same number of points (each
//!    player's MacMahon starting points plus their victories); the penalty grows
//!    with the *square* of the points gap.
//! 5. **No repeated float** — avoid making a player an ascending floater (meeting
//!    someone with more points) or a descending floater (fewer points, or a
//!    bye) twice; the penalty fades with the number of rounds since the last such
//!    float.
//! 6. **Floater selection** — when a group has to pair across score groups, choose
//!    *who* floats: in classic Swiss, the descending floater should be the last
//!    (weakest) of the upper group and the ascending floater the first of the
//!    lower group; in median Swiss, both floaters should be the median of their
//!    respective group. The penalty rises with the
//!    distance from that ideal in-group rank.
//! 7. **Different clubs** — avoid pairing club-mates (ignored when a club is
//!    unknown).
//! 8. **Fold within a score group** — sort a group (equal points) by rating
//!    (unrated = 1), descending; the Nth player of the top half should meet the
//!    Nth of the bottom half, penalized by the *squared* deviation from that ideal.
//!
//! Priority lives in exactly one place — the order of [`Rule::ORDER`] — and the
//! separation between tiers is proven by construction (see [`scale_ladder`]), so
//! adding or reordering rules stays sound with no magic numbers to retune.
//!
//! [`pair_round_weighted`] is the real pairing path; the bye is modeled as a
//! phantom vertex.
//!
//! An ILP/CP-SAT backend is still planned (see TODO.md) for very large fields and
//! for formats needing hard constraints a plain matching can't express.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::elo::{estimate_elos, UNRATED_PRIOR_MEAN};
use crate::matching::{min_weight_perfect_matching, Weight};
use crate::player::Player;
use crate::round::{Board, PairingSource, Round};
use crate::scoring::{compute_scores, Scores};
use crate::settings::{FloaterStyle, TournamentSettings};

// --- Weighted matching ----------------------------------------------------

/// Numerator of the float-repeat penalty, divided by the number of rounds since
/// the player last floated the same way. Chosen with many small divisors so the
/// decay reads smoothly.
const FLOAT_BASE: i128 = 720;

/// Solve a minimum-weight perfect matching, picking the narrowest edge-weight
/// type that comfortably fits the cost matrix. Rule costs are built as `i128`
/// so the ladder's lexicographic multipliers can never overflow while scoring,
/// but most tournaments' actual ladders (few rules, modest gaps) fit easily in
/// `i32` or `i64` — narrower arithmetic the blossom solver runs faster with.
/// Only a ladder that genuinely needs `i128`'s headroom pays for it.
///
/// The `/ 16` margin covers the solver's internal doubling and its `MAX / 4`
/// "infinity" sentinel with room to spare.
fn solve_matching(cost: &[Vec<i128>]) -> Vec<usize> {
    let max = cost.iter().flatten().copied().max().unwrap_or(0);
    if max <= i32::MAX as i128 / 16 {
        min_weight_perfect_matching(&narrow::<i32>(cost))
    } else if max <= i64::MAX as i128 / 16 {
        min_weight_perfect_matching(&narrow::<i64>(cost))
    } else {
        min_weight_perfect_matching(cost)
    }
}

/// Convert an `i128` cost matrix down to a narrower [`Weight`] type. Panics if
/// a value doesn't fit — callers must check the bound first (see
/// [`solve_matching`]).
fn narrow<W>(cost: &[Vec<i128>]) -> Vec<Vec<W>>
where
    W: Weight + TryFrom<i128>,
    <W as TryFrom<i128>>::Error: std::fmt::Debug,
{
    cost.iter()
        .map(|row| row.iter().map(|&c| W::try_from(c).unwrap()).collect())
        .collect()
}

/// The pairing rules. The active subset and its priority order depend on the
/// mode (see [`active_rules`]); that ordering is the single source of truth for
/// priority, and the scalar multipliers are derived from it (see
/// [`scale_ladder`]).
#[derive(Clone, Copy)]
enum Rule {
    /// Never play the same opponent twice / never take the bye twice.
    Rematch,
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
    /// Fold within a score group (top half meets bottom half), by squared deviation.
    Fold,
    /// (ELO mode) Choose *who* takes the bye — the weakest present player by
    /// estimated ELO. A bye-only rule, sitting above [`Rule::EloGap`] (which is
    /// indifferent to the bye), so the sit-out is decided before the rest is
    /// optimized.
    ByeSelection,
    /// (ELO mode) Prefer opponents of equal estimated ELO; penalty grows with the
    /// square of the ELO gap. Replaces the whole Swiss score/float/fold family.
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
    ByeGroup,
    AirtightGroups,
    ScoreGap,
    FloatRepeat,
    FloaterSelection,
    Club,
    Fold,
    ByeSelection,
    EloGap,
}

/// The rules in effect, highest priority first, for the active mode. Swiss/
/// MacMahon is the default; the experimental ELO mode swaps the whole
/// score/float/fold/club family for a bye-selection rule and a squared-ELO-gap
/// rule, keeping only no-rematch above them.
fn active_rules(settings: &TournamentSettings) -> &'static [Rule] {
    const SWISS: [Rule; 8] = [
        Rule::Rematch,
        Rule::ByeGroup,
        Rule::AirtightGroups,
        Rule::ScoreGap,
        Rule::FloatRepeat,
        Rule::FloaterSelection,
        Rule::Club,
        Rule::Fold,
    ];
    const ELO: [Rule; 3] = [Rule::Rematch, Rule::ByeSelection, Rule::EloGap];
    if settings.elo_pairing_enabled {
        &ELO
    } else {
        &SWISS
    }
}

/// Everything the rules need to score an edge, plus the per-round quantities their
/// worst-case bounds (and hence multipliers) are derived from.
struct Ctx<'a> {
    scores: &'a Scores,
    by_player: &'a HashMap<Uuid, &'a Player>,
    fold: &'a HashMap<Uuid, FoldInfo>,
    round: u32,
    /// Which player each lower group sends up as its ascending floater.
    floater_style: FloaterStyle,
    /// Whether club protection applies this round (enabled and within its window).
    club_active: bool,
    /// Whether "airtight groups" applies this round (see
    /// [`TournamentSettings::airtight_groups_active`]).
    airtight_active: bool,
    /// Clubs exempt from protection, in normalized form (see
    /// [`TournamentSettings::normalize_club`]).
    exempt_clubs: &'a HashSet<String>,
    /// Edges in a perfect matching over the vertices (= vertices / 2).
    edges: i128,
    /// Largest points gap between any two vertices (bounds the score rule).
    max_gap: i128,
    /// Lowest points among the free players (the bye's target group).
    min_points: i128,
    /// Largest MacMahon-points gap between any two vertices (bounds the airtight
    /// groups rule).
    max_mm_gap: i128,
    /// Largest score-group size among the free players (bounds the fold rule).
    max_group: i128,
    /// Number of free players (bounds the bye-selection rule).
    free_count: i128,
    /// (ELO mode) Rounded estimated ELO per free player; empty in Swiss mode.
    elo: &'a HashMap<Uuid, i64>,
    /// (ELO mode) Ascending ELO rank per free player, 0 = weakest; empty in Swiss
    /// mode.
    elo_rank: &'a HashMap<Uuid, i128>,
    /// (ELO mode) Largest rounded-ELO gap among free players (bounds the ELO-gap
    /// rule).
    max_elo_gap: i128,
}

/// Float-repeat units for one player/direction: 0 if they never floated that way,
/// else `FLOAT_BASE` decayed by the rounds since (at least 1, so `≤ FLOAT_BASE`).
fn float_units(last: Option<u32>, round: u32) -> i128 {
    match last {
        Some(k) => FLOAT_BASE / (round - k) as i128, // k < round always
        None => 0,
    }
}

/// Floater-selection units for one floater: how far its in-group rank is from the
/// ideal position for its float direction. In classic Swiss, a descending floater
/// ideally sits last (weakest) in its group and an ascending floater first; in
/// median Swiss, both ideally sit at the median. 0 if the player has no fold info
/// (shouldn't happen for free players) or its group is a singleton.
fn floater_units(ctx: &Ctx, id: Uuid, descending: bool) -> i128 {
    let Some(f) = ctx.fold.get(&id) else {
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

impl Rule {
    /// The serializable identity of this rule, for explanations.
    fn id(self) -> RuleId {
        match self {
            Rule::Rematch => RuleId::Rematch,
            Rule::ByeGroup => RuleId::ByeGroup,
            Rule::AirtightGroups => RuleId::AirtightGroups,
            Rule::ScoreGap => RuleId::ScoreGap,
            Rule::FloatRepeat => RuleId::FloatRepeat,
            Rule::FloaterSelection => RuleId::FloaterSelection,
            Rule::Club => RuleId::Club,
            Rule::Fold => RuleId::Fold,
            Rule::ByeSelection => RuleId::ByeSelection,
            Rule::EloGap => RuleId::EloGap,
        }
    }

    /// Penalty units for pairing `a` against `b` (before the priority multiplier).
    fn edge_units(self, ctx: &Ctx, a: Uuid, b: Uuid) -> i128 {
        let sa = ctx.scores.get(&a);
        let sb = ctx.scores.get(&b);
        match self {
            // Rule 1: never play the same opponent twice.
            Rule::Rematch => i128::from(sa.opponents.contains(&b)),
            // Rule 2: bye-only rule, real boards are neutral.
            Rule::ByeGroup => 0,
            // Rule 3 (optional, first N rounds): forbid crossing MacMahon groups;
            // penalty is the square of the gap in MacMahon starting points.
            Rule::AirtightGroups => {
                if !ctx.airtight_active {
                    return 0;
                }
                let gap = (sa.macmahon as i128 - sb.macmahon as i128).abs();
                gap * gap
            }
            // Rule 3: prefer equal scores; penalty is the square of the gap.
            Rule::ScoreGap => {
                let gap = (sa.points as i128 - sb.points as i128).abs();
                gap * gap
            }
            // Rule 4: the lower-scored player floats up, the higher-scored down.
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
            // Rule 5: on a cross-group (float) edge, prefer the right floaters —
            // classic Swiss wants the weakest of the upper group down and the
            // first of the lower group up; median Swiss wants the median of
            // each group instead. Same-group edges aren't floats, so no penalty.
            Rule::FloaterSelection => match sa.points.cmp(&sb.points) {
                Ordering::Equal => 0,
                _ => {
                    let (descender, ascender) = if sa.points > sb.points {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    floater_units(ctx, descender, true) + floater_units(ctx, ascender, false)
                }
            },
            // Rule 6: avoid pairing club-mates — but only when protection is active
            // this round, ignoring unknown clubs and clubs on the exempt list. Club
            // names are matched case-insensitively.
            Rule::Club => {
                if !ctx.club_active {
                    return 0;
                }
                match (&ctx.by_player[&a].club, &ctx.by_player[&b].club) {
                    (Some(ca), Some(cb)) => {
                        let na = TournamentSettings::normalize_club(ca);
                        let same = na == TournamentSettings::normalize_club(cb);
                        i128::from(same && !ctx.exempt_clubs.contains(&na))
                    }
                    _ => 0,
                }
            }
            // Rule 7: fold within a score group — squared deviation from the ideal
            // fold. Squaring (rather than |·|) spreads an unavoidable deviation across
            // boards instead of dumping it all on one, so no single player faces an
            // opponent far from the fold's intent — and it matches the squared
            // ScoreGap / EloGap rules. See `docs/swiss-fold.md`.
            Rule::Fold => {
                if sa.points != sb.points {
                    return 0;
                }
                match (ctx.fold.get(&a), ctx.fold.get(&b)) {
                    (Some(fa), Some(fb)) => {
                        let ia = ideal_rank(fa.rank, fa.group_size) as i128;
                        let ib = ideal_rank(fb.rank, fb.group_size) as i128;
                        let da = fb.rank as i128 - ia;
                        let db = fa.rank as i128 - ib;
                        da * da + db * db
                    }
                    _ => 0,
                }
            }
            // Bye selection acts only on the bye edge; a real board is neutral.
            Rule::ByeSelection => 0,
            // ELO mode: prefer equal estimated ELO; penalty is the squared gap.
            Rule::EloGap => {
                let ga = ctx.elo.get(&a).copied().unwrap_or(0);
                let gb = ctx.elo.get(&b).copied().unwrap_or(0);
                let gap = (ga - gb) as i128;
                gap * gap
            }
        }
    }

    /// Penalty units for giving `player` the bye (before the priority multiplier).
    /// A bye repeats the rematch rule (never bye twice) and counts as a downfloat.
    fn bye_units(self, ctx: &Ctx, player: Uuid) -> i128 {
        let s = ctx.scores.get(&player);
        match self {
            Rule::Rematch => i128::from(s.had_bye),
            // The bye should go to the lowest score group; penalty is the square
            // of the gap to the lowest score among free players.
            Rule::ByeGroup => {
                let gap = s.points as i128 - ctx.min_points;
                gap * gap
            }
            Rule::FloatRepeat => float_units(s.last_descended, ctx.round),
            // A bye is a downfloat, so prefer the weakest of the group (classic)
            // or its median (median Swiss) to take it.
            Rule::FloaterSelection => floater_units(ctx, player, true),
            // ELO mode: the weakest present player (lowest ELO rank) takes the bye.
            Rule::ByeSelection => ctx.elo_rank.get(&player).copied().unwrap_or(0),
            Rule::AirtightGroups | Rule::ScoreGap | Rule::Club | Rule::Fold | Rule::EloGap => 0,
        }
    }

    /// A safe upper bound on the total units this rule can emit across one round's
    /// matching: (largest units on any single edge or bye) × (number of edges).
    fn max_total_units(self, ctx: &Ctx) -> i128 {
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
        let per_edge = match self {
            Rule::Rematch => 1,
            // 0 when off or out of its round window — no wasted tier.
            Rule::AirtightGroups => {
                i128::from(ctx.airtight_active) * ctx.max_mm_gap * ctx.max_mm_gap
            }
            Rule::ScoreGap => ctx.max_gap * ctx.max_gap,
            Rule::FloatRepeat => 2 * FLOAT_BASE, // two directions, each ≤ FLOAT_BASE
            // A descender and an ascender term, each a rank distance ≤ group_size − 1.
            Rule::FloaterSelection => 2 * (ctx.max_group - 1).max(0),
            Rule::Club => i128::from(ctx.club_active), // 0 when off — no wasted tier
            // Two squared terms, each a rank deviation ≤ (group_size − 1)².
            Rule::Fold => 2 * (ctx.max_group - 1).max(0).pow(2),
            // The squared gap between the widest-separated free players.
            Rule::EloGap => ctx.max_elo_gap * ctx.max_elo_gap,
            Rule::ByeSelection | Rule::ByeGroup => unreachable!("handled above"),
        };
        per_edge * ctx.edges
    }
}

/// Derive the priority multipliers from each rule's worst-case total units, given
/// in priority order (highest first). Bottom-up, `mult[i] = 1 + Σ_{j>i}
/// mult[j]·max_total[j]`, so one unit of rule `i` strictly exceeds the largest
/// possible sum of all lower-priority rules combined — a correct lexicographic
/// scalarization with no hand-tuned gaps.
fn scale_ladder(max_total: &[i128]) -> Vec<i128> {
    let mut mult = vec![0i128; max_total.len()];
    let mut lower = 0i128; // Σ over the already-assigned lower-priority rules
    for i in (0..max_total.len()).rev() {
        mult[i] = 1 + lower;
        lower += mult[i] * max_total[i];
    }
    mult
}

/// Total edge weight for pairing `a` against `b`: `Σ mult[rule] · units`, over the
/// active rules for this mode.
fn edge_cost(ctx: &Ctx, rules: &[Rule], mult: &[i128], a: Uuid, b: Uuid) -> i128 {
    rules
        .iter()
        .zip(mult)
        .map(|(rule, m)| m * rule.edge_units(ctx, a, b))
        .sum()
}

/// Total edge weight for giving `player` the bye, over the active rules.
fn bye_cost(ctx: &Ctx, rules: &[Rule], mult: &[i128], player: Uuid) -> i128 {
    rules
        .iter()
        .zip(mult)
        .map(|(rule, m)| m * rule.bye_units(ctx, player))
        .sum()
}

/// Within-group fold placement of a player: its rank in the score group (by
/// rating, descending) and the group's size.
struct FoldInfo {
    rank: usize,
    group_size: usize,
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

/// Fold ranks for the `free` players, grouped by points and sorted within each
/// group by rating (descending; unrated = 1), ties broken by tournament number
/// for a stable, reproducible ordering.
fn fold_ranks(
    scores: &Scores,
    by_player: &HashMap<Uuid, &Player>,
    free: &[Uuid],
) -> HashMap<Uuid, FoldInfo> {
    // Rating for seeding; unrated players count as 1 (per the fold rule).
    let rating = |id: &Uuid| by_player[id].rating.unwrap_or(1);
    let tnum = |id: &Uuid| by_player[id].tournament_id.unwrap_or(u32::MAX);
    let mut groups: HashMap<u32, Vec<Uuid>> = HashMap::new();
    for &id in free {
        groups.entry(scores.get(&id).points).or_default().push(id);
    }
    let mut info = HashMap::new();
    for group in groups.values_mut() {
        group.sort_by(|x, y| {
            rating(y)
                .cmp(&rating(x))
                .then_with(|| tnum(x).cmp(&tnum(y)))
        });
        let m = group.len();
        for (rank, id) in group.iter().enumerate() {
            info.insert(
                *id,
                FoldInfo {
                    rank,
                    group_size: m,
                },
            );
        }
    }
    info
}

// --- Pairing model (shared by pairing and explanation) --------------------

/// One round's Swiss scoring context, built once from the pairing inputs and
/// reused for both pairing and explanation. It owns the derived per-round data
/// (scores, fold ranks, ELO estimates, the multiplier ladder) and lends a [`Ctx`]
/// on demand, so an explanation is scored against the *identical* construction
/// the pairing used — no risk of the two drifting apart.
struct PairingModel<'a> {
    scores: Scores,
    by_player: HashMap<Uuid, &'a Player>,
    fold: HashMap<Uuid, FoldInfo>,
    exempt_clubs: HashSet<String>,
    elo: HashMap<Uuid, i64>,
    elo_rank: HashMap<Uuid, i128>,
    round: u32,
    floater_style: FloaterStyle,
    club_active: bool,
    airtight_active: bool,
    edges: i128,
    max_gap: i128,
    min_points: i128,
    max_mm_gap: i128,
    max_group: i128,
    free_count: i128,
    max_elo_gap: i128,
    rules: &'static [Rule],
    mult: Vec<i128>,
}

impl<'a> PairingModel<'a> {
    /// Build the model for the given `free` set (the players the matching will
    /// pair). `need_phantom` is whether a bye vertex participates, so the edge
    /// count — and hence the derived multipliers — match the matching that was or
    /// will be solved.
    fn build(
        number: u32,
        players: &'a [Player],
        settings: &TournamentSettings,
        completed_rounds: &[Round],
        free: &[Uuid],
        need_phantom: bool,
    ) -> Self {
        let scores = compute_scores(players, settings, completed_rounds);
        let by_player: HashMap<Uuid, &Player> = players.iter().map(|p| (p.id, p)).collect();
        let fold = fold_ranks(&scores, &by_player, free);

        let (mut lo, mut hi) = (u32::MAX, 0u32);
        let (mut mm_lo, mut mm_hi) = (u32::MAX, 0u32);
        for &id in free {
            let s = scores.get(&id);
            lo = lo.min(s.points);
            hi = hi.max(s.points);
            mm_lo = mm_lo.min(s.macmahon);
            mm_hi = mm_hi.max(s.macmahon);
        }
        let exempt_clubs = settings.exempt_clubs_normalized();

        // ELO mode: a live estimate per free player (rounded), its ascending rank
        // (0 = weakest, for the bye-selection rule), and the widest gap (for the
        // ladder bound). All empty / zero in Swiss mode.
        let (elo, elo_rank, max_elo_gap) = if settings.elo_pairing_enabled {
            let est = estimate_elos(players, settings, completed_rounds);
            let elo: HashMap<Uuid, i64> = free
                .iter()
                .map(|&id| {
                    let e = est.get(&id).copied().unwrap_or(UNRATED_PRIOR_MEAN);
                    (id, e.round() as i64)
                })
                .collect();
            let tnum = |id: &Uuid| by_player[id].tournament_id.unwrap_or(u32::MAX);
            let mut order = free.to_vec();
            order.sort_by(|x, y| elo[x].cmp(&elo[y]).then_with(|| tnum(x).cmp(&tnum(y))));
            let elo_rank: HashMap<Uuid, i128> = order
                .iter()
                .enumerate()
                .map(|(rank, id)| (*id, rank as i128))
                .collect();
            let (elo_lo, elo_hi) = elo
                .values()
                .fold((i64::MAX, i64::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
            (elo, elo_rank, (elo_hi - elo_lo).max(0) as i128)
        } else {
            (HashMap::new(), HashMap::new(), 0)
        };

        let k = free.len();
        let vcount = k + usize::from(need_phantom);
        let max_group = fold.values().map(|f| f.group_size).max().unwrap_or(0) as i128;
        let rules = active_rules(settings);

        let mut model = PairingModel {
            scores,
            by_player,
            fold,
            exempt_clubs,
            elo,
            elo_rank,
            round: number,
            floater_style: settings.floater_style,
            club_active: settings.club_protection_active(number),
            airtight_active: settings.airtight_groups_active(number),
            edges: (vcount / 2) as i128,
            max_gap: hi.saturating_sub(lo) as i128,
            min_points: lo as i128,
            max_mm_gap: mm_hi.saturating_sub(mm_lo) as i128,
            max_group,
            free_count: k as i128,
            max_elo_gap,
            rules,
            mult: Vec::new(),
        };
        // The multipliers depend on the per-rule bounds, which need a Ctx — so
        // build the ladder in a second pass, once the rest of the model exists.
        let max_total: Vec<i128> = {
            let ctx = model.ctx();
            rules.iter().map(|r| r.max_total_units(&ctx)).collect()
        };
        model.mult = scale_ladder(&max_total);
        model
    }

    /// A scoring context borrowing this model's owned data.
    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            scores: &self.scores,
            by_player: &self.by_player,
            fold: &self.fold,
            round: self.round,
            floater_style: self.floater_style,
            club_active: self.club_active,
            airtight_active: self.airtight_active,
            exempt_clubs: &self.exempt_clubs,
            edges: self.edges,
            max_gap: self.max_gap,
            min_points: self.min_points,
            max_mm_gap: self.max_mm_gap,
            max_group: self.max_group,
            free_count: self.free_count,
            elo: &self.elo,
            elo_rank: &self.elo_rank,
            max_elo_gap: self.max_elo_gap,
        }
    }

    /// Scalar edge weight for pairing `a` against `b`.
    fn edge_cost(&self, a: Uuid, b: Uuid) -> i128 {
        edge_cost(&self.ctx(), self.rules, &self.mult, a, b)
    }

    /// Scalar edge weight for giving `player` the bye.
    fn bye_cost(&self, player: Uuid) -> i128 {
        bye_cost(&self.ctx(), self.rules, &self.mult, player)
    }

    /// Per-rule penalty units (pre-multiplier) for pairing `a` against `b`, in
    /// priority order (aligned with [`Self::rules`]).
    fn edge_units(&self, a: Uuid, b: Uuid) -> Vec<i128> {
        let ctx = self.ctx();
        self.rules
            .iter()
            .map(|r| r.edge_units(&ctx, a, b))
            .collect()
    }

    /// Per-rule penalty units (pre-multiplier) for giving `player` the bye.
    fn bye_units(&self, player: Uuid) -> Vec<i128> {
        let ctx = self.ctx();
        self.rules
            .iter()
            .map(|r| r.bye_units(&ctx, player))
            .collect()
    }

    fn rules(&self) -> &'static [Rule] {
        self.rules
    }
}

// --- Explanation ----------------------------------------------------------

/// One rule's contribution to a single board (or the bye): the rule and the
/// penalty *units* it emitted, before the priority multiplier.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
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
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct BoardLedger {
    pub player1: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub player2: Option<Uuid>,
    pub contributions: Vec<RuleContribution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub binding: Option<RuleId>,
}

/// How often one rule had to be relaxed across a whole round, and the total units.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct RuleTotal {
    pub rule: RuleId,
    pub boards: u32,
    #[ts(type = "number")]
    pub units: i64,
}

/// A human-facing explanation of one round's Swiss pairings: a per-board ledger,
/// the bye's ledger, and the per-rule round totals (in priority order).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct RoundExplanation {
    pub round: u32,
    pub boards: Vec<BoardLedger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bye: Option<BoardLedger>,
    pub report: Vec<RuleTotal>,
}

/// Turn a per-rule unit vector (aligned with `rules`, priority order) into a
/// ledger: keep only the rules that fired, and note the highest-priority one.
fn ledger(player1: Uuid, player2: Option<Uuid>, rules: &[Rule], units: &[i128]) -> BoardLedger {
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
pub fn explain_pairing(
    number: u32,
    players: &[Player],
    settings: &TournamentSettings,
    completed_rounds: &[Round],
    swiss_boards: &[(Uuid, Uuid)],
    bye: Option<Uuid>,
) -> RoundExplanation {
    // The Swiss free set the round was paired from: both players of every Swiss
    // board, plus the bye. With a bye the count is odd, so a phantom participates.
    let mut free: Vec<Uuid> = swiss_boards.iter().flat_map(|&(a, b)| [a, b]).collect();
    if let Some(b) = bye {
        free.push(b);
    }
    let need_phantom = bye.is_some();
    let model = PairingModel::build(
        number,
        players,
        settings,
        completed_rounds,
        &free,
        need_phantom,
    );
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

// --- Counterfactual ("why not pair A and B?") -----------------------------

/// Sentinel vertex standing in for the bye in a matching. Real player ids are v4
/// UUIDs, never the nil UUID, so this can't collide with a player.
const PHANTOM: Uuid = Uuid::nil();

/// Normalized (order-independent) edge, so `(a, b)` and `(b, a)` are one key.
fn unord_pair(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
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
    /// The player sat this round out (absent or the bye is not being probed).
    Absent,
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
}

/// A ring of players who must reshuffle to honour the probe — a vertex-disjoint
/// alternating cycle of (baseline △ counterfactual), ordered for storytelling.
/// The bye appears as the nil sentinel so clients can render it as "(bye)".
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct AffectedCycle {
    pub players: Vec<Uuid>,
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
/// [`Rule`]: each edge costs `real_cost · (edges + 1) + (edge not in baseline)`.
/// Since `real_cost` is already the correct lexicographic scalar for the real
/// rules and the stability term is at most `edges < edges + 1`, this is exactly
/// the lexicographic order `(real rules, then boards changed)` — the same
/// ordering appending a lowest-priority rule to [`scale_ladder`] would give.
///
/// A forbidden edge is priced above any whole matching that avoids it (`max ·
/// edges + 1`), so it is never chosen when an alternative perfect matching
/// exists — which it always does on a complete graph of ≥ 4 vertices.
fn solve_stable(
    model: &PairingModel,
    verts: &[Uuid],
    baseline: &HashSet<(Uuid, Uuid)>,
    forbidden: &HashSet<(Uuid, Uuid)>,
) -> Vec<(Uuid, Uuid)> {
    let n = verts.len();
    if n < 2 {
        return Vec::new();
    }
    let stab = (n / 2) as i128 + 1; // strictly above the largest stability total
    let base = |a: Uuid, b: Uuid| -> i128 {
        if a == PHANTOM {
            model.bye_cost(b)
        } else if b == PHANTOM {
            model.bye_cost(a)
        } else {
            model.edge_cost(a, b)
        }
    };
    let mut cost = vec![vec![0i128; n]; n];
    let mut max_c = 0i128;
    for i in 0..n {
        for j in (i + 1)..n {
            let stray = i128::from(!baseline.contains(&unord_pair(verts[i], verts[j])));
            let c = base(verts[i], verts[j]) * stab + stray;
            cost[i][j] = c;
            cost[j][i] = c;
            max_c = max_c.max(c);
        }
    }
    if !forbidden.is_empty() {
        // Above the total of any perfect matching that avoids the edge.
        let prohibitive = max_c * (n as i128 / 2) + 1;
        for i in 0..n {
            for j in (i + 1)..n {
                if forbidden.contains(&unord_pair(verts[i], verts[j])) {
                    cost[i][j] = prohibitive;
                    cost[j][i] = prohibitive;
                }
            }
        }
    }
    let mate = solve_matching(&cost);
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
    m0: &HashSet<(Uuid, Uuid)>,
    m1: &HashSet<(Uuid, Uuid)>,
) -> Vec<AffectedCycle> {
    // Adjacency over the changed edges. Every vertex in the symmetric difference
    // has exactly one edge from each matching, so its degree here is exactly 2.
    let mut adj: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for &(a, b) in m0.symmetric_difference(m1) {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    }

    let mut visited: HashSet<Uuid> = HashSet::new();
    let mut cycles = Vec::new();
    let mut starts: Vec<Uuid> = adj.keys().copied().collect();
    starts.sort(); // deterministic cycle order
    for start in starts {
        if visited.contains(&start) {
            continue;
        }
        let mut order = Vec::new();
        let mut cur = start;
        let mut prev: Option<Uuid> = None;
        loop {
            visited.insert(cur);
            order.push(cur);
            let nbrs = &adj[&cur];
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
fn baseline_matching<'a>(
    number: u32,
    players: &'a [Player],
    settings: &TournamentSettings,
    completed_rounds: &[Round],
    swiss_boards: &[(Uuid, Uuid)],
    bye: Option<Uuid>,
) -> (PairingModel<'a>, Vec<Uuid>, bool, HashSet<(Uuid, Uuid)>) {
    let mut free: Vec<Uuid> = swiss_boards.iter().flat_map(|&(x, y)| [x, y]).collect();
    if let Some(p) = bye {
        free.push(p);
    }
    let need_phantom = bye.is_some();
    let model = PairingModel::build(
        number,
        players,
        settings,
        completed_rounds,
        &free,
        need_phantom,
    );
    let mut m0: HashSet<(Uuid, Uuid)> = swiss_boards
        .iter()
        .map(|&(x, y)| unord_pair(x, y))
        .collect();
    if let Some(p) = bye {
        m0.insert(unord_pair(p, PHANTOM));
    }
    (model, free, need_phantom, m0)
}

/// Diff the confirmed matching `m0` against the counterfactual `m1` into a
/// [`Counterfactual`]: the net per-rule cost, the affected rings, and the new
/// boards as ledgers.
fn diff_matchings(
    model: &PairingModel,
    m0: &HashSet<(Uuid, Uuid)>,
    m1: &HashSet<(Uuid, Uuid)>,
) -> Counterfactual {
    let rules = model.rules();
    let units_of = |e: &(Uuid, Uuid)| -> Vec<i128> {
        let (x, y) = *e;
        if x == PHANTOM {
            model.bye_units(y)
        } else if y == PHANTOM {
            model.bye_units(x)
        } else {
            model.edge_units(x, y)
        }
    };
    let ledger_of = |e: &(Uuid, Uuid)| -> BoardLedger {
        let (x, y) = *e;
        if x == PHANTOM {
            ledger(y, None, rules, &model.bye_units(y))
        } else if y == PHANTOM {
            ledger(x, None, rules, &model.bye_units(x))
        } else {
            ledger(x, Some(y), rules, &model.edge_units(x, y))
        }
    };

    // Net per-rule delta: added boards contribute +units, removed boards −units;
    // boards common to both matchings cancel.
    let mut delta: HashMap<RuleId, i64> = HashMap::new();
    for e in m1.difference(m0) {
        for (r, &u) in rules.iter().zip(&units_of(e)) {
            *delta.entry(r.id()).or_insert(0) += u as i64;
        }
    }
    for e in m0.difference(m1) {
        for (r, &u) in rules.iter().zip(&units_of(e)) {
            *delta.entry(r.id()).or_insert(0) -= u as i64;
        }
    }
    let cost_delta: Vec<RuleDelta> = rules
        .iter()
        .filter_map(|r| {
            let u = delta.get(&r.id()).copied().unwrap_or(0);
            (u != 0).then_some(RuleDelta {
                rule: r.id(),
                units: u,
            })
        })
        .collect();

    // The new boards (added edges), sorted for a stable order, as ledgers.
    let mut added: Vec<(Uuid, Uuid)> = m1.difference(m0).copied().collect();
    added.sort();
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
#[allow(clippy::too_many_arguments)]
pub fn counterfactual_force(
    number: u32,
    players: &[Player],
    settings: &TournamentSettings,
    completed_rounds: &[Round],
    swiss_boards: &[(Uuid, Uuid)],
    bye: Option<Uuid>,
    a: Uuid,
    b: Uuid,
) -> Counterfactual {
    let (model, free, need_phantom, m0) = baseline_matching(
        number,
        players,
        settings,
        completed_rounds,
        swiss_boards,
        bye,
    );

    // Re-solve everyone but the forced pair (phantom still in play if there is a
    // bye), then add the forced edge back for the full counterfactual matching.
    let mut verts: Vec<Uuid> = free.iter().copied().filter(|&v| v != a && v != b).collect();
    if need_phantom {
        verts.push(PHANTOM);
    }
    let no_forbidden = HashSet::new();
    let mut m1: HashSet<(Uuid, Uuid)> = solve_stable(&model, &verts, &m0, &no_forbidden)
        .into_iter()
        .collect();
    m1.insert(unord_pair(a, b));

    diff_matchings(&model, &m0, &m1)
}

/// Explain why the engine paired `a`–`b` rather than something else: forbid that
/// edge, re-solve the whole free set with a stability tie-break toward the
/// confirmed pairing, and diff. If `a`–`b` wasn't the engine's choice anyway, the
/// diff is empty.
#[allow(clippy::too_many_arguments)]
pub fn counterfactual_forbid(
    number: u32,
    players: &[Player],
    settings: &TournamentSettings,
    completed_rounds: &[Round],
    swiss_boards: &[(Uuid, Uuid)],
    bye: Option<Uuid>,
    a: Uuid,
    b: Uuid,
) -> Counterfactual {
    let (model, free, need_phantom, m0) = baseline_matching(
        number,
        players,
        settings,
        completed_rounds,
        swiss_boards,
        bye,
    );

    let mut verts = free.clone();
    if need_phantom {
        verts.push(PHANTOM);
    }
    let forbidden: HashSet<(Uuid, Uuid)> = [unord_pair(a, b)].into_iter().collect();
    let m1: HashSet<(Uuid, Uuid)> = solve_stable(&model, &verts, &m0, &forbidden)
        .into_iter()
        .collect();

    diff_matchings(&model, &m0, &m1)
}

/// Pair the `present` players by minimizing the total rule penalty, honoring
/// referee-forced boards and a forced bye. This is the real pairing path used by
/// [`crate::Tournament::confirm_round`]; the rules and their priority are
/// described in the module docs.
///
/// Preconditions (validated by the caller): every forced player is present and
/// appears at most once, and with a forced bye the free players number even.
pub fn pair_round_weighted(
    number: u32,
    players: &[Player],
    settings: &TournamentSettings,
    completed_rounds: &[Round],
    present: &[Uuid],
    forced_boards: &[Board],
    forced_bye: Option<Uuid>,
) -> Round {
    let scores = compute_scores(players, settings, completed_rounds);
    // The float frozen onto each board: points(player1) − points(player2) now.
    let diff = |p1: Uuid, p2: Uuid| scores.points(&p1) as i32 - scores.points(&p2) as i32;

    let mut placed: HashSet<Uuid> = HashSet::new();
    for board in forced_boards {
        placed.insert(board.player1);
        placed.insert(board.player2);
    }
    if let Some(bye) = forced_bye {
        placed.insert(bye);
    }
    let free: Vec<Uuid> = present
        .iter()
        .copied()
        .filter(|id| !placed.contains(id))
        .collect();

    // A phantom vertex absorbs the bye when an odd number of players remain and
    // none was forced; whoever the matching pairs with it sits out.
    let need_phantom = forced_bye.is_none() && free.len() % 2 == 1;
    let k = free.len();
    let vcount = k + usize::from(need_phantom);

    let mut boards: Vec<Board> = forced_boards
        .iter()
        .map(|b| {
            Board::pending(
                b.player1,
                b.player2,
                Some(diff(b.player1, b.player2)),
                PairingSource::Forced,
            )
        })
        .collect();
    let mut bye = forced_bye;

    if vcount >= 2 {
        // The pairing model owns the per-round scoring context and the derived
        // multiplier ladder; the same construction backs `explain_pairing`.
        let model = PairingModel::build(
            number,
            players,
            settings,
            completed_rounds,
            &free,
            need_phantom,
        );

        let mut cost = vec![vec![0i128; vcount]; vcount];
        for i in 0..k {
            for j in (i + 1)..k {
                let c = model.edge_cost(free[i], free[j]);
                cost[i][j] = c;
                cost[j][i] = c;
            }
        }
        if need_phantom {
            let p = k;
            for i in 0..k {
                let c = model.bye_cost(free[i]);
                cost[i][p] = c;
                cost[p][i] = c;
            }
        }
        let mate = solve_matching(&cost);
        let mut seen = vec![false; vcount];
        for i in 0..vcount {
            if seen[i] {
                continue;
            }
            let j = mate[i];
            seen[i] = true;
            seen[j] = true;
            if need_phantom && (i == k || j == k) {
                let real = if i == k { j } else { i };
                bye = Some(free[real]);
            } else {
                boards.push(Board::pending(
                    free[i],
                    free[j],
                    Some(diff(free[i], free[j])),
                    PairingSource::Swiss,
                ));
            }
        }
    } else if k == 1 {
        // A lone leftover with no phantom (only if a bye was forced elsewhere,
        // which the caller prevents). Defensive: sit them out.
        bye = Some(free[0]);
    }

    Round {
        number,
        boards,
        bye,
        absent: Vec::new(),
        completed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::Winner;
    use crate::settings::MacMahonThreshold;

    // --- solve_matching's width dispatch -----------------------------------

    #[test]
    fn solve_matching_matches_i128_regardless_of_chosen_width() {
        // One instance per width tier: small values (fits i32), values above
        // i32::MAX/16 but below i64::MAX/16 (fits i64), and values above that
        // (needs the i128 fallback). All three should agree with a direct
        // i128 solve on the same instance.
        let small = vec![
            vec![0, 1, 10, 10],
            vec![1, 0, 10, 10],
            vec![10, 10, 0, 1],
            vec![10, 10, 1, 0],
        ];
        let medium_unit = i32::MAX as i128 / 16 + 1;
        let medium = vec![
            vec![0, medium_unit, 10, 10],
            vec![medium_unit, 0, 10, 10],
            vec![10, 10, 0, medium_unit],
            vec![10, 10, medium_unit, 0],
        ];
        let huge_unit = i64::MAX as i128 / 16 + 1;
        let huge = vec![
            vec![0, huge_unit, 10, 10],
            vec![huge_unit, 0, 10, 10],
            vec![10, 10, 0, huge_unit],
            vec![10, 10, huge_unit, 0],
        ];
        for cost in [small, medium, huge] {
            assert_eq!(solve_matching(&cost), min_weight_perfect_matching(&cost));
        }
    }

    // --- Weighted pairing -------------------------------------------------

    fn player(tid: u32, rating: Option<u32>, club: Option<&str>) -> Player {
        Player {
            id: Uuid::new_v4(),
            tournament_id: Some(tid),
            last_name: format!("P{tid}"),
            first_name: String::new(),
            rating,
            grade: None,
            fesa_games: None,
            nationality: None,
            club: club.map(|c| c.to_string()),
            eligible: false,
            adjustments: Vec::new(),
        }
    }

    fn completed_round(number: u32, boards: &[(Uuid, Uuid, Winner)], bye: Option<Uuid>) -> Round {
        Round {
            number,
            boards: boards
                .iter()
                .map(|&(a, b, w)| Board {
                    result: Some(w),
                    ..Board::pending(a, b, None, PairingSource::Swiss)
                })
                .collect(),
            bye,
            absent: Vec::new(),
            completed: true,
        }
    }

    fn unord(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
        if a < b {
            (a, b)
        } else {
            (b, a)
        }
    }

    fn board_pairs(round: &Round) -> HashSet<(Uuid, Uuid)> {
        round
            .boards
            .iter()
            .map(|b| unord(b.player1, b.player2))
            .collect()
    }

    #[test]
    fn weighted_pairs_by_score_and_avoids_rematch() {
        let p: Vec<Player> = (1..=4)
            .map(|i| player(i, Some(2000 - i * 10), None))
            .collect();
        // Round 1: p0 beat p1, p2 beat p3. Winners now have 1 victory, losers 0.
        let r1 = completed_round(
            1,
            &[
                (p[0].id, p[1].id, Winner::Player1),
                (p[2].id, p[3].id, Winner::Player1),
            ],
            None,
        );
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();

        let round = pair_round_weighted(
            2,
            &p,
            &TournamentSettings::default(),
            &[r1],
            &present,
            &[],
            None,
        );

        assert_eq!(round.bye, None);
        assert_eq!(round.boards.len(), 2);
        let pairs = board_pairs(&round);
        // Same-score, no rematch: winners together, losers together.
        assert!(pairs.contains(&unord(p[0].id, p[2].id)));
        assert!(pairs.contains(&unord(p[1].id, p[3].id)));
    }

    #[test]
    fn weighted_avoids_repeat_bye() {
        let p: Vec<Player> = (1..=3).map(|i| player(i, Some(1500), None)).collect();
        // Round 1: p0 beat p1; p2 took the bye (so p0 and p2 have 1 victory).
        let r1 = completed_round(1, &[(p[0].id, p[1].id, Winner::Player1)], Some(p[2].id));
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();

        let round = pair_round_weighted(
            2,
            &p,
            &TournamentSettings::default(),
            &[r1],
            &present,
            &[],
            None,
        );

        // p2 already had a bye, so it must fall elsewhere; giving it to p1 also
        // leaves the same-score board p0 vs p2.
        assert_eq!(round.bye, Some(p[1].id));
        assert_eq!(
            board_pairs(&round),
            HashSet::from([unord(p[0].id, p[2].id)])
        );
    }

    #[test]
    fn weighted_gives_the_bye_to_the_lowest_group() {
        // 5 same-rated players: round 1 pairs p0-p1 and p2-p3 (winners p0, p2);
        // p4 takes the bye. Going into round 2: p0, p2, p4 lead on 1 point, p1
        // and p3 trail on 0. p4 already had the bye, so it must fall to p1 or
        // p3 — never to one of the two leaders, even though that could look
        // like a perfectly valid matching otherwise.
        let p: Vec<Player> = (1..=5).map(|i| player(i, Some(1500), None)).collect();
        let r1 = completed_round(
            1,
            &[
                (p[0].id, p[1].id, Winner::Player1),
                (p[2].id, p[3].id, Winner::Player1),
            ],
            Some(p[4].id),
        );
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();

        let round = pair_round_weighted(
            2,
            &p,
            &TournamentSettings::default(),
            &[r1],
            &present,
            &[],
            None,
        );

        let bye = round.bye.expect("odd field needs a bye");
        assert!(bye == p[1].id || bye == p[3].id, "bye went to a leader");
    }

    #[test]
    fn pairing_freezes_the_points_diff_on_each_board() {
        // After round 1 (A, C on 1 point; B, D on 0), force A vs D in round 2.
        // The board should record the float A had going in: 1 − 0 = 1.
        let p: Vec<Player> = (1..=4)
            .map(|i| player(i, Some(2000 - i * 10), None))
            .collect();
        let r1 = completed_round(
            1,
            &[
                (p[0].id, p[1].id, Winner::Player1), // A beats B
                (p[2].id, p[3].id, Winner::Player1), // C beats D
            ],
            None,
        );
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();
        // A (1 pt) vs D (0 pt), forced.
        let forced = vec![Board::pending(p[0].id, p[3].id, None, PairingSource::Swiss)];

        let round = pair_round_weighted(
            2,
            &p,
            &TournamentSettings::default(),
            &[r1],
            &present,
            &forced,
            None,
        );

        // Every board carries a frozen float, and the forced A-vs-D board's is +1.
        assert!(round.boards.iter().all(|b| b.points_diff.is_some()));
        let ad = round
            .boards
            .iter()
            .find(|b| b.player1 == p[0].id && b.player2 == p[3].id)
            .expect("forced board present");
        assert_eq!(ad.points_diff, Some(1));
    }

    #[test]
    fn macmahon_points_group_the_pairings() {
        // Round 1, everyone on 0 victories. With no MacMahon the four form one
        // score group and the fold pairs p0-p2 / p1-p3. A 1500 threshold splits
        // them into a top group {p0,p1} and a bottom group {p2,p3}, and rule 2
        // (equal points) keeps the groups apart instead.
        let p = vec![
            player(1, Some(2000), None),
            player(2, Some(1900), None),
            player(3, Some(1000), None),
            player(4, Some(900), None),
        ];
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();
        let settings = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)],
            ..Default::default()
        };

        let round = pair_round_weighted(1, &p, &settings, &[], &present, &[], None);

        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(p[0].id, p[1].id)),
            "top MacMahon group paired within itself"
        );
        assert!(
            pairs.contains(&unord(p[2].id, p[3].id)),
            "bottom MacMahon group paired within itself"
        );
    }

    /// Round 1, one score group of four, rating order p0>p1>p2>p3 so the fold
    /// ideal is p0-p2 and p1-p3 — which are club-mates (X and Y). Used by the club
    /// tests below to see whether protection overrides the fold.
    fn two_clubs_where_fold_pairs_mates() -> Vec<Player> {
        vec![
            player(1, Some(2000), Some("X")),
            player(2, Some(1900), Some("Y")),
            player(3, Some(1800), Some("X")),
            player(4, Some(1700), Some("Y")),
        ]
    }

    #[test]
    fn weighted_avoids_pairing_club_mates_when_protection_on() {
        let p = two_clubs_where_fold_pairs_mates();
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();
        let settings = TournamentSettings {
            club_protection_enabled: true,
            ..Default::default()
        };

        let round = pair_round_weighted(1, &p, &settings, &[], &present, &[], None);

        assert_eq!(round.boards.len(), 2);
        let club_of = |id: Uuid| p.iter().find(|q| q.id == id).unwrap().club.clone();
        for b in &round.boards {
            assert_ne!(
                club_of(b.player1),
                club_of(b.player2),
                "club-mates were paired despite protection"
            );
        }
    }

    #[test]
    fn club_protection_off_by_default_pairs_the_fold() {
        // With protection off (the default), the club rule is silent, so the fold
        // ideal wins and club-mates X-X / Y-Y are paired.
        let p = two_clubs_where_fold_pairs_mates();
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();

        let round = pair_round_weighted(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &present,
            &[],
            None,
        );

        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(p[0].id, p[2].id)),
            "fold pairs the X club-mates"
        );
        assert!(
            pairs.contains(&unord(p[1].id, p[3].id)),
            "fold pairs the Y club-mates"
        );
    }

    #[test]
    fn exempt_club_members_may_be_paired() {
        // Fold ideal is p0-p2 (both "Home") and p1-p3 (both unclubbed). With
        // protection on but "Home" exempt (spelled differently to prove the match
        // is case-insensitive), the Home pair is allowed and the fold wins; without
        // the exemption the club rule breaks that pairing up.
        let p = vec![
            player(1, Some(2000), Some("Home")),
            player(2, Some(1900), None),
            player(3, Some(1800), Some("Home")),
            player(4, Some(1700), None),
        ];
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();

        let exempt = TournamentSettings {
            club_protection_enabled: true,
            club_protection_exempt_clubs: vec!["  HOME ".into()],
            ..Default::default()
        };
        let round = pair_round_weighted(1, &p, &exempt, &[], &present, &[], None);
        assert!(
            board_pairs(&round).contains(&unord(p[0].id, p[2].id)),
            "exempt club-mates should be paired by the fold"
        );

        let protected = TournamentSettings {
            club_protection_enabled: true,
            ..Default::default()
        };
        let round = pair_round_weighted(1, &p, &protected, &[], &present, &[], None);
        assert!(
            !board_pairs(&round).contains(&unord(p[0].id, p[2].id)),
            "non-exempt club-mates should not be paired"
        );
    }

    #[test]
    fn club_protection_only_applies_within_its_round_window() {
        // Protection limited to round 1: round 2 must ignore clubs, so the fold
        // ideal (club-mate pairs) wins again.
        let p = two_clubs_where_fold_pairs_mates();
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();
        let settings = TournamentSettings {
            club_protection_enabled: true,
            club_protection_rounds: Some(1),
            ..Default::default()
        };

        // Pair round 2 directly (no completed rounds needed to exercise the window).
        let round = pair_round_weighted(2, &p, &settings, &[], &present, &[], None);
        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(p[0].id, p[2].id)),
            "past the window, fold pairs X-X"
        );
        assert!(
            pairs.contains(&unord(p[1].id, p[3].id)),
            "past the window, fold pairs Y-Y"
        );
    }

    /// Two MacMahon groups of 4 (threshold 1500), each already paired internally
    /// in round 1 with an upset, so round 2's tied scores admit a cheaper
    /// cross-group matching under score-gap alone: the group boundary is
    /// otherwise avoidable (both groups have a same-group, non-rematch partner
    /// available), so this isolates the airtight-groups rule rather than a
    /// parity-forced float.
    #[test]
    fn airtight_groups_keeps_macmahon_groups_apart_when_scores_would_cross() {
        let p: Vec<Player> = vec![
            player(1, Some(2000), None),
            player(2, Some(1950), None),
            player(3, Some(1900), None),
            player(4, Some(1850), None),
            player(5, Some(1000), None),
            player(6, Some(950), None),
            player(7, Some(900), None),
            player(8, Some(850), None),
        ];
        let settings_base = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)],
            ..Default::default()
        };
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();
        let r1 = completed_round(
            1,
            &[
                (p[0].id, p[1].id, Winner::Player2), // p1 beats p0 (top group upset)
                (p[2].id, p[3].id, Winner::Player1), // p2 beats p3
                (p[4].id, p[5].id, Winner::Player2), // p5 beats p4 (bottom group upset)
                (p[6].id, p[7].id, Winner::Player1), // p6 beats p7
            ],
            None,
        );

        // Without airtight groups, score-gap alone finds a cheaper matching that
        // crosses the MacMahon boundary twice.
        let round_off = pair_round_weighted(
            2,
            &p,
            &settings_base,
            std::slice::from_ref(&r1),
            &present,
            &[],
            None,
        );
        let top: HashSet<Uuid> = [p[0].id, p[1].id, p[2].id, p[3].id].into_iter().collect();
        let crosses = round_off
            .boards
            .iter()
            .filter(|b| top.contains(&b.player1) != top.contains(&b.player2))
            .count();
        assert_eq!(crosses, 2, "score-gap alone crosses the MacMahon boundary");

        // With airtight groups active for round 2, every board stays within its
        // MacMahon group.
        let settings_on = TournamentSettings {
            airtight_groups_rounds: Some(2),
            ..settings_base
        };
        let round_on = pair_round_weighted(2, &p, &settings_on, &[r1], &present, &[], None);
        for b in &round_on.boards {
            assert_eq!(
                top.contains(&b.player1),
                top.contains(&b.player2),
                "airtight groups should keep every board within its MacMahon group"
            );
        }
    }

    #[test]
    fn airtight_groups_only_applies_within_its_round_window() {
        // Same setup as above, but the window only covers round 1: round 2 must
        // ignore MacMahon groups again, so score-gap crosses the boundary.
        let p: Vec<Player> = vec![
            player(1, Some(2000), None),
            player(2, Some(1950), None),
            player(3, Some(1900), None),
            player(4, Some(1850), None),
            player(5, Some(1000), None),
            player(6, Some(950), None),
            player(7, Some(900), None),
            player(8, Some(850), None),
        ];
        let settings = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)],
            airtight_groups_rounds: Some(1),
            ..Default::default()
        };
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();
        let r1 = completed_round(
            1,
            &[
                (p[0].id, p[1].id, Winner::Player2),
                (p[2].id, p[3].id, Winner::Player1),
                (p[4].id, p[5].id, Winner::Player2),
                (p[6].id, p[7].id, Winner::Player1),
            ],
            None,
        );

        let round = pair_round_weighted(2, &p, &settings, &[r1], &present, &[], None);
        let top: HashSet<Uuid> = [p[0].id, p[1].id, p[2].id, p[3].id].into_iter().collect();
        let crosses = round
            .boards
            .iter()
            .filter(|b| top.contains(&b.player1) != top.contains(&b.player2))
            .count();
        assert_eq!(crosses, 2, "past the window, score-gap crosses again");
    }

    // --- ELO (non-Swiss) mode ---------------------------------------------

    fn elo_settings() -> TournamentSettings {
        TournamentSettings {
            elo_pairing_enabled: true,
            // Neutralize the provisional-rating widening so these pairing tests
            // exercise the base drift; reliability is covered in elo.rs tests.
            elo_provisional_multiplier_percent: 100,
            ..Default::default()
        }
    }

    #[test]
    fn elo_mode_pairs_adjacent_ratings_round_one() {
        // Round 1 (no games yet): every estimate sits at its registration rating,
        // so minimizing the squared ELO gap pairs neighbours: 2000-1950, 1500-1450.
        let p = vec![
            player(1, Some(2000), None),
            player(2, Some(1950), None),
            player(3, Some(1500), None),
            player(4, Some(1450), None),
        ];
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();

        let round = pair_round_weighted(1, &p, &elo_settings(), &[], &present, &[], None);

        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(p[0].id, p[1].id)),
            "closest pair 2000-1950"
        );
        assert!(
            pairs.contains(&unord(p[2].id, p[3].id)),
            "closest pair 1500-1450"
        );
    }

    #[test]
    fn elo_mode_gives_the_bye_to_the_weakest() {
        // Five players, no forced bye: the lowest-rated (and, round 1, lowest
        // estimate) should sit out, and the other four pair by adjacency.
        let p: Vec<Player> = (1..=5)
            .map(|i| player(i, Some(2000 - (i - 1) * 300), None))
            .collect();
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();

        let round = pair_round_weighted(1, &p, &elo_settings(), &[], &present, &[], None);

        assert_eq!(round.bye, Some(p[4].id), "the weakest player takes the bye");
        assert_eq!(round.boards.len(), 2);
    }

    #[test]
    fn elo_mode_reacts_to_results_and_avoids_rematch() {
        // Round 1 paired 2000-1950 and 1500-1450. Say the two lower-rated players
        // (1950, 1450) win. Their estimates rise; round 2 must not rematch, and the
        // squared-ELO-gap rule now pairs the two round-1 winners together and the
        // two losers together.
        let p = vec![
            player(1, Some(2000), None),
            player(2, Some(1950), None),
            player(3, Some(1500), None),
            player(4, Some(1450), None),
        ];
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();
        let settings = elo_settings();

        // 1950 (p1) beat 2000 (p0); 1450 (p3) beat 1500 (p2).
        let r1 = completed_round(
            1,
            &[
                (p[1].id, p[0].id, Winner::Player1),
                (p[3].id, p[2].id, Winner::Player1),
            ],
            None,
        );

        let round = pair_round_weighted(2, &p, &settings, &[r1], &present, &[], None);
        let pairs = board_pairs(&round);
        // No rematch of the round-1 boards.
        assert!(!pairs.contains(&unord(p[0].id, p[1].id)));
        assert!(!pairs.contains(&unord(p[2].id, p[3].id)));
        // Winners (raised estimates) meet, losers meet.
        assert!(
            pairs.contains(&unord(p[1].id, p[3].id)),
            "the two winners are paired"
        );
        assert!(
            pairs.contains(&unord(p[0].id, p[2].id)),
            "the two losers are paired"
        );
    }

    #[test]
    fn floater_selection_floats_down_the_weakest() {
        // Upper group (1 MacMahon point): X0>X1>X2 by rating. Lower group (0): a
        // single Y. One X must float down; it should be X2, the weakest of X, so
        // X0-X1 stay together.
        let p = vec![
            player(1, Some(2000), None), // X0 (rank 0 of the upper group)
            player(2, Some(1900), None), // X1
            player(3, Some(1800), None), // X2 (weakest)
            player(4, Some(1000), None), // Y (lower group)
        ];
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();
        let settings = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)], // X0..X2 on 1 point, Y on 0
            ..Default::default()
        };

        let round = pair_round_weighted(1, &p, &settings, &[], &present, &[], None);
        assert!(
            board_pairs(&round).contains(&unord(p[2].id, p[3].id)),
            "the weakest of the upper group should float down"
        );
    }

    #[test]
    fn floater_selection_classic_vs_median_pick_different_ascenders() {
        // Upper group (1 point): a single H. Lower group (0 points): L0>L1>L2 by
        // rating. One L floats up: classic sends the first (L0), median the middle
        // (L1). The fold is indifferent between those two outcomes, so the floater
        // rule decides.
        let p = vec![
            player(1, Some(2000), None), // H  (1 MacMahon point)
            player(2, Some(1400), None), // L0 (rank 0 of the lower group)
            player(3, Some(1300), None), // L1 (median)
            player(4, Some(1200), None), // L2
        ];
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();
        let base = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)],
            ..Default::default()
        };

        let classic = TournamentSettings {
            floater_style: FloaterStyle::Classic,
            ..base.clone()
        };
        let round = pair_round_weighted(1, &p, &classic, &[], &present, &[], None);
        assert!(
            board_pairs(&round).contains(&unord(p[0].id, p[1].id)),
            "classic Swiss floats up the strongest of the group (L0)"
        );

        let median = TournamentSettings {
            floater_style: FloaterStyle::Median,
            ..base
        };
        let round = pair_round_weighted(1, &p, &median, &[], &present, &[], None);
        assert!(
            board_pairs(&round).contains(&unord(p[0].id, p[2].id)),
            "median Swiss floats up the median of the group (L1)"
        );
    }

    #[test]
    fn floater_selection_median_descends_the_median_not_the_weakest() {
        // Upper group (1 point): X0>X1>X2 by rating. Lower group (0 points): a
        // single Y. One X must float down: classic sends the weakest (X2),
        // median sends the middle (X1).
        let p = vec![
            player(1, Some(2000), None), // X0 (strongest)
            player(2, Some(1900), None), // X1 (median)
            player(3, Some(1800), None), // X2 (weakest)
            player(4, Some(1000), None), // Y (lower group)
        ];
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();
        let base = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)],
            ..Default::default()
        };

        let classic = TournamentSettings {
            floater_style: FloaterStyle::Classic,
            ..base.clone()
        };
        let round = pair_round_weighted(1, &p, &classic, &[], &present, &[], None);
        assert!(
            board_pairs(&round).contains(&unord(p[2].id, p[3].id)),
            "classic Swiss floats down the weakest of the group (X2)"
        );

        let median = TournamentSettings {
            floater_style: FloaterStyle::Median,
            ..base
        };
        let round = pair_round_weighted(1, &p, &median, &[], &present, &[], None);
        assert!(
            board_pairs(&round).contains(&unord(p[1].id, p[3].id)),
            "median Swiss floats down the median of the group (X1)"
        );
    }

    #[test]
    fn floater_selection_median_gives_the_bye_to_the_median_of_the_group() {
        // 5 players, all in the same (single) score group in round 1, ranked
        // P0 > P1 > P2 > P3 > P4 by rating. Classic sends the bye to the
        // weakest (P4); median sends it to the middle of the group (P2).
        let p = vec![
            player(1, Some(2000), None), // P0
            player(2, Some(1800), None), // P1
            player(3, Some(1600), None), // P2 (median)
            player(4, Some(1400), None), // P3
            player(5, Some(1200), None), // P4 (weakest)
        ];
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();

        let classic = TournamentSettings {
            floater_style: FloaterStyle::Classic,
            ..Default::default()
        };
        let round = pair_round_weighted(1, &p, &classic, &[], &present, &[], None);
        assert_eq!(
            round.bye,
            Some(p[4].id),
            "classic Swiss gives the bye to the weakest of the group (P4)"
        );

        let median = TournamentSettings {
            floater_style: FloaterStyle::Median,
            ..Default::default()
        };
        let round = pair_round_weighted(1, &p, &median, &[], &present, &[], None);
        assert_eq!(
            round.bye,
            Some(p[2].id),
            "median Swiss gives the bye to the median of the group (P2)"
        );
    }

    #[test]
    fn scale_ladder_tiers_are_disjoint() {
        // Arbitrary per-rule worst-case totals, in priority order (highest first).
        let max_total = [7i128, 40, 13, 21, 5, 9];
        let mult = scale_ladder(&max_total);
        // Each rule's multiplier is exactly 1 plus the most every lower-priority
        // rule can contribute, so one of its units strictly dominates them all —
        // a correct lexicographic ordering.
        for i in 0..max_total.len() {
            let lower_max: i128 = ((i + 1)..max_total.len())
                .map(|j| mult[j] * max_total[j])
                .sum();
            assert_eq!(mult[i], 1 + lower_max);
            assert!(mult[i] > lower_max);
        }
        assert_eq!(mult[max_total.len() - 1], 1); // the lowest-priority rule is the unit
    }

    #[test]
    fn rule_bounds_are_valid_upper_bounds() {
        // A field with rematches, a bye, floats, club-mates and a spread of scores
        // so every rule can fire; then assert no single edge (or bye) can exceed
        // the per-edge share of its rule's `max_total_units`.
        let p = vec![
            player(1, Some(2000), Some("X")),
            player(2, Some(1800), Some("X")),
            player(3, Some(1600), Some("Y")),
            player(4, Some(1400), Some("Y")),
            player(5, Some(1200), None),
        ];
        let id = |i: usize| p[i].id;
        let r1 = completed_round(
            1,
            &[
                (id(0), id(1), Winner::Player1),
                (id(2), id(3), Winner::Player1),
            ],
            Some(id(4)), // p5 took a bye
        );
        let settings = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)],
            ..Default::default()
        };
        let scores = compute_scores(&p, &settings, &[r1]);
        let by_player: HashMap<Uuid, &Player> = p.iter().map(|q| (q.id, q)).collect();
        let free: Vec<Uuid> = p.iter().map(|q| q.id).collect();
        let fold = fold_ranks(&scores, &by_player, &free);
        let (mut lo, mut hi) = (u32::MAX, 0u32);
        let (mut mm_lo, mut mm_hi) = (u32::MAX, 0u32);
        for &pid in &free {
            let s = scores.get(&pid);
            lo = lo.min(s.points);
            hi = hi.max(s.points);
            mm_lo = mm_lo.min(s.macmahon);
            mm_hi = mm_hi.max(s.macmahon);
        }
        let edges = 3i128; // 5 free + phantom bye = 6 vertices → 3 edges
        let exempt_clubs = HashSet::new();
        let empty_elo: HashMap<Uuid, i64> = HashMap::new();
        let empty_rank: HashMap<Uuid, i128> = HashMap::new();
        let ctx = Ctx {
            scores: &scores,
            by_player: &by_player,
            fold: &fold,
            round: 2,
            floater_style: FloaterStyle::Median, // exercise the floater-selection bound
            club_active: true,                   // exercise the club rule's bound
            airtight_active: true,               // exercise the airtight-groups bound
            exempt_clubs: &exempt_clubs,
            edges,
            max_gap: (hi - lo) as i128,
            min_points: lo as i128,
            max_mm_gap: (mm_hi - mm_lo) as i128,
            max_group: fold.values().map(|f| f.group_size).max().unwrap_or(0) as i128,
            free_count: free.len() as i128,
            elo: &empty_elo,
            elo_rank: &empty_rank,
            max_elo_gap: 0,
        };

        // Check the Swiss rules (the ones active in the default mode). Bye-only
        // rules fire once per round, not once per edge, so their bound isn't
        // scaled by `edges` (see `max_total_units`).
        for &rule in active_rules(&TournamentSettings::default()) {
            let bound = rule.max_total_units(&ctx);
            let bye_scale = match rule {
                Rule::ByeSelection | Rule::ByeGroup => 1,
                _ => edges,
            };
            for i in 0..free.len() {
                for j in (i + 1)..free.len() {
                    assert!(
                        rule.edge_units(&ctx, free[i], free[j]) * edges <= bound,
                        "an edge exceeded the rule's total-units bound"
                    );
                }
                assert!(
                    rule.bye_units(&ctx, free[i]) * bye_scale <= bound,
                    "a bye exceeded the rule's total-units bound"
                );
            }
        }
    }

    // --- Explanation ------------------------------------------------------

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
        let boards = [(p[0].id, p[1].id), (p[2].id, p[3].id)];

        let ex = explain_pairing(1, &p, &TournamentSettings::default(), &[], &boards, None);

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
        let boards = [(p[0].id, p[2].id), (p[1].id, p[3].id)];

        let ex = explain_pairing(1, &p, &TournamentSettings::default(), &[], &boards, None);

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
        let settings = TournamentSettings {
            macmahon_thresholds: vec![MacMahonThreshold::elo(1500)],
            ..Default::default()
        };
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();
        let round = pair_round_weighted(1, &p, &settings, &[], &present, &[], None);

        let boards: Vec<(Uuid, Uuid)> = round
            .boards
            .iter()
            .map(|b| (b.player1, b.player2))
            .collect();
        let ex = explain_pairing(1, &p, &settings, &[], &boards, round.bye);

        // Re-derive the units independently through a fresh model and compare.
        let mut free: Vec<Uuid> = boards.iter().flat_map(|&(a, b)| [a, b]).collect();
        if let Some(b) = round.bye {
            free.push(b);
        }
        let model = PairingModel::build(1, &p, &settings, &[], &free, round.bye.is_some());
        for (ledger, &(a, b)) in ex.boards.iter().zip(&boards) {
            let units = model.edge_units(a, b);
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

    // --- Counterfactual ---------------------------------------------------

    fn changed_pairs(cf: &Counterfactual) -> HashSet<(Uuid, Uuid)> {
        cf.changed
            .iter()
            .map(|b| unord(b.player1, b.player2.unwrap_or(PHANTOM)))
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
        let boards = [(p[0].id, p[2].id), (p[1].id, p[3].id)];

        let cf = counterfactual_force(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &boards,
            None,
            p[0].id,
            p[1].id,
        );

        assert!(cf.scoped_out.is_none());
        let changed = changed_pairs(&cf);
        assert!(
            changed.contains(&unord(p[0].id, p[1].id)),
            "the forced board appears"
        );
        assert!(
            changed.contains(&unord(p[2].id, p[3].id)),
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
        let boards = [(p[0].id, p[2].id), (p[1].id, p[3].id)];

        let cf = counterfactual_force(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &boards,
            None,
            p[0].id,
            p[2].id,
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
        let boards = [(p[0].id, p[2].id), (p[1].id, p[3].id)];

        let cf = counterfactual_force(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &boards,
            None,
            p[0].id,
            p[1].id,
        );

        let fold = cf.cost_delta.iter().find(|d| d.rule == RuleId::Fold);
        assert!(
            matches!(fold, Some(d) if d.units > 0),
            "forcing the worse fold costs units"
        );
    }

    #[test]
    fn forcing_across_a_bye_reassigns_the_sit_out() {
        // Three equal players: one byes. Force the current bye-taker to play the
        // player they'd otherwise sit behind, and someone else must take the bye —
        // surfaced as a changed board with no player2.
        let p: Vec<Player> = (1..=3).map(|i| player(i, Some(1500), None)).collect();
        // Engine round: p0-p1 play, p2 byes (deterministic by tournament number).
        let round = pair_round_weighted(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &p.iter().map(|x| x.id).collect::<Vec<_>>(),
            &[],
            None,
        );
        let boards: Vec<(Uuid, Uuid)> = round
            .boards
            .iter()
            .map(|b| (b.player1, b.player2))
            .collect();
        let bye = round.bye.expect("odd count byes someone");
        let opponent = boards[0].0;

        let cf = counterfactual_force(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
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
        let boards = [(p[0].id, p[2].id), (p[1].id, p[3].id)];

        let cf = counterfactual_forbid(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &boards,
            None,
            p[0].id,
            p[2].id,
        );

        let changed = changed_pairs(&cf);
        assert!(
            !changed.contains(&unord(p[0].id, p[2].id)),
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
        let boards = [(p[0].id, p[2].id), (p[1].id, p[3].id)];

        let cf = counterfactual_forbid(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &boards,
            None,
            p[0].id,
            p[1].id,
        );

        assert!(cf.changed.is_empty());
        assert!(cf.cost_delta.is_empty());
    }
}
