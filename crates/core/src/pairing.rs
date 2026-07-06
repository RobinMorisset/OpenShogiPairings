//! Round pairing.
//!
//! The intended model is a **minimum-weight perfect matching** over a complete
//! graph of the players, with edge weights encoding how undesirable a pairing is
//! — see the design notes in the repo.
//!
//! For now only the most naïve mode exists: **every edge has weight 1**, so any
//! perfect matching is optimal, and we just pair players consecutively. On top of
//! that, [`pair_round_constrained`] honors referee-forced pairings and a forced
//! bye. The real weighted matching (Blossom, then ILP/CP-SAT) will replace these
//! internals without changing the signatures — see TODO.md.

use std::collections::HashSet;

use uuid::Uuid;

use crate::round::{Board, Round};

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
        })
        .collect();
    for pair in remaining.chunks(2) {
        boards.push(Board {
            player1: pair[0],
            player2: pair[1],
            result: None,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
