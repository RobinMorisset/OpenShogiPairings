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
//! 4. **Different clubs** — avoid pairing club-mates (ignored when a club is
//!    unknown).
//! 5. **Fold within a score group** — sort a group (equal points) by rating
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

use crate::matching::min_weight_perfect_matching;
use crate::player::Player;
use crate::round::{Board, Round};
use crate::scoring::{compute_scores, Scores};
use crate::settings::TournamentSettings;

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
        .map(|b| Board {
            player1: b.player1,
            player2: b.player2,
            result: None,
            drawn: false,
            handicap: None,
            points_diff: None,
        })
        .collect();
    for pair in remaining.chunks(2) {
        boards.push(Board {
            player1: pair[0],
            player2: pair[1],
            result: None,
            drawn: false,
            handicap: None,
            points_diff: None,
        });
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

/// The pairing rules, in priority order (highest first). This ordering is the
/// single source of truth for priority; the scalar multipliers are derived from
/// it (see [`scale_ladder`]).
#[derive(Clone, Copy)]
enum Rule {
    /// Never play the same opponent twice / never take the bye twice.
    Rematch,
    /// Prefer equal scores; penalty grows with the square of the points gap.
    ScoreGap,
    /// Avoid repeating a float in the same direction; decays with rounds since.
    FloatRepeat,
    /// Avoid pairing club-mates (ignored when a club is unknown).
    Club,
    /// Fold within a score group (top half meets bottom half).
    Fold,
}

impl Rule {
    /// The rules from highest to lowest priority.
    const ORDER: [Rule; 5] = [
        Rule::Rematch,
        Rule::ScoreGap,
        Rule::FloatRepeat,
        Rule::Club,
        Rule::Fold,
    ];
    /// How many rules there are (the multiplier ladder's length).
    const COUNT: usize = Self::ORDER.len();
}

/// Everything the rules need to score an edge, plus the per-round quantities their
/// worst-case bounds (and hence multipliers) are derived from.
struct Ctx<'a> {
    scores: &'a Scores,
    by_player: &'a HashMap<Uuid, &'a Player>,
    fold: &'a HashMap<Uuid, FoldInfo>,
    round: u32,
    /// Edges in a perfect matching over the vertices (= vertices / 2).
    edges: i128,
    /// Largest points gap between any two vertices (bounds the score rule).
    max_gap: i128,
    /// Largest score-group size among the free players (bounds the fold rule).
    max_group: i128,
}

/// Float-repeat units for one player/direction: 0 if they never floated that way,
/// else `FLOAT_BASE` decayed by the rounds since (at least 1, so `≤ FLOAT_BASE`).
fn float_units(last: Option<u32>, round: u32) -> i128 {
    match last {
        Some(k) => FLOAT_BASE / (round - k) as i128, // k < round always
        None => 0,
    }
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
            // Rule 4: avoid pairing club-mates (ignored when a club is unknown).
            Rule::Club => match (&ctx.by_player[&a].club, &ctx.by_player[&b].club) {
                (Some(ca), Some(cb)) if ca == cb => 1,
                _ => 0,
            },
            // Rule 5: fold within a score group — deviation from the ideal fold.
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
        }
    }

    /// Penalty units for giving `player` the bye (before the priority multiplier).
    /// A bye repeats the rematch rule (never bye twice) and counts as a downfloat.
    fn bye_units(self, ctx: &Ctx, player: Uuid) -> i128 {
        let s = ctx.scores.get(&player);
        match self {
            Rule::Rematch => i128::from(s.had_bye),
            Rule::FloatRepeat => float_units(s.last_descended, ctx.round),
            Rule::ScoreGap | Rule::Club | Rule::Fold => 0,
        }
    }

    /// A safe upper bound on the total units this rule can emit across one round's
    /// matching: (largest units on any single edge or bye) × (number of edges).
    fn max_total_units(self, ctx: &Ctx) -> i128 {
        let per_edge = match self {
            Rule::Rematch => 1,
            Rule::ScoreGap => ctx.max_gap * ctx.max_gap,
            Rule::FloatRepeat => 2 * FLOAT_BASE, // two directions, each ≤ FLOAT_BASE
            Rule::Club => 1,
            // Two |·| terms, each ≤ group_size − 1.
            Rule::Fold => 2 * (ctx.max_group - 1).max(0),
        };
        per_edge * ctx.edges
    }
}

/// Derive the priority multipliers from each rule's worst-case total units, given
/// in priority order (highest first). Bottom-up, `mult[i] = 1 + Σ_{j>i}
/// mult[j]·max_total[j]`, so one unit of rule `i` strictly exceeds the largest
/// possible sum of all lower-priority rules combined — a correct lexicographic
/// scalarization with no hand-tuned gaps.
fn scale_ladder(max_total: [i128; Rule::COUNT]) -> [i128; Rule::COUNT] {
    let mut mult = [0i128; Rule::COUNT];
    let mut lower = 0i128; // Σ over the already-assigned lower-priority rules
    for i in (0..Rule::COUNT).rev() {
        mult[i] = 1 + lower;
        lower += mult[i] * max_total[i];
    }
    mult
}

/// Total edge weight for pairing `a` against `b`: `Σ mult[rule] · units`.
fn edge_cost(ctx: &Ctx, mult: &[i128; Rule::COUNT], a: Uuid, b: Uuid) -> i128 {
    Rule::ORDER
        .iter()
        .zip(mult)
        .map(|(rule, m)| m * rule.edge_units(ctx, a, b))
        .sum()
}

/// Total edge weight for giving `player` the bye.
fn bye_cost(ctx: &Ctx, mult: &[i128; Rule::COUNT], player: Uuid) -> i128 {
    Rule::ORDER
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
        .map(|b| Board {
            player1: b.player1,
            player2: b.player2,
            result: None,
            drawn: false,
            handicap: None,
            points_diff: Some(diff(b.player1, b.player2)),
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
        let ctx = Ctx {
            scores: &scores,
            by_player: &by_player,
            fold: &fold,
            round: number,
            edges: (vcount / 2) as i128,
            max_gap: hi.saturating_sub(lo) as i128,
            max_group: fold.values().map(|f| f.group_size).max().unwrap_or(0) as i128,
        };
        let mult = scale_ladder(Rule::ORDER.map(|r| r.max_total_units(&ctx)));

        let mut cost = vec![vec![0i128; vcount]; vcount];
        for i in 0..k {
            for j in (i + 1)..k {
                let c = edge_cost(&ctx, &mult, free[i], free[j]);
                cost[i][j] = c;
                cost[j][i] = c;
            }
        }
        if need_phantom {
            let p = k;
            for i in 0..k {
                let c = bye_cost(&ctx, &mult, free[i]);
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
                boards.push(Board {
                    player1: free[i],
                    player2: free[j],
                    result: None,
                    drawn: false,
                    handicap: None,
                    points_diff: Some(diff(free[i], free[j])),
                });
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
        let forced = vec![Board {
            player1: p[0],
            player2: p[3],
            result: None,
            drawn: false,
            handicap: None,
            points_diff: None,
        }];
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
                    player1: a,
                    player2: b,
                    result: Some(w),
                    drawn: false,
                    handicap: None,
                    points_diff: None,
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
        let forced = vec![Board {
            player1: p[0].id, // A (1 pt)
            player2: p[3].id, // D (0 pt)
            result: None,
            drawn: false,
            handicap: None,
            points_diff: None,
        }];

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

    #[test]
    fn weighted_avoids_pairing_club_mates() {
        // Everyone unbeaten (round 1), one score group of four. Rating order
        // p0>p1>p2>p3 makes the fold ideal p0-p2 and p1-p3 — but those are
        // club-mates, so the club rule must override the fold.
        let p = vec![
            player(1, Some(2000), Some("X")),
            player(2, Some(1900), Some("Y")),
            player(3, Some(1800), Some("X")),
            player(4, Some(1700), Some("Y")),
        ];
        let present: Vec<Uuid> = p.iter().map(|x| x.id).collect();

        let round =
            pair_round_weighted(1, &p, &TournamentSettings::default(), &[], &present, &[], None);

        assert_eq!(round.boards.len(), 2);
        let club_of = |id: Uuid| p.iter().find(|q| q.id == id).unwrap().club.clone();
        for b in &round.boards {
            assert_ne!(
                club_of(b.player1),
                club_of(b.player2),
                "club-mates were paired"
            );
        }
    }

    #[test]
    fn scale_ladder_tiers_are_disjoint() {
        // Arbitrary per-rule worst-case totals, in priority order (highest first).
        let max_total = [7i128, 40, 13, 5, 9];
        let mult = scale_ladder(max_total);
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
        assert_eq!(mult[Rule::COUNT - 1], 1); // the lowest-priority rule is the unit
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
        let ctx = Ctx {
            scores: &scores,
            by_player: &by_player,
            fold: &fold,
            round: 2,
            edges,
            max_gap: (hi - lo) as i128,
            max_group: fold.values().map(|f| f.group_size).max().unwrap_or(0) as i128,
        };

        for rule in Rule::ORDER {
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
