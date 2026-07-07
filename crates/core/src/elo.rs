//! Bayesian ELO estimation for the experimental non-Swiss pairing mode.
//!
//! Each player has a latent strength `θ` on the ELO scale. Game outcomes follow
//! the logistic (Bradley–Terry) model `P(i beats j) = σ((θᵢ − θⱼ)/s)` with
//! `s = 400/ln 10`, and each player has a Gaussian prior anchoring `θ` to their
//! registration rating (or, for an unrated player, to a wide `N(600, 350²)`). The
//! estimated ELO is the **MAP** (maximum a-posteriori) point — the maximiser of
//! the penalised log-likelihood — found by coordinate-ascent Newton. The whole
//! thing is a **pure replay** of the completed rounds, recomputed from scratch
//! each time (like [`crate::scoring::compute_scores`]), so it is order-independent
//! and survives result edits / undo.
//!
//! The design, including why the single K multiplier is the only knob and how the
//! per-game drift auto-decelerates, is written up in `docs/elo-pairing-mode.md`.

use std::collections::HashMap;

use uuid::Uuid;

use crate::player::Player;
use crate::round::{Round, Winner};
use crate::settings::TournamentSettings;

/// ELO scale factor `s = 400 / ln 10`: a 400-point gap is 10:1 odds.
const S: f64 = 173.717_792_761_565_8;

/// Prior mean for an unrated player — the midpoint of the assumed `[1, 1200]`
/// strength range.
pub const UNRATED_PRIOR_MEAN: f64 = 600.0;

/// Prior standard deviation for an unrated player — the std of a uniform on
/// `[1, 1200]` is `1199/√12 ≈ 346`, rounded to 350.
const UNRATED_PRIOR_STD: f64 = 350.0;

/// Solver limits: coordinate-ascent sweeps and the per-sweep convergence
/// tolerance (in ELO points). The objective is strongly concave, so this
/// converges quickly and deterministically.
const MAX_SWEEPS: usize = 200;
const CONVERGENCE_TOL: f64 = 1e-4;

/// A generous per-update step clamp (ELO points) guarding against a Newton
/// overshoot when a wide-prior player is far from the optimum; repeated sweeps
/// then close the remaining distance.
const MAX_STEP: f64 = 800.0;

/// FIDE's rating-dependent K factor, used only to seed each rated player's prior
/// width (see [`prior`]).
fn fide_k(rating: u32) -> f64 {
    match rating {
        r if r >= 2000 => 20.0,
        r if r >= 1600 => 24.0,
        r if r >= 1200 => 28.0,
        r if r >= 800 => 32.0,
        r if r >= 400 => 36.0,
        _ => 40.0,
    }
}

/// A player's Gaussian prior `(mean, standard deviation)` on the ELO scale.
///
/// A rated player is centered on their registration rating with a width derived
/// from `m · K_FIDE(rating)` via `σ₀ = √(K · s)` (so `K` is literally their
/// first-game K factor). An unrated player gets the wide `N(600, 350²)` prior.
fn prior(player: &Player, k_multiplier: f64) -> (f64, f64) {
    match player.rating {
        Some(rating) => {
            // `k_multiplier` is clamped ≥ 1% by settings normalization, so K > 0.
            let k = k_multiplier * fide_k(rating);
            (f64::from(rating), (k * S).sqrt())
        }
        None => (UNRATED_PRIOR_MEAN, UNRATED_PRIOR_STD),
    }
}

/// Expected score `σ(x) = 1/(1+e^−x)` for `x = (θself − θopp)/s`.
fn expected(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Estimate every player's current ELO (posterior mode) from the completed
/// rounds, using the FIDE-K × multiplier prior from `settings`.
///
/// Byes contribute nothing (they are not games), draws score ½ for each side, and
/// handicap games are excluded for now (V1 — a handicap→ELO mapping is future
/// work). The returned map has one entry per player in `players`; a player with no
/// counted games sits exactly at their prior mean.
pub fn estimate_elos(
    players: &[Player],
    settings: &TournamentSettings,
    rounds: &[Round],
) -> HashMap<Uuid, f64> {
    let m = settings.elo_k_multiplier();
    let n = players.len();

    let index: HashMap<Uuid, usize> = players.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    let mut theta = vec![0.0_f64; n];
    let mut mean = vec![0.0_f64; n];
    let mut precision = vec![0.0_f64; n]; // 1/σ₀² — the prior's weight
    for (i, p) in players.iter().enumerate() {
        let (mu0, sigma0) = prior(p, m);
        mean[i] = mu0;
        theta[i] = mu0; // seed at the prior mean
        precision[i] = 1.0 / (sigma0 * sigma0);
    }

    // Per-player incident games: (opponent index, this player's score in {0, ½, 1}).
    let mut games: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for round in rounds.iter().filter(|r| r.completed) {
        for board in &round.boards {
            // V1: handicap games don't inform the even-game strength estimate.
            if board.handicap.is_some() {
                continue;
            }
            let (Some(&a), Some(&b)) = (index.get(&board.player1), index.get(&board.player2)) else {
                continue; // an opponent no longer in the tournament
            };
            let score_a = if board.drawn {
                0.5
            } else {
                match board.result {
                    Some(Winner::Player1) => 1.0,
                    Some(Winner::Player2) => 0.0,
                    None => continue, // unplayed
                }
            };
            games[a].push((b, score_a));
            games[b].push((a, 1.0 - score_a));
        }
    }

    // Coordinate ascent: sweep players, each a 1-D Newton step on the concave
    // penalised log-likelihood. Strong concavity (every player has a finite prior
    // precision) guarantees a unique maximum and convergence.
    for _ in 0..MAX_SWEEPS {
        let mut max_delta = 0.0_f64;
        for i in 0..n {
            // Prior term first, then each game's likelihood contribution.
            let mut gradient = -(theta[i] - mean[i]) * precision[i];
            let mut hessian = -precision[i];
            for &(j, score) in &games[i] {
                let e = expected((theta[i] - theta[j]) / S);
                gradient += (score - e) / S;
                hessian -= e * (1.0 - e) / (S * S);
            }
            // hessian ≤ −precision[i] < 0, so this is a well-defined ascent step.
            let step = (-gradient / hessian).clamp(-MAX_STEP, MAX_STEP);
            theta[i] += step;
            max_delta = max_delta.max(step.abs());
        }
        if max_delta < CONVERGENCE_TOL {
            break;
        }
    }

    index.into_iter().map(|(id, i)| (id, theta[i])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::{Board, PairingSource};

    fn player(rating: Option<u32>) -> Player {
        Player {
            id: Uuid::new_v4(),
            tournament_id: None,
            last_name: "P".into(),
            first_name: String::new(),
            rating,
            nationality: None,
            club: None,
            eligible: false,
            adjustments: Vec::new(),
        }
    }

    fn decided(number: u32, boards: Vec<Board>) -> Round {
        Round {
            number,
            boards,
            bye: None,
            absent: Vec::new(),
            completed: true,
        }
    }

    fn win(a: Uuid, b: Uuid) -> Board {
        Board {
            result: Some(Winner::Player1),
            ..Board::pending(a, b, None, PairingSource::Swiss)
        }
    }

    #[test]
    fn no_games_leaves_everyone_at_their_prior_mean() {
        let rated = player(Some(1800));
        let unrated = player(None);
        let elos = estimate_elos(
            &[rated.clone(), unrated.clone()],
            &TournamentSettings::default(),
            &[],
        );
        assert!((elos[&rated.id] - 1800.0).abs() < 1e-6);
        assert!((elos[&unrated.id] - UNRATED_PRIOR_MEAN).abs() < 1e-6);
    }

    #[test]
    fn a_win_raises_the_winner_and_lowers_the_loser() {
        let a = player(Some(1500));
        let b = player(Some(1500));
        let r = decided(1, vec![win(a.id, b.id)]);
        let elos = estimate_elos(&[a.clone(), b.clone()], &TournamentSettings::default(), &[r]);
        assert!(elos[&a.id] > 1500.0, "winner rises");
        assert!(elos[&b.id] < 1500.0, "loser falls");
        // Symmetric priors and opposite results → symmetric shifts.
        assert!(((elos[&a.id] - 1500.0) + (elos[&b.id] - 1500.0)).abs() < 1e-3);
    }

    #[test]
    fn single_game_shift_is_capped_near_k() {
        // A rated 1100 player (FIDE K = 32, m = 1) beating a 1600: the first-game
        // move is capped by K = 32 and, since the opponent is much stronger
        // (E ≈ 0.05), lands just under it — matching a plain Elo update.
        let winner = player(Some(1100));
        let loser = player(Some(1600));
        let r = decided(1, vec![win(winner.id, loser.id)]);
        let elos = estimate_elos(
            &[winner.clone(), loser.clone()],
            &TournamentSettings::default(),
            &[r],
        );
        let shift = elos[&winner.id] - 1100.0;
        assert!(shift > 25.0 && shift < 32.0, "shift was {shift}, expected ~30 (< K=32)");
    }

    #[test]
    fn a_bigger_multiplier_moves_the_estimate_more() {
        let a = player(Some(1500));
        let b = player(Some(1500));
        let r = decided(1, vec![win(a.id, b.id)]);
        let base = TournamentSettings::default();
        let hot = TournamentSettings {
            elo_k_multiplier_percent: 400,
            ..Default::default()
        };
        let shift_base = estimate_elos(&[a.clone(), b.clone()], &base, std::slice::from_ref(&r))[&a.id] - 1500.0;
        let shift_hot = estimate_elos(&[a.clone(), b.clone()], &hot, std::slice::from_ref(&r))[&a.id] - 1500.0;
        assert!(shift_hot > shift_base * 2.0, "a 4× multiplier should move the estimate much more");
    }

    #[test]
    fn unrated_estimate_swings_far_on_a_big_upset() {
        // An unrated player (wide prior, seeded 600) beating a 1600 should jump
        // hundreds of points — the uncertainty-proportional cap in action.
        let newcomer = player(None);
        let strong = player(Some(1600));
        let r = decided(1, vec![win(newcomer.id, strong.id)]);
        let elos = estimate_elos(
            &[newcomer.clone(), strong.clone()],
            &TournamentSettings::default(),
            &[r],
        );
        assert!(elos[&newcomer.id] > 1100.0, "unrated upset should swing far, got {}", elos[&newcomer.id]);
    }

    #[test]
    fn draws_are_scored_as_half_and_leave_equals_level() {
        let a = player(Some(1500));
        let b = player(Some(1500));
        let drawn = Board {
            drawn: true,
            ..Board::pending(a.id, b.id, None, PairingSource::Swiss)
        };
        let elos = estimate_elos(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &[decided(1, vec![drawn])],
        );
        // Equal-rated players who draw stay put.
        assert!((elos[&a.id] - 1500.0).abs() < 1e-3);
        assert!((elos[&b.id] - 1500.0).abs() < 1e-3);
    }

    #[test]
    fn handicap_games_are_excluded_v1() {
        use crate::round::{Handicap, HandicapGame};
        let a = player(Some(1500));
        let b = player(Some(1500));
        let handicap_board = Board {
            result: Some(Winner::Player1),
            handicap: Some(HandicapGame {
                handicap: Handicap::Rook,
                giver: Winner::Player1,
            }),
            ..Board::pending(a.id, b.id, None, PairingSource::Swiss)
        };
        let elos = estimate_elos(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &[decided(1, vec![handicap_board])],
        );
        // Excluded from the likelihood → both stay at their prior mean.
        assert!((elos[&a.id] - 1500.0).abs() < 1e-6);
        assert!((elos[&b.id] - 1500.0).abs() < 1e-6);
    }

    #[test]
    fn repeated_wins_keep_climbing_but_decelerate() {
        // The same 1500 player beats three different 1500s. The estimate keeps
        // rising, but each win moves it less than the previous one (the cap
        // shrinks as the rating firms up).
        let hero = player(Some(1500));
        let opps: Vec<Player> = (0..3).map(|_| player(Some(1500))).collect();
        let mut all = vec![hero.clone()];
        all.extend(opps.iter().cloned());

        let mut prev = 1500.0;
        let mut prev_gain = f64::INFINITY;
        for k in 0..3 {
            let rounds: Vec<Round> = (0..=k)
                .map(|i| decided(i as u32 + 1, vec![win(hero.id, opps[i].id)]))
                .collect();
            let elos = estimate_elos(&all, &TournamentSettings::default(), &rounds);
            let now = elos[&hero.id];
            let gain = now - prev;
            assert!(gain > 0.0, "estimate should keep rising");
            assert!(gain < prev_gain, "each win should move it less than the last");
            prev = now;
            prev_gain = gain;
        }
    }
}
