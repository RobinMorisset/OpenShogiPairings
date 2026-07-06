//! Round pairing.
//!
//! This is where the pairing engine lives. The intended model is a
//! **minimum-weight perfect matching** over a complete graph of the players,
//! where edge weights encode how undesirable a pairing is (score difference,
//! rematches, colour balance, …) — see the design notes in the repo.
//!
//! For now only the most naïve mode exists: **every edge has weight 1**, so any
//! perfect matching is optimal. We therefore just pair players in their current
//! order and give the odd one out a bye. The real weighted matching (Blossom
//! algorithm, then an ILP/CP-SAT backend) will replace the body of
//! [`pair_round`] without changing its signature — see TODO.md.

use uuid::Uuid;

use crate::round::{Board, Round};

/// Pair the given players for round `number`.
///
/// Naïve uniform-weight matching: consecutive players are paired, and if the
/// count is odd the last player receives a bye.
pub fn pair_round(number: u32, player_ids: &[Uuid]) -> Round {
    let mut remaining = player_ids.to_vec();
    let bye = if remaining.len() % 2 == 1 {
        remaining.pop()
    } else {
        None
    };
    let boards = remaining
        .chunks(2)
        .map(|pair| Board {
            player1: pair[0],
            player2: pair[1],
            result: None,
        })
        .collect();
    Round {
        number,
        boards,
        bye,
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
        // Consecutive pairing.
        assert_eq!(round.boards[0].player1, players[0]);
        assert_eq!(round.boards[0].player2, players[1]);
        assert_eq!(round.boards[1].player1, players[2]);
        assert_eq!(round.boards[1].player2, players[3]);
    }

    #[test]
    fn odd_count_gives_last_player_a_bye() {
        let players = ids(5);
        let round = pair_round(3, &players);
        assert_eq!(round.boards.len(), 2);
        assert_eq!(round.bye, Some(players[4]));
    }

    #[test]
    fn every_player_appears_exactly_once() {
        let players = ids(7);
        let round = pair_round(1, &players);
        let mut seen: Vec<Uuid> = round
            .boards
            .iter()
            .flat_map(|b| [b.player1, b.player2])
            .chain(round.bye)
            .collect();
        seen.sort();
        let mut expected = players.clone();
        expected.sort();
        assert_eq!(seen, expected);
    }
}
