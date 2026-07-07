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
//! 2. **Equal scores** — prefer opponents with the same number of points (each
//!    player's MacMahon starting points plus their victories); the penalty grows
//!    with the *square* of the points gap.
//! 3. **No repeated float** — avoid making a player an ascending floater (meeting
//!    someone with more points) or a descending floater (fewer points, or a
//!    bye) twice; the penalty fades with the number of rounds since the last such
//!    float.
//! 4. **Floater selection** — when a group has to pair across score groups, choose
//!    *who* floats: the descending floater should be the last (weakest) of the
//!    upper group, and the ascending floater the first (classic Swiss) or the
//!    median (median Swiss) of the lower group. The penalty rises with the
//!    distance from that ideal in-group rank.
//! 5. **Different clubs** — avoid pairing club-mates (ignored when a club is
//!    unknown).
//! 6. **Fold within a score group** — sort a group (equal points) by rating
//!    (unrated = 1), descending; the Nth player of the top half should meet the
//!    Nth of the bottom half, penalized by how far the actual pairing deviates.
//!
//! Priority lives in exactly one place — the order of [`Rule::ORDER`] — and the
//! separation between tiers is proven by construction (see [`scale_ladder`]), so
//! adding or reordering rules stays sound with no magic numbers to retune.
//!
//! [`pair_round_weighted`] is the real pairing path. [`pair_round`] /
//! [`pair_round_constrained`] remain as the trivial uniform-weight baseline
//! (used by the odd unit test); the bye is modeled as a phantom vertex.
//!
//! An ILP/CP-SAT backend is still planned (see TODO.md) for very large fields and
//! for formats needing hard constraints a plain matching can't express.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::elo::{estimate_elos, UNRATED_PRIOR_MEAN};
use crate::matching::min_weight_perfect_matching;
use crate::player::Player;
use crate::round::{Board, PairingSource, Round};
use crate::scoring::{compute_scores, Scores};
use crate::settings::{FloaterStyle, TournamentSettings};

/// Pair all the given players with no constraints (naïve mode).
pub fn pair_round(number: u32, player_ids: &[Uuid]) -> Round {
    pair_round_constrained(number, player_ids, &[], None)
}

/// Pair the `present` players, honoring `forced_boards` and an optional
/// `forced_bye`. Players not covered by a constraint are paired consecutively;
/// an odd leftover takes the bye unless one is already forced.
///
/// Preconditions (validated by the caller before generation): every forced
/// player is present and appears at most once, and with a forced bye the number
/// of leftover players is even.
pub fn pair_round_constrained(
    number: u32,
    present: &[Uuid],
    forced_boards: &[Board],
    forced_bye: Option<Uuid>,
) -> Round {
    let mut placed: HashSet<Uuid> = HashSet::new();
    for board in forced_boards {
        placed.insert(board.player1);
        placed.insert(board.player2);
    }
    if let Some(bye) = forced_bye {
        placed.insert(bye);
    }

    let mut remaining: Vec<Uuid> = present
        .iter()
        .copied()
        .filter(|id| !placed.contains(id))
        .collect();

    let bye = match forced_bye {
        Some(bye) => Some(bye),
        None if remaining.len() % 2 == 1 => remaining.pop(),
        None => None,
    };

    let mut boards: Vec<Board> = forced_boards
        .iter()
        .map(|b| Board::pending(b.player1, b.player2, None, PairingSource::Forced))
        .collect();
    for pair in remaining.chunks(2) {
        boards.push(Board::pending(pair[0], pair[1], None, PairingSource::Swiss));
    }

    Round {
        number,
        boards,
        bye,
        absent: Vec::new(),
        completed: false,
    }
}

// --- Weighted matching ----------------------------------------------------

/// Numerator of the float-repeat penalty, divided by the number of rounds since
/// the player last floated the same way. Chosen with many small divisors so the
/// decay reads smoothly.
const FLOAT_BASE: i128 = 720;

/// The pairing rules. The active subset and its priority order depend on the
/// mode (see [`active_rules`]); that ordering is the single source of truth for
/// priority, and the scalar multipliers are derived from it (see
/// [`scale_ladder`]).
#[derive(Clone, Copy)]
enum Rule {
    /// Never play the same opponent twice / never take the bye twice.
    Rematch,
    /// Prefer equal scores; penalty grows with the square of the points gap.
    ScoreGap,
    /// Avoid repeating a float in the same direction; decays with rounds since.
    FloatRepeat,
    /// When a pairing floats across groups, choose the right players to float:
    /// the weakest of the upper group descends, and the first/median of the lower
    /// group ascends.
    FloaterSelection,
    /// Avoid pairing club-mates (ignored when a club is unknown).
    Club,
    /// Fold within a score group (top half meets bottom half).
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

/// The rules in effect, highest priority first, for the active mode. Swiss/
/// MacMahon is the default; the experimental ELO mode swaps the whole
/// score/float/fold/club family for a bye-selection rule and a squared-ELO-gap
/// rule, keeping only no-rematch above them.
fn active_rules(settings: &TournamentSettings) -> &'static [Rule] {
    const SWISS: [Rule; 6] = [
        Rule::Rematch,
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
    /// Clubs exempt from protection, in normalized form (see
    /// [`TournamentSettings::normalize_club`]).
    exempt_clubs: &'a HashSet<String>,
    /// Edges in a perfect matching over the vertices (= vertices / 2).
    edges: i128,
    /// Largest points gap between any two vertices (bounds the score rule).
    max_gap: i128,
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
/// ideal position for its float direction. A descending floater ideally sits last
/// (weakest) in its group; an ascending floater ideally sits first (classic) or at
/// the median (median Swiss). 0 if the player has no fold info (shouldn't happen
/// for free players) or its group is a singleton.
fn floater_units(ctx: &Ctx, id: Uuid, descending: bool) -> i128 {
    let Some(f) = ctx.fold.get(&id) else {
        return 0;
    };
    let ideal = if descending {
        f.group_size.saturating_sub(1)
    } else {
        match ctx.floater_style {
            FloaterStyle::Classic => 0,
            FloaterStyle::Median => f.group_size / 2,
        }
    };
    (f.rank as i128 - ideal as i128).abs()
}

impl Rule {
    /// Penalty units for pairing `a` against `b` (before the priority multiplier).
    fn edge_units(self, ctx: &Ctx, a: Uuid, b: Uuid) -> i128 {
        let sa = ctx.scores.get(&a);
        let sb = ctx.scores.get(&b);
        match self {
            // Rule 1: never play the same opponent twice.
            Rule::Rematch => i128::from(sa.opponents.contains(&b)),
            // Rule 2: prefer equal scores; penalty is the square of the gap.
            Rule::ScoreGap => {
                let gap = (sa.points as i128 - sb.points as i128).abs();
                gap * gap
            }
            // Rule 3: the lower-scored player floats up, the higher-scored down.
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
            // Rule 4: on a cross-group (float) edge, prefer the right floaters —
            // the weakest of the upper group down, the first/median of the lower
            // group up. Same-group edges aren't floats, so no penalty.
            Rule::FloaterSelection => match sa.points.cmp(&sb.points) {
                Ordering::Equal => 0,
                _ => {
                    let (descender, ascender) = if sa.points > sb.points { (a, b) } else { (b, a) };
                    floater_units(ctx, descender, true) + floater_units(ctx, ascender, false)
                }
            },
            // Rule 5: avoid pairing club-mates — but only when protection is active
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
            // Rule 6: fold within a score group — deviation from the ideal fold.
            Rule::Fold => {
                if sa.points != sb.points {
                    return 0;
                }
                match (ctx.fold.get(&a), ctx.fold.get(&b)) {
                    (Some(fa), Some(fb)) => {
                        let ia = ideal_rank(fa.rank, fa.group_size) as i128;
                        let ib = ideal_rank(fb.rank, fb.group_size) as i128;
                        (fb.rank as i128 - ia).abs() + (fa.rank as i128 - ib).abs()
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
            Rule::FloatRepeat => float_units(s.last_descended, ctx.round),
            // A bye is a downfloat, so prefer the weakest of the group to take it.
            Rule::FloaterSelection => floater_units(ctx, player, true),
            // ELO mode: the weakest present player (lowest ELO rank) takes the bye.
            Rule::ByeSelection => ctx.elo_rank.get(&player).copied().unwrap_or(0),
            Rule::ScoreGap | Rule::Club | Rule::Fold | Rule::EloGap => 0,
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
        let per_edge = match self {
            Rule::Rematch => 1,
            Rule::ScoreGap => ctx.max_gap * ctx.max_gap,
            Rule::FloatRepeat => 2 * FLOAT_BASE, // two directions, each ≤ FLOAT_BASE
            // A descender and an ascender term, each a rank distance ≤ group_size − 1.
            Rule::FloaterSelection => 2 * (ctx.max_group - 1).max(0),
            Rule::Club => i128::from(ctx.club_active), // 0 when off — no wasted tier
            // Two |·| terms, each ≤ group_size − 1.
            Rule::Fold => 2 * (ctx.max_group - 1).max(0),
            // The squared gap between the widest-separated free players.
            Rule::EloGap => ctx.max_elo_gap * ctx.max_elo_gap,
            Rule::ByeSelection => unreachable!("handled above"),
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
        group.sort_by(|x, y| rating(y).cmp(&rating(x)).then_with(|| tnum(x).cmp(&tnum(y))));
        let m = group.len();
        for (rank, id) in group.iter().enumerate() {
            info.insert(*id, FoldInfo { rank, group_size: m });
        }
    }
    info
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
    let by_player: HashMap<Uuid, &Player> = players.iter().map(|p| (p.id, p)).collect();
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

    let fold = fold_ranks(&scores, &by_player, &free);

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
        // Per-round quantities the rule bounds (and hence multipliers) depend on:
        // the number of matching edges, the widest points gap, and the largest
        // score group. From these the multiplier ladder is derived so its tiers
        // are guaranteed disjoint (see `scale_ladder`).
        let (mut lo, mut hi) = (u32::MAX, 0u32);
        for &id in &free {
            let p = scores.get(&id).points;
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let exempt_clubs = settings.exempt_clubs_normalized();

        // ELO mode: a live estimate per free player (rounded), its ascending rank
        // (0 = weakest, for the bye-selection rule), and the widest gap (for the
        // ladder bound). All empty / zero in Swiss mode, where the ELO rules are
        // not active.
        let (elo, elo_rank, max_elo_gap) = if settings.elo_pairing_enabled {
            let est = estimate_elos(players, settings, completed_rounds);
            let elo: HashMap<Uuid, i64> = free
                .iter()
                .map(|&id| {
                    let e = est.get(&id).copied().unwrap_or(UNRATED_PRIOR_MEAN);
                    (id, e.round() as i64)
                })
                .collect();
            // Rank ascending by ELO, ties broken by tournament number so the bye
            // choice is deterministic.
            let tnum = |id: &Uuid| by_player[id].tournament_id.unwrap_or(u32::MAX);
            let mut order = free.clone();
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

        let ctx = Ctx {
            scores: &scores,
            by_player: &by_player,
            fold: &fold,
            round: number,
            floater_style: settings.floater_style,
            club_active: settings.club_protection_active(number),
            exempt_clubs: &exempt_clubs,
            edges: (vcount / 2) as i128,
            max_gap: hi.saturating_sub(lo) as i128,
            max_group: fold.values().map(|f| f.group_size).max().unwrap_or(0) as i128,
            free_count: k as i128,
            elo: &elo,
            elo_rank: &elo_rank,
            max_elo_gap,
        };
        let rules = active_rules(settings);
        let max_total: Vec<i128> = rules.iter().map(|r| r.max_total_units(&ctx)).collect();
        let mult = scale_ladder(&max_total);

        let mut cost = vec![vec![0i128; vcount]; vcount];
        for i in 0..k {
            for j in (i + 1)..k {
                let c = edge_cost(&ctx, rules, &mult, free[i], free[j]);
                cost[i][j] = c;
                cost[j][i] = c;
            }
        }
        if need_phantom {
            let p = k;
            for i in 0..k {
                let c = bye_cost(&ctx, rules, &mult, free[i]);
                cost[i][p] = c;
                cost[p][i] = c;
            }
        }
        let mate = min_weight_perfect_matching(&cost);
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

    fn ids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    #[test]
    fn even_count_pairs_all_players_no_bye() {
        let players = ids(4);
        let round = pair_round(1, &players);
        assert_eq!(round.number, 1);
        assert_eq!(round.boards.len(), 2);
        assert_eq!(round.bye, None);
        assert_eq!(round.boards[0].player1, players[0]);
        assert_eq!(round.boards[0].player2, players[1]);
    }

    #[test]
    fn odd_count_gives_last_player_a_bye() {
        let players = ids(5);
        let round = pair_round(3, &players);
        assert_eq!(round.boards.len(), 2);
        assert_eq!(round.bye, Some(players[4]));
    }

    #[test]
    fn forced_board_is_kept_and_rest_paired() {
        let p = ids(4);
        // Force p[0] vs p[3]; p[1] and p[2] should be paired automatically.
        let forced = vec![Board::pending(p[0], p[3], None, PairingSource::Swiss)];
        let round = pair_round_constrained(1, &p, &forced, None);
        assert_eq!(round.boards.len(), 2);
        assert_eq!(round.boards[0].player1, p[0]);
        assert_eq!(round.boards[0].player2, p[3]);
        assert_eq!(round.boards[1].player1, p[1]);
        assert_eq!(round.boards[1].player2, p[2]);
        assert_eq!(round.bye, None);
    }

    #[test]
    fn forced_bye_sits_the_chosen_player_out() {
        let p = ids(5);
        let round = pair_round_constrained(1, &p, &[], Some(p[1]));
        assert_eq!(round.bye, Some(p[1]));
        // The other four are paired, p[1] appears in no board.
        assert_eq!(round.boards.len(), 2);
        let in_boards: Vec<Uuid> = round
            .boards
            .iter()
            .flat_map(|b| [b.player1, b.player2])
            .collect();
        assert!(!in_boards.contains(&p[1]));
    }

    // --- Weighted pairing -------------------------------------------------

    fn player(tid: u32, rating: Option<u32>, club: Option<&str>) -> Player {
        Player {
            id: Uuid::new_v4(),
            tournament_id: Some(tid),
            last_name: format!("P{tid}"),
            first_name: String::new(),
            rating,
            nationality: None,
            club: club.map(|c| c.to_string()),
            eligible: false,
            adjustments: Vec::new(),
        }
    }

    fn completed_round(
        number: u32,
        boards: &[(Uuid, Uuid, Winner)],
        bye: Option<Uuid>,
    ) -> Round {
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
        let p: Vec<Player> = (1..=4).map(|i| player(i, Some(2000 - i * 10), None)).collect();
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

        let round =
            pair_round_weighted(2, &p, &TournamentSettings::default(), &[r1], &present, &[], None);

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

        let round =
            pair_round_weighted(2, &p, &TournamentSettings::default(), &[r1], &present, &[], None);

        // p2 already had a bye, so it must fall elsewhere; giving it to p1 also
        // leaves the same-score board p0 vs p2.
        assert_eq!(round.bye, Some(p[1].id));
        assert_eq!(board_pairs(&round), HashSet::from([unord(p[0].id, p[2].id)]));
    }

    #[test]
    fn pairing_freezes_the_points_diff_on_each_board() {
        // After round 1 (A, C on 1 point; B, D on 0), force A vs D in round 2.
        // The board should record the float A had going in: 1 − 0 = 1.
        let p: Vec<Player> = (1..=4).map(|i| player(i, Some(2000 - i * 10), None)).collect();
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

        let round =
            pair_round_weighted(2, &p, &TournamentSettings::default(), &[r1], &present, &forced, None);

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
            macmahon_thresholds: vec![1500],
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

        let round =
            pair_round_weighted(1, &p, &TournamentSettings::default(), &[], &present, &[], None);

        let pairs = board_pairs(&round);
        assert!(pairs.contains(&unord(p[0].id, p[2].id)), "fold pairs the X club-mates");
        assert!(pairs.contains(&unord(p[1].id, p[3].id)), "fold pairs the Y club-mates");
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
        assert!(pairs.contains(&unord(p[0].id, p[2].id)), "past the window, fold pairs X-X");
        assert!(pairs.contains(&unord(p[1].id, p[3].id)), "past the window, fold pairs Y-Y");
    }

    // --- ELO (non-Swiss) mode ---------------------------------------------

    fn elo_settings() -> TournamentSettings {
        TournamentSettings {
            elo_pairing_enabled: true,
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
        assert!(pairs.contains(&unord(p[0].id, p[1].id)), "closest pair 2000-1950");
        assert!(pairs.contains(&unord(p[2].id, p[3].id)), "closest pair 1500-1450");
    }

    #[test]
    fn elo_mode_gives_the_bye_to_the_weakest() {
        // Five players, no forced bye: the lowest-rated (and, round 1, lowest
        // estimate) should sit out, and the other four pair by adjacency.
        let p: Vec<Player> = (1..=5).map(|i| player(i, Some(2000 - (i - 1) * 300), None)).collect();
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
        assert!(pairs.contains(&unord(p[1].id, p[3].id)), "the two winners are paired");
        assert!(pairs.contains(&unord(p[0].id, p[2].id)), "the two losers are paired");
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
            macmahon_thresholds: vec![1500], // X0..X2 on 1 point, Y on 0
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
            macmahon_thresholds: vec![1500],
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
            macmahon_thresholds: vec![1500],
            ..Default::default()
        };
        let scores = compute_scores(&p, &settings, &[r1]);
        let by_player: HashMap<Uuid, &Player> = p.iter().map(|q| (q.id, q)).collect();
        let free: Vec<Uuid> = p.iter().map(|q| q.id).collect();
        let fold = fold_ranks(&scores, &by_player, &free);
        let (mut lo, mut hi) = (u32::MAX, 0u32);
        for &pid in &free {
            let pts = scores.get(&pid).points;
            lo = lo.min(pts);
            hi = hi.max(pts);
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
            club_active: true, // exercise the club rule's bound
            exempt_clubs: &exempt_clubs,
            edges,
            max_gap: (hi - lo) as i128,
            max_group: fold.values().map(|f| f.group_size).max().unwrap_or(0) as i128,
            free_count: free.len() as i128,
            elo: &empty_elo,
            elo_rank: &empty_rank,
            max_elo_gap: 0,
        };

        // Check the Swiss rules (the ones active in the default mode).
        for &rule in active_rules(&TournamentSettings::default()) {
            let bound = rule.max_total_units(&ctx);
            for i in 0..free.len() {
                for j in (i + 1)..free.len() {
                    assert!(
                        rule.edge_units(&ctx, free[i], free[j]) * edges <= bound,
                        "an edge exceeded the rule's total-units bound"
                    );
                }
                assert!(
                    rule.bye_units(&ctx, free[i]) * edges <= bound,
                    "a bye exceeded the rule's total-units bound"
                );
            }
        }
    }
}
