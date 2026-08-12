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
//!    Immediately below it, and only in a hybrid cup's qualification round:
//!    **no pre-qualified clash** — the cup's pre-qualified players spend that
//!    round in the Swiss pool, but must not be paired with each other. Absent
//!    from the rule set in every other round and every other format, so it costs
//!    nothing to have.
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
//! 8. **Different nationalities** (optional, off by default) — the same idea one
//!    notch weaker: avoid pairing compatriots (ignored when a nationality is
//!    unknown). Below the club rule, so when the two disagree — one pairing
//!    shares a club, the other a nationality — the club clash is the one avoided.
//! 9. **Fold within a score group** — sort a group (equal points) by rating
//!    (unrated = 1), descending; the Nth player of the top half should meet the
//!    Nth of the bottom half, penalized by the *squared* deviation from that ideal.
//!
//! Priority lives in exactly one place — the order of [`active_rules`] — and the
//! separation between tiers is proven by construction (see [`scale_ladder`]), so
//! adding or reordering rules stays sound with no magic numbers to retune.
//!
//! [`pair_round_weighted`] is the real pairing path; the bye is modeled as a
//! phantom vertex.
//!
//! ## Determinism and tie-breaking
//!
//! Rule costs are coarse integers, so the minimum-weight matching is frequently
//! achieved by several distinct pairings at once — e.g. two interchangeable
//! players, or which of the equal-lowest scorers takes the bye. Because the scalar
//! weight is an injective lexicographic encoding of the per-rule unit totals (see
//! [`scale_ladder`]), two pairings tie on total cost *exactly* when they emit the
//! same units on every rule — i.e. when the rules are genuinely indifferent
//! between them.
//!
//! The choice among tied pairings is made deterministically and as a pure function
//! of tournament *state*: the matching's vertices are ordered by tournament number
//! (see the `free.sort_unstable()` in [`pair_round_weighted`]), so the same field
//! pairs identically no matter how it was registered, imported, or reloaded, and
//! within that
//! canonical order the blossom solver returns one fixed optimum. No lower-priority
//! "seniority" preference is layered on top — a tie means the rules do not care,
//! and the seed order only fixes *a* stable, reproducible representative.
//!
//! An ILP/CP-SAT backend is still planned (see TODO.md) for very large fields and
//! for formats needing hard constraints a plain matching can't express.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::elo::{estimate_elos, UNRATED_PRIOR_MEAN};
use crate::matching::{min_weight_perfect_matching, Weight};
use crate::player::Player;
use crate::round::{PairingSource, Round};
use crate::scoring::compute_scores;
use crate::settings::{FloaterStyle, TournamentSettings};
use crate::units::{HalfPoints, TournamentId, UnitKey};

use typed_index_collections::{TiSlice, TiVec};

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
/// The `/ 16` margin covers the solver's internal ×4 weight scaling and its
/// `MAX / 4` "infinity" sentinel with room to spare.
fn solve_matching(cost: &[i128], n: usize) -> Vec<usize> {
    dump_matching_instance(cost, n);
    let max = cost.iter().copied().max().unwrap_or(0);
    if max <= i32::MAX as i128 / 16 {
        min_weight_perfect_matching(&narrow::<i32>(cost), n)
    } else if max <= i64::MAX as i128 / 16 {
        min_weight_perfect_matching(&narrow::<i64>(cost), n)
    } else {
        // The narrow tiers require `max <= TYPE::MAX / 16` (headroom for the
        // solver's internal ×4 edge-weight scaling and its `MAX / 4` "infinity"
        // sentinel); the i128 tier needs the same guard, or a cost that reaches
        // this range overflows *inside* the solver — silently, in release.
        // There is no wider integer to fall back to, so this is a hard scale
        // limit, checked here rather than left to corrupt the matching.
        assert!(
            max <= i128::MAX / 16,
            "pairing cost {max} exceeds the matching solver's i128 headroom \
             (i128::MAX / 16): field too large or score/MacMahon spread too wide \
             (known scale limit)"
        );
        min_weight_perfect_matching(cost, n)
    }
}

/// Capture hook: when `OSP_MATCHING_DUMP` names a directory, every cost matrix
/// handed to the solver is also written there as a little-endian binary blob
/// (`b"OSPM1"`, `n` as `u64`, then the `n*n` `i128` values) for offline replay
/// by `integer-blossom`'s `examples/bench.rs --replay` — solver benchmarks on
/// the exact graphs this engine produces rather than synthetic families.
///
/// Best-effort by design: I/O errors are swallowed (a failed dump must never
/// affect pairing), and the cost when the variable is unset is one lazily
/// initialized check. File names carry a process-wide sequence number, so
/// multithreaded runs dump safely — but for a *canonical* capture set, run
/// single-threaded with `--runs 1` (the sequence order is then deterministic).
fn dump_matching_instance(cost: &[i128], n: usize) {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::OnceLock;
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let Some(dir) = DIR.get_or_init(|| {
        let dir = std::env::var_os("OSP_MATCHING_DUMP").map(PathBuf::from)?;
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }) else {
        return;
    };
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut buf = Vec::with_capacity(13 + cost.len() * 16);
    buf.extend_from_slice(b"OSPM1");
    buf.extend_from_slice(&(n as u64).to_le_bytes());
    for c in cost {
        buf.extend_from_slice(&c.to_le_bytes());
    }
    let _ = std::fs::write(dir.join(format!("solve_{seq:05}_n{n}.ospm")), buf);
}

/// Convert a row-major `i128` cost matrix down to a narrower [`Weight`] type.
/// Panics if a value doesn't fit — callers must check the bound first (see
/// [`solve_matching`]).
fn narrow<W>(cost: &[i128]) -> Vec<W>
where
    W: Weight + TryFrom<i128>,
    <W as TryFrom<i128>>::Error: std::fmt::Debug,
{
    cost.iter().map(|&c| W::try_from(c).unwrap()).collect()
}

/// The pairing rules. The active subset and its priority order depend on the
/// mode (see [`active_rules`]); that ordering is the single source of truth for
/// priority, and the scalar multipliers are derived from it (see
/// [`scale_ladder`]).
#[derive(Clone, Copy)]
enum Rule {
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
fn active_rules(settings: &TournamentSettings) -> &'static [Rule] {
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

/// Everything the engine needs to know about one **pairable unit**, whoever it
/// is: a player in an individual tournament, a team in a team tournament. The
/// rules read nothing else, so one engine serves both modes — see [`UnitKey`].
///
/// Defaultable so the table can leave a gap at any key no unit holds (key 0, the
/// phantom, and any number freed by a pre-play removal), exactly as
/// [`Scores`](crate::scoring::Scores) does.
///
/// Built by [`player_units`] for the individual path.
#[derive(Debug, Default, Clone)]
pub(crate) struct PairingUnit {
    /// Total score entering the round: MacMahon start, adjustments and results.
    pub points: HalfPoints,
    /// MacMahon starting points alone (the airtight-groups rule's quantity).
    pub macmahon: HalfPoints,
    /// Units faced so far, one entry per game (a long board counts twice).
    pub opponents: Vec<UnitKey>,
    /// Whether this unit has already had a bye (or the free point an opponent's
    /// no-show hands it).
    pub had_bye: bool,
    /// Round of the most recent up / down float, `None` if never.
    pub last_ascended: Option<u32>,
    pub last_descended: Option<u32>,
    /// What the fold sorts on: a player's rating, a team's average pairing
    /// rating. `None` for unrated, which sorts last (as rating 1).
    pub rating: Option<u32>,
    /// Normalized clubs (see [`TournamentSettings::normalize_club`]) in **board
    /// order** — one entry for a player, one per member for a team. The club rule
    /// compares aligned positions, since board `k` of one team only ever meets
    /// board `k` of the other.
    pub clubs: Vec<Option<String>>,
    /// Normalized nationalities (see
    /// [`TournamentSettings::normalize_nationality`]) in **board order**, read
    /// by [`Rule::Nationality`] exactly as `clubs` is read by [`Rule::Club`] —
    /// so the two vectors always have the same length (one entry per board).
    pub nationalities: Vec<Option<String>>,
    /// Whether this unit is a **pre-qualified** cup entrant this round (see
    /// [`Rule::CupPrequalified`]). Always false outside the qualifier cup's
    /// qualification round, where the rule is filtered out of the set entirely,
    /// and in team mode, where the cup is rejected.
    pub prequalified: bool,
    /// (ELO mode) Rounded live ELO estimate; zero in every other mode, which
    /// rejects ELO pairing.
    pub elo: i64,
}

impl PairingUnit {
    /// What the fold sorts by: the rating, with unrated as 1 so it sorts last.
    fn fold_rating(&self) -> u32 {
        self.rating.unwrap_or(1)
    }
}

/// Everything the rules need to score an edge, plus the per-round quantities their
/// worst-case bounds (and hence multipliers) are derived from.
struct Ctx<'a> {
    /// The units being paired, indexed by their key (gaps hold a default).
    units: &'a TiSlice<UnitKey, PairingUnit>,
    /// Fold placement per free unit, indexed by key (`None` for a non-free unit,
    /// whose key still indexes the slice).
    fold: &'a TiSlice<UnitKey, Option<FoldInfo>>,
    round: u32,
    /// Which unit each lower group sends up as its ascending floater.
    floater_style: FloaterStyle,
    /// Clubs exempt from protection, in normalized form (see
    /// [`TournamentSettings::normalize_club`]).
    exempt_clubs: &'a HashSet<String>,
    /// Nationalities exempt from protection, in normalized form (see
    /// [`TournamentSettings::normalize_nationality`]).
    exempt_nationalities: &'a HashSet<String>,
    /// Edges in a perfect matching over the vertices (= vertices / 2).
    edges: i128,
    /// Largest points gap between any two vertices (bounds the score rule).
    max_gap: i128,
    /// Lowest points among the free units (the bye's target group).
    min_points: i128,
    /// Largest MacMahon-points gap between any two vertices (bounds the airtight
    /// groups rule).
    max_mm_gap: i128,
    /// Largest score-group size among the free units (bounds the fold rule).
    max_group: i128,
    /// Number of free units (bounds the bye-selection rule).
    free_count: i128,
    /// Most board positions any free unit has (1 for a player, the team size for
    /// a team) — the club rule's per-edge maximum.
    max_boards: i128,
    /// (ELO mode) Ascending ELO rank per free unit, 0 = weakest, indexed by key;
    /// all zero in Swiss mode.
    elo_rank: &'a TiSlice<UnitKey, i128>,
    /// (ELO mode) Largest rounded-ELO gap among free units (bounds the ELO-gap
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
fn floater_units(ctx: &Ctx, id: UnitKey, descending: bool) -> i128 {
    let Some(f) = &ctx.fold[id] else {
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
    fn id(self) -> RuleId {
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
    fn edge_units(self, ctx: &Ctx, a: UnitKey, b: UnitKey) -> i128 {
        let sa = &ctx.units[a];
        let sb = &ctx.units[b];
        match self {
            // Rule 1: never play the same opponent twice.
            Rule::Rematch => i128::from(sa.opponents.contains(&b)),
            // Rule 1b (qualifier cup, round 1): never pair two pre-qualified cup
            // players. Only in the rule set when the set is non-empty, so the
            // lookup is free in every other round and every other format.
            Rule::CupPrequalified => i128::from(sa.prequalified && sb.prequalified),
            // Rule 2: bye-only rule, real boards are neutral.
            Rule::ByeGroup => 0,
            // Rule 3 (optional, first N rounds): forbid crossing MacMahon groups;
            // penalty is the square of the gap in MacMahon starting points. Only in
            // the rule set when active (inactive rules are filtered out upstream),
            // so no `airtight_active` check; and the gap is squared, so its sign —
            // and an `abs` — are irrelevant.
            Rule::AirtightGroups => {
                let gap = sa.macmahon.halves() as i128 - sb.macmahon.halves() as i128;
                gap * gap
            }
            // Rule 4: prefer equal scores; penalty is the square of the gap (so the
            // gap's sign, and an `abs`, don't matter).
            Rule::ScoreGap => {
                let gap = sa.points.halves() as i128 - sb.points.halves() as i128;
                gap * gap
            }
            // Rule 5: the lower-scored player floats up, the higher-scored down.
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
            // Rule 6: on a cross-group (float) edge, prefer the right floaters —
            // classic Swiss wants the weakest of the upper group down and the
            // first of the lower group up; median Swiss wants the median of
            // each group instead. Same-group edges aren't floats, so no penalty.
            Rule::FloaterSelection => match sa.points.cmp(&sb.points) {
                Ordering::Equal => 0,
                // The higher-scored player is the descender, the lower the ascender;
                // the comparison already told us which is which.
                Ordering::Greater => floater_units(ctx, a, true) + floater_units(ctx, b, false),
                Ordering::Less => floater_units(ctx, b, true) + floater_units(ctx, a, false),
            },
            // Rule 7: avoid pairing club-mates — but only when protection is active
            // this round, ignoring unknown clubs and clubs on the exempt list. Club
            // names are matched case-insensitively. Only in the rule set when
            // protection is active (inactive rules are filtered out upstream), so
            // no `club_active` check.
            //
            // Within the rule's ladder tier the matching minimizes the same-club
            // *games* of the round — see [`shared_affiliation_units`] for why the
            // count is over aligned board positions.
            Rule::Club => shared_affiliation_units(&sa.clubs, &sb.clubs, ctx.exempt_clubs),
            // Rule 8: the same, one tier weaker, over nationalities — so when the
            // two rules disagree the club clash is the one the matching avoids.
            // Also filtered out upstream when inactive.
            Rule::Nationality => shared_affiliation_units(
                &sa.nationalities,
                &sb.nationalities,
                ctx.exempt_nationalities,
            ),
            // Rule 9: fold within a score group — squared deviation from the ideal
            // fold. Squaring (rather than |·|) spreads an unavoidable deviation across
            // boards instead of dumping it all on one, so no single player faces an
            // opponent far from the fold's intent — and it matches the squared
            // ScoreGap / EloGap rules. See `docs/swiss-fold.md`.
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
                    _ => 0,
                }
            }
            // Bye selection acts only on the bye edge; a real board is neutral.
            Rule::ByeSelection => 0,
            // ELO mode: prefer equal estimated ELO; penalty is the squared gap.
            Rule::EloGap => {
                let gap = (sa.elo - sb.elo) as i128;
                gap * gap
            }
        }
    }

    /// Penalty units for giving `player` the bye (before the priority multiplier).
    /// A bye repeats the rematch rule (never bye twice) and counts as a downfloat.
    fn bye_units(self, ctx: &Ctx, unit: UnitKey) -> i128 {
        let s = &ctx.units[unit];
        match self {
            Rule::Rematch => i128::from(s.had_bye),
            // A sit-out isn't a pairing, so two pre-qualified players can't clash
            // on it.
            Rule::CupPrequalified => 0,
            // The bye should go to the lowest score group; penalty is the square
            // of the gap to the lowest score among free players.
            Rule::ByeGroup => {
                let gap = s.points.halves() as i128 - ctx.min_points;
                gap * gap
            }
            Rule::FloatRepeat => float_units(s.last_descended, ctx.round),
            // A bye is a downfloat, so prefer the weakest of the group (classic)
            // or its median (median Swiss) to take it.
            Rule::FloaterSelection => floater_units(ctx, unit, true),
            // ELO mode: the weakest present player (lowest ELO rank) takes the bye.
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
/// spread, or the counterfactual re-solve ([`solve_stable`]) near a thousand.
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
fn ladder_mul(a: i128, b: i128) -> i128 {
    a.checked_mul(b).unwrap_or_else(|| ladder_overflow())
}

#[track_caller]
fn ladder_add(a: i128, b: i128) -> i128 {
    a.checked_add(b).unwrap_or_else(|| ladder_overflow())
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
        mult[i] = ladder_add(1, lower);
        lower = ladder_add(lower, ladder_mul(mult[i], max_total[i]));
    }
    mult
}

/// Total edge weight for pairing `a` against `b`: `Σ mult[rule] · units`, over the
/// active rules for this mode. Used off the hot path (explanations, the
/// alternative-pairing search); the O(k²) cost-matrix fill uses the per-rule
/// [`accumulate_edge_rule`] instead.
fn edge_cost(ctx: &Ctx, rules: &[Rule], mult: &[i128], a: UnitKey, b: UnitKey) -> i128 {
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
/// [`pair_round_weighted`]), not per edge. Only the upper triangle is written:
/// [`min_weight_perfect_matching`] reads the matrix as symmetric, taking just
/// `cost[i*n + j]` for `i < j`.
#[inline]
fn accumulate_edge_rule<F: Fn(&Ctx, UnitKey, UnitKey) -> i128>(
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
fn bye_cost(ctx: &Ctx, rules: &[Rule], mult: &[i128], unit: UnitKey) -> i128 {
    rules
        .iter()
        .zip(mult)
        .map(|(rule, m)| m * rule.bye_units(ctx, unit))
        .sum()
}

/// Within-group fold placement of a unit: its rank in the score group (by
/// rating, descending) and the group's size.
#[derive(Clone, Copy)]
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

/// Fold ranks for the `free` units, grouped by points and sorted within each
/// group by rating (descending; unrated = 1), ties broken by unit key for a
/// stable, reproducible ordering.
fn fold_ranks(
    units: &TiSlice<UnitKey, PairingUnit>,
    free: &[UnitKey],
) -> TiVec<UnitKey, Option<FoldInfo>> {
    // Group the free units by their points.
    let mut groups: HashMap<HalfPoints, Vec<UnitKey>> = HashMap::new();
    for &k in free {
        groups.entry(units[k].points).or_default().push(k);
    }
    // Result indexed by unit key (`None` for a non-free unit).
    let mut info: TiVec<UnitKey, Option<FoldInfo>> = vec![None; units.len()].into();
    for group in groups.values_mut() {
        // Highest rating first; ties broken by key for a stable, reproducible
        // ordering.
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

// --- Pairing model (shared by pairing and explanation) --------------------

/// One round's Swiss scoring context, built once from the pairing inputs and
/// reused for both pairing and explanation. It owns the derived per-round data
/// (scores, fold ranks, ELO estimates, the multiplier ladder) and lends a [`Ctx`]
/// on demand, so an explanation is scored against the *identical* construction
/// the pairing used — no risk of the two drifting apart.
struct PairingModel<'u> {
    /// The units being paired, indexed by key so the O(k²) cost loop indexes
    /// rather than hashes. Borrowed: the caller owns the table and reuses it (a
    /// pairing and its explanation are scored against the very same units).
    units: &'u TiSlice<UnitKey, PairingUnit>,
    /// Derived per-round data the rules read, also key-indexed. See [`Ctx`].
    fold: TiVec<UnitKey, Option<FoldInfo>>,
    exempt_clubs: HashSet<String>,
    exempt_nationalities: HashSet<String>,
    elo_rank: TiVec<UnitKey, i128>,
    round: u32,
    floater_style: FloaterStyle,
    edges: i128,
    max_gap: i128,
    min_points: i128,
    max_mm_gap: i128,
    max_group: i128,
    free_count: i128,
    max_boards: i128,
    max_elo_gap: i128,
    rules: Vec<Rule>,
    mult: Vec<i128>,
}

impl<'u> PairingModel<'u> {
    /// Build the model for the given `free` set (the units the matching will
    /// pair). `need_phantom` is whether a bye vertex participates, so the edge
    /// count — and hence the derived multipliers — match the matching that was or
    /// will be solved.
    fn build(
        number: u32,
        settings: &TournamentSettings,
        units: &'u TiSlice<UnitKey, PairingUnit>,
        free: &[UnitKey],
        need_phantom: bool,
    ) -> Self {
        let fold = fold_ranks(units, free);

        let (mut lo, mut hi) = (u32::MAX, 0u32);
        let (mut mm_lo, mut mm_hi) = (u32::MAX, 0u32);
        for &key in free {
            let s = &units[key];
            lo = lo.min(s.points.halves());
            hi = hi.max(s.points.halves());
            mm_lo = mm_lo.min(s.macmahon.halves());
            mm_hi = mm_hi.max(s.macmahon.halves());
        }
        let exempt_clubs = settings.exempt_clubs_normalized();
        let exempt_nationalities = settings.exempt_nationalities_normalized();

        // ELO mode: the ascending ELO rank of each free unit (0 = weakest, for the
        // bye-selection rule) and the widest gap (for the ladder bound). The
        // estimate itself is on the unit; all of this is zero in Swiss mode.
        let (elo_rank, max_elo_gap): (TiVec<UnitKey, i128>, i128) =
            if settings.elo_estimate_needed() {
                // Ascending ELO; ties by key.
                let mut order = free.to_vec();
                order.sort_by(|&x, &y| units[x].elo.cmp(&units[y].elo).then(x.cmp(&y)));
                let mut elo_rank: TiVec<UnitKey, i128> = vec![0i128; units.len()].into();
                for (rank, &key) in order.iter().enumerate() {
                    elo_rank[key] = rank as i128;
                }
                let (elo_lo, elo_hi) = free
                    .iter()
                    .map(|&key| units[key].elo)
                    .fold((i64::MAX, i64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
                (elo_rank, (elo_hi - elo_lo).max(0) as i128)
            } else {
                (vec![0i128; units.len()].into(), 0)
            };

        let k = free.len();
        let vcount = k + usize::from(need_phantom);
        let max_group = fold
            .iter()
            .flatten()
            .map(|f| f.group_size)
            .max()
            .unwrap_or(0) as i128;
        // The affiliation rules' per-edge ceiling, read off the instance rather
        // than assumed: one board per player, `size` boards per team. The club and
        // nationality vectors are built together from the same members, so one
        // count bounds both — an invariant worth stating rather than trusting.
        debug_assert!(
            free.iter()
                .all(|&key| units[key].clubs.len() == units[key].nationalities.len()),
            "a unit's clubs and nationalities must be one per board"
        );
        let max_boards = free
            .iter()
            .map(|&key| units[key].clubs.len())
            .max()
            .unwrap_or(0) as i128;
        // The active rules, minus the whole-round no-ops that contribute 0 to every
        // edge and bye — and (having max-total 0) leave every other rule's
        // multiplier unchanged, so dropping them here is exact and spares the O(k²)
        // cost loop a per-edge branch and call each:
        //   - `AirtightGroups` with its window closed, and `Club` / `Nationality`
        //     with their protection off;
        //   - the bye-only rules (`ByeGroup`, `ByeSelection`) when no phantom is in
        //     play: on an even field there is no bye vertex for them to fire on, so
        //     they would only reserve a ladder tier (and eat overflow headroom) for
        //     nothing.
        let club_active = settings.club_protection_active(number);
        let nationality_active = settings.nationality_protection_active(number);
        let airtight_active = settings.airtight_groups_active(number);
        let rules: Vec<Rule> = active_rules(settings)
            .iter()
            .copied()
            .filter(|r| match r {
                Rule::AirtightGroups => airtight_active,
                Rule::Club => club_active,
                Rule::Nationality => nationality_active,
                Rule::CupPrequalified => free.iter().any(|&key| units[key].prequalified),
                Rule::ByeGroup | Rule::ByeSelection => need_phantom,
                _ => true,
            })
            .collect();

        let mut model = PairingModel {
            units,
            fold,
            exempt_clubs,
            exempt_nationalities,
            elo_rank,
            round: number,
            floater_style: settings.floater_style(),
            edges: (vcount / 2) as i128,
            max_gap: hi.saturating_sub(lo) as i128,
            min_points: lo as i128,
            max_mm_gap: mm_hi.saturating_sub(mm_lo) as i128,
            max_group,
            free_count: k as i128,
            max_boards,
            max_elo_gap,
            rules,
            mult: Vec::new(),
        };
        // The multipliers depend on the per-rule bounds, which need a Ctx — so
        // build the ladder in a second pass, once the rest of the model exists.
        let max_total: Vec<i128> = {
            let ctx = model.ctx();
            model
                .rules
                .iter()
                .map(|r| r.max_total_units(&ctx))
                .collect()
        };
        model.mult = scale_ladder(&max_total);
        model
    }

    /// A scoring context borrowing this model's data.
    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            units: self.units,
            fold: &self.fold,
            round: self.round,
            floater_style: self.floater_style,
            exempt_clubs: &self.exempt_clubs,
            exempt_nationalities: &self.exempt_nationalities,
            edges: self.edges,
            max_gap: self.max_gap,
            min_points: self.min_points,
            max_mm_gap: self.max_mm_gap,
            max_group: self.max_group,
            free_count: self.free_count,
            max_boards: self.max_boards,
            elo_rank: &self.elo_rank,
            max_elo_gap: self.max_elo_gap,
        }
    }

    /// Scalar edge weight for pairing unit `a` against unit `b`.
    fn edge_cost(&self, a: UnitKey, b: UnitKey) -> i128 {
        edge_cost(&self.ctx(), &self.rules, &self.mult, a, b)
    }

    /// Scalar edge weight for giving `unit` the bye.
    fn bye_cost(&self, unit: UnitKey) -> i128 {
        bye_cost(&self.ctx(), &self.rules, &self.mult, unit)
    }

    /// Per-rule penalty units (pre-multiplier) for pairing `a` against `b`, in
    /// priority order (aligned with [`Self::rules`]).
    fn edge_units(&self, a: UnitKey, b: UnitKey) -> Vec<i128> {
        let ctx = self.ctx();
        self.rules
            .iter()
            .map(|r| r.edge_units(&ctx, a, b))
            .collect()
    }

    /// Per-rule penalty units (pre-multiplier) for giving `unit` the bye.
    fn bye_units(&self, unit: UnitKey) -> Vec<i128> {
        let ctx = self.ctx();
        self.rules.iter().map(|r| r.bye_units(&ctx, unit)).collect()
    }

    fn rules(&self) -> &[Rule] {
        &self.rules
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
///
/// The two sides are [`UnitKey`]s, so this reads as player numbers in individual
/// mode and team numbers in team mode — the pairing it explains is a pairing of
/// whatever the engine was given.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
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
fn ledger(
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

// --- Counterfactual ("why not pair A and B?") -----------------------------

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
            cost[j * n + i] = c;
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
                    cost[j * n + i] = prohibitive;
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
    let mut added: Vec<(UnitKey, UnitKey)> = m1.difference(m0).copied().collect();
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

/// One round's pairing in **unit** terms: who plays whom, and who sits out. The
/// caller turns each pair into the board(s) it stands for — one for a player
/// pairing, `size` for a team match — and each bye into the sit-out(s) it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnitPairing {
    /// The matched pairs: the referee's forced ones first (in draft order), then
    /// the engine's own.
    pub pairs: Vec<PairedUnits>,
    /// The bye the engine chose, if the free field was left odd. There is at most
    /// one by construction — a matching has a single phantom.
    pub swiss_bye: Option<UnitKey>,
}

/// One matched pair, with the facts of the pairing the boards must carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PairedUnits {
    pub a: UnitKey,
    pub b: UnitKey,
    /// How the pairing was decided (engine or referee).
    pub source: PairingSource,
    /// `points(a) − points(b)` at pairing time, frozen onto every board of the
    /// pairing: it is a fact of *this* pairing, and the float history replays
    /// from it.
    pub points_diff: i32,
}

/// Pair the `present` units by minimizing the total rule penalty, honoring
/// referee-forced pairs and forced byes. This is the real pairing path used by
/// [`crate::Tournament::confirm_round`], for players and teams alike; the rules
/// and their priority are described in the module docs.
///
/// The forced byes are simply taken out of the pool (and not echoed back — the
/// caller already knows them); if what remains is odd the engine still picks a
/// bye of its own, so any number of them is consistent.
///
/// Preconditions (validated by the caller): every forced unit is present and
/// appears at most once.
pub(crate) fn pair_round_weighted(
    number: u32,
    settings: &TournamentSettings,
    units: &TiSlice<UnitKey, PairingUnit>,
    present: &[UnitKey],
    forced_pairs: &[(UnitKey, UnitKey)],
    forced_byes: &[UnitKey],
) -> UnitPairing {
    // The float frozen onto each board: points(a) − points(b) now.
    let diff =
        |a: UnitKey, b: UnitKey| units[a].points.halves() as i32 - units[b].points.halves() as i32;

    let mut placed: HashSet<UnitKey> = HashSet::new();
    for &(a, b) in forced_pairs {
        placed.insert(a);
        placed.insert(b);
    }
    placed.extend(forced_byes.iter().copied());
    let mut free: Vec<UnitKey> = present
        .iter()
        .copied()
        .filter(|key| !placed.contains(key))
        .collect();

    // A phantom vertex absorbs the bye when an odd number of units remain
    // after the forced byes are taken out; whoever the matching pairs with it
    // sits out.
    let need_phantom = free.len() % 2 == 1;
    let k = free.len();
    let vcount = k + usize::from(need_phantom);

    let mut pairs: Vec<PairedUnits> = forced_pairs
        .iter()
        .map(|&(a, b)| PairedUnits {
            a,
            b,
            source: PairingSource::Forced,
            points_diff: diff(a, b),
        })
        .collect();
    let mut swiss_bye = None;

    if vcount >= 2 {
        // The pairing model owns the per-round derived data and the multiplier
        // ladder; the same construction backs `explain_pairing`.
        let model = PairingModel::build(number, settings, units, &free, need_phantom);

        // Order the matching's vertices canonically by seed (the unit key is the
        // seed), so which of several equally-optimal pairings the solver returns
        // depends only on tournament state — not on the order players were
        // registered, imported, or reloaded (the model itself is order-independent,
        // so this affects ties only).
        free.sort_unstable();

        // `free` is the unit keys in matching-vertex order, so the edge/bye scoring
        // indexes the model's per-key tables directly — no per-edge or per-vertex
        // cast. `ctx` is built once and shared across every edge.
        let ctx = model.ctx();
        // Row-major `vcount × vcount` cost matrix in one flat allocation (entry
        // `(i, j)` at `i * vcount + j`), rather than a `Vec<Vec>`: one allocation
        // and no per-row pointer-chase feeding the solver.
        let mut cost = vec![0i128; vcount * vcount];
        // Fill by rule, not by edge: each rule adds its contribution to every edge
        // in its own monomorphized O(k²) loop, so the per-edge `match self` in
        // `edge_units` folds away (the rule is a constant here). The dispatch on
        // which rule runs once per rule. Bye-only rules are 0 on every real edge, so
        // they are skipped. Only the upper triangle is written — the solver reads
        // the matrix as symmetric — so there is no symmetric store or mirror pass.
        macro_rules! fill {
            ($rule:expr, $m:expr) => {
                accumulate_edge_rule(&mut cost, vcount, k, &free, &ctx, $m, |ctx, a, b| {
                    $rule.edge_units(ctx, a, b)
                })
            };
        }
        for (&rule, &m) in model.rules.iter().zip(&model.mult) {
            match rule {
                Rule::Rematch => fill!(Rule::Rematch, m),
                Rule::CupPrequalified => fill!(Rule::CupPrequalified, m),
                Rule::AirtightGroups => fill!(Rule::AirtightGroups, m),
                Rule::ScoreGap => fill!(Rule::ScoreGap, m),
                Rule::FloatRepeat => fill!(Rule::FloatRepeat, m),
                Rule::FloaterSelection => fill!(Rule::FloaterSelection, m),
                Rule::Club => fill!(Rule::Club, m),
                Rule::Nationality => fill!(Rule::Nationality, m),
                Rule::Fold => fill!(Rule::Fold, m),
                Rule::EloGap => fill!(Rule::EloGap, m),
                // Bye-only rules: 0 on every real edge (they act on the bye edge).
                Rule::ByeGroup | Rule::ByeSelection => {}
            }
        }
        if need_phantom {
            let p = k;
            for i in 0..k {
                let c = bye_cost(&ctx, &model.rules, &model.mult, free[i]);
                cost[i * vcount + p] = c;
                cost[p * vcount + i] = c;
            }
        }
        let mate = solve_matching(&cost, vcount);
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
                swiss_bye = Some(free[real]);
            } else {
                pairs.push(PairedUnits {
                    a: free[i],
                    b: free[j],
                    source: PairingSource::Swiss,
                    points_diff: diff(free[i], free[j]),
                });
            }
        }
    }

    UnitPairing { pairs, swiss_bye }
}

// --- The individual-mode wrapper ------------------------------------------

/// Build the engine's input for an **individual** tournament: one unit per
/// player, keyed by their tournament number, from the replayed scores plus the
/// registration data the rules read (rating, club, nationality) and the
/// per-round cup and ELO context.
///
/// Gap keys (number 0, and any number freed by a pre-play removal) hold a default
/// unit, exactly as [`Scores`](crate::scoring::Scores) leaves gaps — the free set
/// never names them.
pub(crate) fn player_units(
    players: &[Player],
    settings: &TournamentSettings,
    completed_rounds: &[Round],
    prequalified: &[TournamentId],
) -> TiVec<UnitKey, PairingUnit> {
    let scores = compute_scores(players, settings, completed_rounds);
    let cap = scores.tid_capacity();
    let mut units: TiVec<UnitKey, PairingUnit> = vec![PairingUnit::default(); cap].into();

    // A live ELO estimate is only computed for the mode that pairs on it —
    // it replays every game, so it is far too expensive to take speculatively.
    let estimates = settings
        .elo_estimate_needed()
        .then(|| estimate_elos(players, settings, completed_rounds));

    for p in players {
        let Some(tid) = p.tournament_id else {
            continue; // not finalized yet, so on no board either
        };
        let s = scores.get_tid(tid);
        let key = UnitKey::from(tid);
        units[key] = PairingUnit {
            points: s.points(),
            macmahon: s.macmahon,
            opponents: s.opponents.iter().copied().map(UnitKey::from).collect(),
            had_bye: s.had_bye,
            last_ascended: s.last_ascended,
            last_descended: s.last_descended,
            rating: p.rating,
            // One board, so the affiliation rules' aligned-position count
            // degenerates to the individual mode's 0/1.
            clubs: vec![p
                .club
                .as_ref()
                .map(|c| TournamentSettings::normalize_club(c))],
            nationalities: vec![p
                .nationality
                .as_ref()
                .map(|n| TournamentSettings::normalize_nationality(n))],
            prequalified: false,
            elo: estimates
                .as_ref()
                .map(|est| {
                    est.get(&p.id)
                        .copied()
                        .unwrap_or(UNRATED_PRIOR_MEAN)
                        .round() as i64
                })
                .unwrap_or(0),
        };
    }
    for &tid in prequalified {
        units[UnitKey::from(tid)].prequalified = true;
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PointAdjustment;
    use crate::round::{Board, Outcome, Sitout, SitoutKind, SitoutValue, Winner};
    use crate::settings::{
        ClubProtection, MacMahonThreshold, NationalityProtection, RatioAtLeastOne,
    };
    use std::num::NonZeroU32;
    use uuid::Uuid;

    /// The engine seen through the individual-mode wrapper: build the player
    /// units, pair them, and reassemble the `Round` the caller would — so these
    /// tests keep speaking in players and boards, which is what the rules are
    /// actually about. Mirrors `Tournament::confirm_round_inner`'s Swiss half.
    #[allow(clippy::too_many_arguments)]
    fn pair_players(
        number: u32,
        players: &[Player],
        settings: &TournamentSettings,
        completed_rounds: &[Round],
        present: &[TournamentId],
        forced_boards: &[Board],
        forced_byes: &[TournamentId],
        prequalified: &[TournamentId],
    ) -> Round {
        let units = player_units(players, settings, completed_rounds, prequalified);
        let present: Vec<UnitKey> = present.iter().copied().map(UnitKey::from).collect();
        let forced_pairs: Vec<(UnitKey, UnitKey)> = forced_boards
            .iter()
            .map(|b| (UnitKey::from(b.player1), UnitKey::from(b.player2)))
            .collect();
        let forced_bye_keys: Vec<UnitKey> =
            forced_byes.iter().copied().map(UnitKey::from).collect();
        let paired = pair_round_weighted(
            number,
            settings,
            &units,
            &present,
            &forced_pairs,
            &forced_bye_keys,
        );
        let mut sitouts: Vec<Sitout> = forced_byes
            .iter()
            .map(|&player| Sitout {
                player,
                kind: SitoutKind::ForcedBye,
                value: SitoutValue::Full,
            })
            .collect();
        sitouts.extend(paired.swiss_bye.map(|key| Sitout {
            player: TournamentId::from(key),
            kind: SitoutKind::Bye,
            value: SitoutValue::Full,
        }));
        Round {
            number,
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
    fn explain_players(
        number: u32,
        players: &[Player],
        settings: &TournamentSettings,
        completed_rounds: &[Round],
        swiss_boards: &[(TournamentId, TournamentId)],
        bye: Option<TournamentId>,
        prequalified: &[TournamentId],
    ) -> RoundExplanation {
        let units = player_units(players, settings, completed_rounds, prequalified);
        explain_pairing(
            number,
            settings,
            &units,
            &keys(swiss_boards),
            bye.map(UnitKey::from),
        )
    }

    /// The counterfactual probes, likewise wrapped for the player path.
    #[allow(clippy::too_many_arguments)]
    fn force_players(
        number: u32,
        players: &[Player],
        settings: &TournamentSettings,
        completed_rounds: &[Round],
        swiss_boards: &[(TournamentId, TournamentId)],
        bye: Option<TournamentId>,
        prequalified: &[TournamentId],
        a: TournamentId,
        b: TournamentId,
    ) -> Counterfactual {
        let units = player_units(players, settings, completed_rounds, prequalified);
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

    #[allow(clippy::too_many_arguments)]
    fn forbid_players(
        number: u32,
        players: &[Player],
        settings: &TournamentSettings,
        completed_rounds: &[Round],
        swiss_boards: &[(TournamentId, TournamentId)],
        bye: Option<TournamentId>,
        prequalified: &[TournamentId],
        a: TournamentId,
        b: TournamentId,
    ) -> Counterfactual {
        let units = player_units(players, settings, completed_rounds, prequalified);
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
    fn keys(boards: &[(TournamentId, TournamentId)]) -> Vec<(UnitKey, UnitKey)> {
        boards
            .iter()
            .map(|&(a, b)| (UnitKey::from(a), UnitKey::from(b)))
            .collect()
    }

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
            let n = cost.len();
            let flat: Vec<i128> = cost.iter().flatten().copied().collect();
            assert_eq!(
                solve_matching(&flat, n),
                min_weight_perfect_matching(&flat, n)
            );
        }
    }

    // --- Weighted pairing -------------------------------------------------

    fn player(tid: u32, rating: Option<u32>, club: Option<&str>) -> Player {
        player_nat(tid, rating, club, None)
    }

    /// [`player`] plus a nationality, for the nationality-protection tests.
    fn player_nat(
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

    fn completed_round(
        number: u32,
        boards: &[(TournamentId, TournamentId, Winner)],
        bye: Option<TournamentId>,
    ) -> Round {
        Round {
            number,
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

    fn unord(a: TournamentId, b: TournamentId) -> (TournamentId, TournamentId) {
        if a < b {
            (a, b)
        } else {
            (b, a)
        }
    }

    fn board_pairs(round: &Round) -> HashSet<(TournamentId, TournamentId)> {
        round
            .boards
            .iter()
            .map(|b| unord(b.player1, b.player2))
            .collect()
    }

    #[test]
    fn pairing_is_independent_of_input_order() {
        // A genuinely tied field: two blocks of three equal-rated players in ELO
        // mode round 1. Any single cross-block board costs the same squared gap, so
        // *which* players cross is a real tie the rules can't resolve. Presenting the
        // exact same players (same ids, same tournament numbers) to the engine in a
        // different vector order must not change the pairing — the round is a
        // function of tournament state, not registration/import order.
        let p: Vec<Player> = vec![
            player(1, Some(2000), None),
            player(2, Some(2000), None),
            player(3, Some(2000), None),
            player(4, Some(1000), None),
            player(5, Some(1000), None),
            player(6, Some(1000), None),
        ];
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let settings = elo_settings();

        let forward = pair_players(1, &p, &settings, &[], &present, &[], &[], &[]);

        // Same players and tournament numbers, reversed order in both the players
        // slice and the present list.
        let mut p_rev = p.clone();
        p_rev.reverse();
        let present_rev: Vec<TournamentId> = present.iter().rev().copied().collect();
        let reversed = pair_players(1, &p_rev, &settings, &[], &present_rev, &[], &[], &[]);

        assert_eq!(board_pairs(&forward), board_pairs(&reversed));
        assert_eq!(forward.swiss_bye(), reversed.swiss_bye());
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
                (
                    p[0].tournament_id.unwrap(),
                    p[1].tournament_id.unwrap(),
                    Winner::Player1,
                ),
                (
                    p[2].tournament_id.unwrap(),
                    p[3].tournament_id.unwrap(),
                    Winner::Player1,
                ),
            ],
            None,
        );
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

        let round = pair_players(
            2,
            &p,
            &TournamentSettings::default(),
            &[r1],
            &present,
            &[],
            &[],
            &[],
        );

        assert_eq!(round.swiss_bye(), None);
        assert_eq!(round.boards.len(), 2);
        let pairs = board_pairs(&round);
        // Same-score, no rematch: winners together, losers together.
        assert!(pairs.contains(&unord(
            p[0].tournament_id.unwrap(),
            p[2].tournament_id.unwrap()
        )));
        assert!(pairs.contains(&unord(
            p[1].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )));
    }

    #[test]
    fn weighted_avoids_repeat_bye() {
        let p: Vec<Player> = (1..=3).map(|i| player(i, Some(1500), None)).collect();
        // Round 1: p0 beat p1; p2 took the bye (so p0 and p2 have 1 victory).
        let r1 = completed_round(
            1,
            &[(
                p[0].tournament_id.unwrap(),
                p[1].tournament_id.unwrap(),
                Winner::Player1,
            )],
            Some(p[2].tournament_id.unwrap()),
        );
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

        let round = pair_players(
            2,
            &p,
            &TournamentSettings::default(),
            &[r1],
            &present,
            &[],
            &[],
            &[],
        );

        // p2 already had a bye, so it must fall elsewhere; giving it to p1 also
        // leaves the same-score board p0 vs p2.
        assert_eq!(round.swiss_bye(), Some(p[1].tournament_id.unwrap()));
        assert_eq!(
            board_pairs(&round),
            HashSet::from([unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )])
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
                (
                    p[0].tournament_id.unwrap(),
                    p[1].tournament_id.unwrap(),
                    Winner::Player1,
                ),
                (
                    p[2].tournament_id.unwrap(),
                    p[3].tournament_id.unwrap(),
                    Winner::Player1,
                ),
            ],
            Some(p[4].tournament_id.unwrap()),
        );
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

        let round = pair_players(
            2,
            &p,
            &TournamentSettings::default(),
            &[r1],
            &present,
            &[],
            &[],
            &[],
        );

        let bye = round.swiss_bye().expect("odd field needs a bye");
        assert!(
            bye == p[1].tournament_id.unwrap() || bye == p[3].tournament_id.unwrap(),
            "bye went to a leader"
        );
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
                (
                    p[0].tournament_id.unwrap(),
                    p[1].tournament_id.unwrap(),
                    Winner::Player1,
                ), // A beats B
                (
                    p[2].tournament_id.unwrap(),
                    p[3].tournament_id.unwrap(),
                    Winner::Player1,
                ), // C beats D
            ],
            None,
        );
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        // A (1 pt) vs D (0 pt), forced.
        let forced = vec![Board::pending(
            p[0].tournament_id.unwrap(),
            p[3].tournament_id.unwrap(),
            0,
            PairingSource::Swiss,
        )];

        let round = pair_players(
            2,
            &p,
            &TournamentSettings::default(),
            &[r1],
            &present,
            &forced,
            &[],
            &[],
        );

        // The forced A-vs-D board freezes the float A had going in: +2 half-points
        // (A on 1 point = 2 halves, D on 0). Only its sign matters to the float
        // history.
        let ad = round
            .boards
            .iter()
            .find(|b| {
                b.player1 == p[0].tournament_id.unwrap() && b.player2 == p[3].tournament_id.unwrap()
            })
            .expect("forced board present");
        assert_eq!(ad.points_diff, 2);
    }

    #[test]
    fn a_half_point_reaches_pairing_at_half_point_granularity() {
        // C sits out round 1 for half a point (a `0=`, 1 half-unit) and carries it
        // into round 2, while A has a full point (2). The engine sees the scores at
        // half-point granularity: A and C are one half-point apart, so their round-2
        // board freezes an *odd* points_diff (±1) — a float whole-point scoring
        // could never produce. B (lowest) takes the bye.
        let a = player(1, Some(2000), None);
        let b = player(2, Some(1500), None);
        let c = player(3, Some(1000), None);
        let players = vec![a.clone(), b.clone(), c.clone()];
        let settings = TournamentSettings::default();

        // Round 1: A beats B; C is absent for half a point.
        let r1 = Round {
            number: 1,
            boards: vec![Board {
                outcome: Outcome::won(Winner::Player1),
                ..Board::pending(
                    a.tournament_id.unwrap(),
                    b.tournament_id.unwrap(),
                    0,
                    PairingSource::Swiss,
                )
            }],
            sitouts: vec![Sitout {
                player: c.tournament_id.unwrap(),
                kind: SitoutKind::Absent,
                value: SitoutValue::Half,
            }],
            completed: true,
        };

        let present = vec![
            a.tournament_id.unwrap(),
            b.tournament_id.unwrap(),
            c.tournament_id.unwrap(),
        ];
        let round = pair_players(
            2,
            &players,
            &settings,
            std::slice::from_ref(&r1),
            &present,
            &[],
            &[],
            &[],
        );

        assert_eq!(round.swiss_bye(), Some(b.tournament_id.unwrap())); // lowest score takes the bye
        let ac = round
            .boards
            .iter()
            .find(|bd| {
                (bd.player1 == a.tournament_id.unwrap() && bd.player2 == c.tournament_id.unwrap())
                    || (bd.player1 == c.tournament_id.unwrap()
                        && bd.player2 == a.tournament_id.unwrap())
            })
            .expect("A vs C paired");
        assert_eq!(ac.points_diff.abs(), 1); // A(2) vs C(1) — an odd float
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
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let settings =
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);

        let round = pair_players(1, &p, &settings, &[], &present, &[], &[], &[]);

        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(
                p[0].tournament_id.unwrap(),
                p[1].tournament_id.unwrap()
            )),
            "top MacMahon group paired within itself"
        );
        assert!(
            pairs.contains(&unord(
                p[2].tournament_id.unwrap(),
                p[3].tournament_id.unwrap()
            )),
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
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let settings = TournamentSettings::default().with_club(ClubProtection::On {
            rounds: None,
            exempt_clubs: Vec::new(),
        });

        let round = pair_players(1, &p, &settings, &[], &present, &[], &[], &[]);

        assert_eq!(round.boards.len(), 2);
        let club_of = |id: TournamentId| {
            p.iter()
                .find(|q| q.tournament_id.unwrap() == id)
                .unwrap()
                .club
                .clone()
        };
        for b in &round.boards {
            assert_ne!(
                club_of(b.player1),
                club_of(b.player2),
                "club-mates were paired despite protection"
            );
        }
    }

    /// The club rule counts the same-club *games* an edge would create, over
    /// aligned board positions — the form that serves a multi-board unit (a team)
    /// as well as a player. Positions are aligned because board `k` only ever
    /// meets board `k`, so a shared club on different boards costs nothing.
    #[test]
    fn the_club_rule_counts_aligned_same_club_boards() {
        let unit = |clubs: &[Option<&str>]| PairingUnit {
            clubs: clubs
                .iter()
                .map(|c| c.map(TournamentSettings::normalize_club))
                .collect(),
            ..PairingUnit::default()
        };
        // Board 1 shares club X, board 2 differs, board 3 shares club Z — but the
        // Y on a's board 2 also appears on b's board 3, on a board it never plays.
        let units: TiVec<UnitKey, PairingUnit> = vec![
            PairingUnit::default(), // key 0, the phantom's gap slot
            unit(&[Some("X"), Some("Y"), Some("Z")]),
            unit(&[Some("x"), Some("W"), Some("Z")]), // case-insensitive on board 1
            unit(&[None, None, None]),
        ]
        .into();
        let free = [UnitKey(1), UnitKey(2), UnitKey(3)];
        let exempt_clubs = HashSet::new();
        let exempt_nationalities = HashSet::new();
        let empty_rank: TiVec<UnitKey, i128> = vec![0i128; units.len()].into();
        let fold = fold_ranks(&units, &free);
        let ctx = Ctx {
            units: &units,
            fold: &fold,
            round: 1,
            floater_style: FloaterStyle::Classic,
            exempt_clubs: &exempt_clubs,
            exempt_nationalities: &exempt_nationalities,
            edges: 1,
            max_gap: 0,
            min_points: 0,
            max_mm_gap: 0,
            max_group: 3,
            free_count: 3,
            max_boards: 3,
            elo_rank: &empty_rank,
            max_elo_gap: 0,
        };
        // Two aligned clashes (boards 1 and 3), not three: the Y/W board differs,
        // and a's Y never meets b's Y because they sit on different boards.
        assert_eq!(Rule::Club.edge_units(&ctx, UnitKey(1), UnitKey(2)), 2);
        // An unknown club is never a clash.
        assert_eq!(Rule::Club.edge_units(&ctx, UnitKey(1), UnitKey(3)), 0);
        // ...and the ladder bound is read off the instance, so it still covers the
        // worst edge — the check that keeps the lexicographic separation exact
        // once an edge can emit more than one unit.
        let bound = Rule::Club.max_total_units(&ctx);
        assert!(Rule::Club.edge_units(&ctx, UnitKey(1), UnitKey(2)) * ctx.edges <= bound);
        assert_eq!(bound, 3);
    }

    #[test]
    fn club_protection_off_by_default_pairs_the_fold() {
        // With protection off (the default), the club rule is silent, so the fold
        // ideal wins and club-mates X-X / Y-Y are paired.
        let p = two_clubs_where_fold_pairs_mates();
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

        let round = pair_players(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &present,
            &[],
            &[],
            &[],
        );

        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )),
            "fold pairs the X club-mates"
        );
        assert!(
            pairs.contains(&unord(
                p[1].tournament_id.unwrap(),
                p[3].tournament_id.unwrap()
            )),
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
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

        let exempt = TournamentSettings::default().with_club(ClubProtection::On {
            rounds: None,
            exempt_clubs: vec!["  HOME ".into()],
        });
        let round = pair_players(1, &p, &exempt, &[], &present, &[], &[], &[]);
        assert!(
            board_pairs(&round).contains(&unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )),
            "exempt club-mates should be paired by the fold"
        );

        let protected = TournamentSettings::default().with_club(ClubProtection::On {
            rounds: None,
            exempt_clubs: Vec::new(),
        });
        let round = pair_players(1, &p, &protected, &[], &present, &[], &[], &[]);
        assert!(
            !board_pairs(&round).contains(&unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )),
            "non-exempt club-mates should not be paired"
        );
    }

    #[test]
    fn club_protection_only_applies_within_its_round_window() {
        // Protection limited to round 1: round 2 must ignore clubs, so the fold
        // ideal (club-mate pairs) wins again.
        let p = two_clubs_where_fold_pairs_mates();
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let settings = TournamentSettings::default().with_club(ClubProtection::On {
            rounds: NonZeroU32::new(1),
            exempt_clubs: Vec::new(),
        });

        // Pair round 2 directly (no completed rounds needed to exercise the window).
        let round = pair_players(2, &p, &settings, &[], &present, &[], &[], &[]);
        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )),
            "past the window, fold pairs X-X"
        );
        assert!(
            pairs.contains(&unord(
                p[1].tournament_id.unwrap(),
                p[3].tournament_id.unwrap()
            )),
            "past the window, fold pairs Y-Y"
        );
    }

    /// Round 1, one score group of four, rating order p0>p1>p2>p3 so the fold
    /// ideal is p0-p2 and p1-p3 — which are compatriots (JP and FR). The
    /// nationality twin of [`two_clubs_where_fold_pairs_mates`].
    fn two_nationalities_where_fold_pairs_compatriots() -> Vec<Player> {
        vec![
            player_nat(1, Some(2000), None, Some("JP")),
            player_nat(2, Some(1900), None, Some("FR")),
            player_nat(3, Some(1800), None, Some("JP")),
            player_nat(4, Some(1700), None, Some("FR")),
        ]
    }

    #[test]
    fn weighted_avoids_pairing_compatriots_when_protection_on() {
        let p = two_nationalities_where_fold_pairs_compatriots();
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let settings = TournamentSettings::default().with_nationality(NationalityProtection::On {
            rounds: None,
            exempt_nationalities: Vec::new(),
        });

        let round = pair_players(1, &p, &settings, &[], &present, &[], &[], &[]);

        assert_eq!(round.boards.len(), 2);
        let nationality_of = |id: TournamentId| {
            p.iter()
                .find(|q| q.tournament_id.unwrap() == id)
                .unwrap()
                .nationality
                .clone()
        };
        for b in &round.boards {
            assert_ne!(
                nationality_of(b.player1),
                nationality_of(b.player2),
                "compatriots were paired despite protection"
            );
        }
    }

    #[test]
    fn nationality_protection_off_by_default_pairs_the_fold() {
        // With protection off (the default), the nationality rule is silent, so
        // the fold ideal wins and the compatriots JP-JP / FR-FR are paired.
        let p = two_nationalities_where_fold_pairs_compatriots();
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

        let round = pair_players(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &present,
            &[],
            &[],
            &[],
        );

        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )),
            "fold pairs the JP compatriots"
        );
        assert!(
            pairs.contains(&unord(
                p[1].tournament_id.unwrap(),
                p[3].tournament_id.unwrap()
            )),
            "fold pairs the FR compatriots"
        );
    }

    #[test]
    fn exempt_nationality_members_may_be_paired() {
        // Fold ideal is p0-p2 (both "JP", the host country) and p1-p3 (both of
        // unknown nationality). With protection on but JP exempt (spelled
        // differently to prove the match is case-insensitive), the JP pair is
        // allowed and the fold wins; without the exemption it is broken up.
        let p = vec![
            player_nat(1, Some(2000), None, Some("JP")),
            player_nat(2, Some(1900), None, None),
            player_nat(3, Some(1800), None, Some("JP")),
            player_nat(4, Some(1700), None, None),
        ];
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

        let exempt = TournamentSettings::default().with_nationality(NationalityProtection::On {
            rounds: None,
            exempt_nationalities: vec![" jp ".into()],
        });
        let round = pair_players(1, &p, &exempt, &[], &present, &[], &[], &[]);
        assert!(
            board_pairs(&round).contains(&unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )),
            "exempt compatriots should be paired by the fold"
        );

        let protected = TournamentSettings::default().with_nationality(NationalityProtection::On {
            rounds: None,
            exempt_nationalities: Vec::new(),
        });
        let round = pair_players(1, &p, &protected, &[], &present, &[], &[], &[]);
        assert!(
            !board_pairs(&round).contains(&unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )),
            "non-exempt compatriots should not be paired"
        );
    }

    #[test]
    fn nationality_protection_only_applies_within_its_round_window() {
        // Protection limited to round 1: round 2 must ignore nationalities, so
        // the fold ideal (compatriot pairs) wins again.
        let p = two_nationalities_where_fold_pairs_compatriots();
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let settings = TournamentSettings::default().with_nationality(NationalityProtection::On {
            rounds: NonZeroU32::new(1),
            exempt_nationalities: Vec::new(),
        });

        // Pair round 2 directly (no completed rounds needed to exercise the window).
        let round = pair_players(2, &p, &settings, &[], &present, &[], &[], &[]);
        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )),
            "past the window, fold pairs JP-JP"
        );
        assert!(
            pairs.contains(&unord(
                p[1].tournament_id.unwrap(),
                p[3].tournament_id.unwrap()
            )),
            "past the window, fold pairs FR-FR"
        );
    }

    /// The point of the whole rule: nationality protection is *weaker* than club
    /// protection, so when only one of the two can be honoured the club clash is
    /// the one avoided.
    ///
    /// Four players in one score group, with the fold-ideal matching (p0-p2,
    /// p1-p3) ruled out as a double rematch. That leaves two candidates:
    ///   * p0-p1 / p2-p3 — one nationality clash (p0, p1 are both JP), fold 20;
    ///   * p0-p3 / p1-p2 — one club clash (p1, p2 are both "A"), fold 4.
    ///
    /// The second is much the better fold and the only nationality-clean one, so
    /// it is what any ordering *other* than club-above-nationality would pick.
    #[test]
    fn club_protection_outranks_nationality_protection() {
        let p = vec![
            player_nat(1, Some(2000), Some("B"), Some("JP")),
            player_nat(2, Some(1900), Some("A"), Some("JP")),
            player_nat(3, Some(1800), Some("A"), Some("FR")),
            player_nat(4, Some(1700), Some("C"), Some("DE")),
        ];
        let id = |i: usize| p[i].tournament_id.unwrap();
        let present: Vec<TournamentId> = (0..4).map(id).collect();
        // Round 1 played the fold ideal, so round 2 cannot repeat it. The losers'
        // adjustments put all four back on one point, keeping a single score
        // group — otherwise the score-gap rule, far above both protections,
        // would decide the round on its own.
        let r1 = [completed_round(
            1,
            &[
                (id(0), id(2), Winner::Player1),
                (id(1), id(3), Winner::Player1),
            ],
            None,
        )];
        let mut p = p.clone();
        for i in [2, 3] {
            p[i].adjustments.push(PointAdjustment {
                id: Uuid::new_v4(),
                delta: 1,
                reason: "level the score group".into(),
            });
        }

        // With nationality protection alone, the club clash is free and the
        // better fold wins — so the two candidates really are in tension, and
        // the assertion below is about the priority and not about one matching
        // being better on every count.
        let nationality_only =
            TournamentSettings::default().with_nationality(NationalityProtection::On {
                rounds: None,
                exempt_nationalities: Vec::new(),
            });
        let round = pair_players(2, &p, &nationality_only, &r1, &present, &[], &[], &[]);
        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(id(0), id(3))) && pairs.contains(&unord(id(1), id(2))),
            "without club protection the nationality-clean, better-fold matching \
             should win; got {pairs:?}"
        );

        let both = nationality_only.with_club(ClubProtection::On {
            rounds: None,
            exempt_clubs: Vec::new(),
        });
        let round = pair_players(2, &p, &both, &r1, &present, &[], &[], &[]);
        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(id(0), id(1))) && pairs.contains(&unord(id(2), id(3))),
            "the engine should accept the nationality clash (JP-JP) to avoid the \
             club clash (A-A), even at a worse fold; got {pairs:?}"
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
        let settings_base =
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let r1 = completed_round(
            1,
            &[
                (
                    p[0].tournament_id.unwrap(),
                    p[1].tournament_id.unwrap(),
                    Winner::Player2,
                ), // p1 beats p0 (top group upset)
                (
                    p[2].tournament_id.unwrap(),
                    p[3].tournament_id.unwrap(),
                    Winner::Player1,
                ), // p2 beats p3
                (
                    p[4].tournament_id.unwrap(),
                    p[5].tournament_id.unwrap(),
                    Winner::Player2,
                ), // p5 beats p4 (bottom group upset)
                (
                    p[6].tournament_id.unwrap(),
                    p[7].tournament_id.unwrap(),
                    Winner::Player1,
                ), // p6 beats p7
            ],
            None,
        );

        // Without airtight groups, score-gap alone finds a cheaper matching that
        // crosses the MacMahon boundary twice.
        let round_off = pair_players(
            2,
            &p,
            &settings_base,
            std::slice::from_ref(&r1),
            &present,
            &[],
            &[],
            &[],
        );
        let top: HashSet<TournamentId> = [
            p[0].tournament_id.unwrap(),
            p[1].tournament_id.unwrap(),
            p[2].tournament_id.unwrap(),
            p[3].tournament_id.unwrap(),
        ]
        .into_iter()
        .collect();
        let crosses = round_off
            .boards
            .iter()
            .filter(|b| top.contains(&b.player1) != top.contains(&b.player2))
            .count();
        assert_eq!(crosses, 2, "score-gap alone crosses the MacMahon boundary");

        // With airtight groups active for round 2, every board stays within its
        // MacMahon group.
        let settings_on = settings_base.with_airtight(NonZeroU32::new(2));
        let round_on = pair_players(2, &p, &settings_on, &[r1], &present, &[], &[], &[]);
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
        let settings = TournamentSettings::default()
            .with_thresholds(vec![MacMahonThreshold::elo(1500)])
            .with_airtight(NonZeroU32::new(1));
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let r1 = completed_round(
            1,
            &[
                (
                    p[0].tournament_id.unwrap(),
                    p[1].tournament_id.unwrap(),
                    Winner::Player2,
                ),
                (
                    p[2].tournament_id.unwrap(),
                    p[3].tournament_id.unwrap(),
                    Winner::Player1,
                ),
                (
                    p[4].tournament_id.unwrap(),
                    p[5].tournament_id.unwrap(),
                    Winner::Player2,
                ),
                (
                    p[6].tournament_id.unwrap(),
                    p[7].tournament_id.unwrap(),
                    Winner::Player1,
                ),
            ],
            None,
        );

        let round = pair_players(2, &p, &settings, &[r1], &present, &[], &[], &[]);
        let top: HashSet<TournamentId> = [
            p[0].tournament_id.unwrap(),
            p[1].tournament_id.unwrap(),
            p[2].tournament_id.unwrap(),
            p[3].tournament_id.unwrap(),
        ]
        .into_iter()
        .collect();
        let crosses = round
            .boards
            .iter()
            .filter(|b| top.contains(&b.player1) != top.contains(&b.player2))
            .count();
        assert_eq!(crosses, 2, "past the window, score-gap crosses again");
    }

    // --- ELO (non-Swiss) mode ---------------------------------------------

    fn elo_settings() -> TournamentSettings {
        // Neutralize the provisional-rating widening so these pairing tests
        // exercise the base drift; reliability is covered in elo.rs tests.
        TournamentSettings::elo_pairing()
            .map_estimator(|e| e.provisional_multiplier = RatioAtLeastOne::from_percent(100))
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
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

        let round = pair_players(1, &p, &elo_settings(), &[], &present, &[], &[], &[]);

        let pairs = board_pairs(&round);
        assert!(
            pairs.contains(&unord(
                p[0].tournament_id.unwrap(),
                p[1].tournament_id.unwrap()
            )),
            "closest pair 2000-1950"
        );
        assert!(
            pairs.contains(&unord(
                p[2].tournament_id.unwrap(),
                p[3].tournament_id.unwrap()
            )),
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
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

        let round = pair_players(1, &p, &elo_settings(), &[], &present, &[], &[], &[]);

        assert_eq!(
            round.swiss_bye(),
            Some(p[4].tournament_id.unwrap()),
            "the weakest player takes the bye"
        );
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
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let settings = elo_settings();

        // 1950 (p1) beat 2000 (p0); 1450 (p3) beat 1500 (p2).
        let r1 = completed_round(
            1,
            &[
                (
                    p[1].tournament_id.unwrap(),
                    p[0].tournament_id.unwrap(),
                    Winner::Player1,
                ),
                (
                    p[3].tournament_id.unwrap(),
                    p[2].tournament_id.unwrap(),
                    Winner::Player1,
                ),
            ],
            None,
        );

        let round = pair_players(2, &p, &settings, &[r1], &present, &[], &[], &[]);
        let pairs = board_pairs(&round);
        // No rematch of the round-1 boards.
        assert!(!pairs.contains(&unord(
            p[0].tournament_id.unwrap(),
            p[1].tournament_id.unwrap()
        )));
        assert!(!pairs.contains(&unord(
            p[2].tournament_id.unwrap(),
            p[3].tournament_id.unwrap()
        )));
        // Winners (raised estimates) meet, losers meet.
        assert!(
            pairs.contains(&unord(
                p[1].tournament_id.unwrap(),
                p[3].tournament_id.unwrap()
            )),
            "the two winners are paired"
        );
        assert!(
            pairs.contains(&unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )),
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
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        // X0..X2 on 1 point, Y on 0
        let settings =
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);

        let round = pair_players(1, &p, &settings, &[], &present, &[], &[], &[]);
        assert!(
            board_pairs(&round).contains(&unord(
                p[2].tournament_id.unwrap(),
                p[3].tournament_id.unwrap()
            )),
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
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let base =
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);

        let classic = base.clone().with_floater(FloaterStyle::Classic);
        let round = pair_players(1, &p, &classic, &[], &present, &[], &[], &[]);
        assert!(
            board_pairs(&round).contains(&unord(
                p[0].tournament_id.unwrap(),
                p[1].tournament_id.unwrap()
            )),
            "classic Swiss floats up the strongest of the group (L0)"
        );

        let median = base.with_floater(FloaterStyle::Median);
        let round = pair_players(1, &p, &median, &[], &present, &[], &[], &[]);
        assert!(
            board_pairs(&round).contains(&unord(
                p[0].tournament_id.unwrap(),
                p[2].tournament_id.unwrap()
            )),
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
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();
        let base =
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);

        let classic = base.clone().with_floater(FloaterStyle::Classic);
        let round = pair_players(1, &p, &classic, &[], &present, &[], &[], &[]);
        assert!(
            board_pairs(&round).contains(&unord(
                p[2].tournament_id.unwrap(),
                p[3].tournament_id.unwrap()
            )),
            "classic Swiss floats down the weakest of the group (X2)"
        );

        let median = base.with_floater(FloaterStyle::Median);
        let round = pair_players(1, &p, &median, &[], &present, &[], &[], &[]);
        assert!(
            board_pairs(&round).contains(&unord(
                p[1].tournament_id.unwrap(),
                p[3].tournament_id.unwrap()
            )),
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
        let present: Vec<TournamentId> = p.iter().map(|x| x.tournament_id.unwrap()).collect();

        let classic = TournamentSettings::default().with_floater(FloaterStyle::Classic);
        let round = pair_players(1, &p, &classic, &[], &present, &[], &[], &[]);
        assert_eq!(
            round.swiss_bye(),
            Some(p[4].tournament_id.unwrap()),
            "classic Swiss gives the bye to the weakest of the group (P4)"
        );

        let median = TournamentSettings::default().with_floater(FloaterStyle::Median);
        let round = pair_players(1, &p, &median, &[], &present, &[], &[], &[]);
        assert_eq!(
            round.swiss_bye(),
            Some(p[2].tournament_id.unwrap()),
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
    #[should_panic(expected = "overflowed i128")]
    fn scale_ladder_aborts_on_overflow_instead_of_wrapping() {
        // Two astronomically large worst-case totals push the running product past
        // i128. Without the checked arithmetic this would wrap silently in release
        // and corrupt the lexicographic ordering; it must abort loudly instead.
        let _ = scale_ladder(&[i128::MAX / 2, i128::MAX / 2]);
    }

    #[test]
    #[should_panic(expected = "i128 headroom")]
    fn solve_matching_rejects_costs_beyond_i128_headroom() {
        // A 2×2 cost matrix whose off-diagonal cost is above the solver's i128
        // headroom (MAX/16): the solver's internal doubling would overflow, so the
        // dispatch must reject it rather than hand it over to be silently mangled.
        let big = i128::MAX / 2;
        let _ = solve_matching(&[0, big, big, 0], 2);
    }

    #[test]
    fn bye_only_rules_are_dropped_from_the_ladder_on_an_even_field() {
        let settings = TournamentSettings::default();
        let has_bye_rule = |need_phantom: bool, n: u32| {
            let p: Vec<Player> = (1..=n).map(|i| player(i, Some(1500), None)).collect();
            let free: Vec<UnitKey> = (1..=n).map(UnitKey).collect();
            let units = player_units(&p, &settings, &[], &[]);
            let model = PairingModel::build(1, &settings, &units, &free, need_phantom);
            model
                .rules
                .iter()
                .any(|r| matches!(r, Rule::ByeGroup | Rule::ByeSelection))
        };
        // No phantom (even field) → the bye-only rules can never fire, so they must
        // not reserve a ladder tier. A phantom (odd field) → they stay.
        assert!(
            !has_bye_rule(false, 4),
            "an even field must not reserve a bye-rule tier"
        );
        assert!(has_bye_rule(true, 3), "an odd field keeps the bye rules");
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
        let id = |i: usize| p[i].tournament_id.unwrap();
        let r1 = completed_round(
            1,
            &[
                (id(0), id(1), Winner::Player1),
                (id(2), id(3), Winner::Player1),
            ],
            Some(id(4)), // p5 took a bye
        );
        let settings =
            TournamentSettings::default().with_thresholds(vec![MacMahonThreshold::elo(1500)]);
        let units = player_units(&p, &settings, &[r1], &[]);
        let free: Vec<UnitKey> = p
            .iter()
            .map(|q| UnitKey::from(q.tournament_id.unwrap()))
            .collect();
        let (mut lo, mut hi) = (u32::MAX, 0u32);
        let (mut mm_lo, mut mm_hi) = (u32::MAX, 0u32);
        for &key in &free {
            let s = &units[key];
            lo = lo.min(s.points.halves());
            hi = hi.max(s.points.halves());
            mm_lo = mm_lo.min(s.macmahon.halves());
            mm_hi = mm_hi.max(s.macmahon.halves());
        }
        let edges = 3i128; // 5 free + phantom bye = 6 vertices → 3 edges
        let exempt_clubs = HashSet::new();
        let exempt_nationalities = HashSet::new();
        let fold = fold_ranks(&units, &free);
        let empty_rank: TiVec<UnitKey, i128> = vec![0i128; units.len()].into();
        let ctx = Ctx {
            units: &units,
            fold: &fold,
            round: 2,
            floater_style: FloaterStyle::Median, // exercise the floater-selection bound
            exempt_clubs: &exempt_clubs,
            exempt_nationalities: &exempt_nationalities,
            edges,
            max_gap: (hi - lo) as i128,
            min_points: lo as i128,
            max_mm_gap: (mm_hi - mm_lo) as i128,
            max_group: fold
                .iter()
                .flatten()
                .map(|f| f.group_size)
                .max()
                .unwrap_or(0) as i128,
            free_count: free.len() as i128,
            max_boards: free
                .iter()
                .map(|&k| units[k].clubs.len())
                .max()
                .unwrap_or(0) as i128,
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
        let boards = [
            (p[0].tournament_id.unwrap(), p[1].tournament_id.unwrap()),
            (p[2].tournament_id.unwrap(), p[3].tournament_id.unwrap()),
        ];

        let ex = explain_players(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &boards,
            None,
            &[],
        );

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

        let ex = explain_players(
            1,
            &p,
            &TournamentSettings::default(),
            &[],
            &boards,
            None,
            &[],
        );

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
        let round = pair_players(1, &p, &settings, &[], &present, &[], &[], &[]);

        let boards: Vec<(TournamentId, TournamentId)> = round
            .boards
            .iter()
            .map(|b| (b.player1, b.player2))
            .collect();
        let ex = explain_players(1, &p, &settings, &[], &boards, round.swiss_bye(), &[]);

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

    // --- Counterfactual ---------------------------------------------------

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
            &[],
            &boards,
            None,
            &[],
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
            &[],
            &boards,
            None,
            &[],
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
            &[],
            &boards,
            None,
            &[],
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
            &[],
            &[],
            &[],
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
            &[],
            &boards,
            Some(bye),
            &[],
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
            &[],
            &boards,
            None,
            &[],
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
            &[],
            &boards,
            None,
            &[],
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
            &[],
            &[],
            &[],
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
            &[],
            &boards,
            Some(bye),
            &[],
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
            &[],
            &[],
            &[],
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
            &[],
            &boards,
            Some(bye),
            &[],
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
