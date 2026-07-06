//! Round pairing.
//!
//! A round is modeled as a **minimum-weight perfect matching** over a complete
//! graph of the present players (solved by [`crate::matching`]). Each edge's
//! weight is the sum of penalties from a fixed set of rules, ordered by a ladder
//! of priority multipliers ([`W_REMATCH`] ≫ [`W_SCORE`] ≫ [`W_FLOAT`] ≫
//! [`W_CLUB`] ≫ [`W_FOLD`]) so that a violation of a higher rule always outweighs
//! any amount of lower-rule imperfection. The rules, most important first:
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
//! [`pair_round_weighted`] is the real pairing path. [`pair_round`] /
//! [`pair_round_constrained`] remain as the trivial uniform-weight baseline
//! (used by the odd unit test); the bye is modeled as a phantom vertex.
//!
//! The multiplier gaps hold comfortably for realistic events (≲128 players, ≲20
//! rounds). Larger tournaments — and strict lexicographic tiers — are the
//! motivation for the planned ILP/CP-SAT backend (see TODO.md).

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

/// Priority multipliers for the pairing rules (see the module docs). Each is far
/// larger than the maximum total the lower-priority rules can contribute over a
/// whole round, so the combined scalar weight orders matchings lexicographically
/// by rule priority.
const W_REMATCH: i128 = 1_000_000_000_000_000_000_000_000; // 1e24 — rule 1
const W_SCORE: i128 = 1_000_000_000_000_000_000; // 1e18 — rule 2
const W_FLOAT: i128 = 1_000_000_000_000; // 1e12 — rule 3
const W_CLUB: i128 = 1_000_000; // 1e6 — rule 4
const W_FOLD: i128 = 1; // rule 5

/// Numerator of the float-repeat penalty, divided by the number of rounds since
/// the player last floated the same way. Chosen with many small divisors so the
/// decay reads smoothly.
const FLOAT_BASE: i128 = 720;

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

/// Penalty (0 if `last` is `None`) for floating the same direction again this
/// `round`, decaying with the number of rounds since `last`.
fn float_penalty(last: Option<u32>, round: u32) -> i128 {
    match last {
        Some(k) => {
            let since = (round - k) as i128; // k < round always
            W_FLOAT * (FLOAT_BASE / since)
        }
        None => 0,
    }
}

/// Penalty for pairing `a` against `b` this round.
fn edge_cost(
    scores: &Scores,
    by_player: &HashMap<Uuid, &Player>,
    fold: &HashMap<Uuid, FoldInfo>,
    a: Uuid,
    b: Uuid,
    round: u32,
) -> i128 {
    let sa = scores.get(&a);
    let sb = scores.get(&b);
    let mut cost = 0i128;

    // Rule 1: never play the same opponent twice.
    if sa.opponents.contains(&b) {
        cost += W_REMATCH;
    }

    // Rule 2: prefer equal scores; penalty grows with the square of the gap.
    let gap = (sa.points as i128 - sb.points as i128).abs();
    cost += W_SCORE * gap * gap;

    // Rule 3: avoid repeating a float in the same direction. The lower-scored
    // player floats up, the higher-scored one floats down.
    match sa.points.cmp(&sb.points) {
        Ordering::Less => {
            cost += float_penalty(sa.last_ascended, round);
            cost += float_penalty(sb.last_descended, round);
        }
        Ordering::Greater => {
            cost += float_penalty(sa.last_descended, round);
            cost += float_penalty(sb.last_ascended, round);
        }
        Ordering::Equal => {}
    }

    // Rule 4: avoid pairing club-mates (ignored when a club is unknown).
    if let (Some(ca), Some(cb)) = (&by_player[&a].club, &by_player[&b].club) {
        if ca == cb {
            cost += W_CLUB;
        }
    }

    // Rule 5: fold within a score group.
    if sa.points == sb.points {
        if let (Some(fa), Some(fb)) = (fold.get(&a), fold.get(&b)) {
            let ia = ideal_rank(fa.rank, fa.group_size) as i128;
            let ib = ideal_rank(fb.rank, fb.group_size) as i128;
            let dev = (fb.rank as i128 - ia).abs() + (fa.rank as i128 - ib).abs();
            cost += W_FOLD * dev;
        }
    }

    cost
}

/// Penalty for giving `player` the bye this round: a bye repeats rule 1 (never
/// bye twice) and counts as a downfloat for rule 3.
fn bye_cost(scores: &Scores, player: Uuid, round: u32) -> i128 {
    let s = scores.get(&player);
    let mut cost = 0i128;
    if s.had_bye {
        cost += W_REMATCH;
    }
    cost += float_penalty(s.last_descended, round);
    cost
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
        let mut cost = vec![vec![0i128; vcount]; vcount];
        for i in 0..k {
            for j in (i + 1)..k {
                let c = edge_cost(&scores, &by_player, &fold, free[i], free[j], number);
                cost[i][j] = c;
                cost[j][i] = c;
            }
        }
        if need_phantom {
            let p = k;
            for i in 0..k {
                let c = bye_cost(&scores, free[i], number);
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
}
