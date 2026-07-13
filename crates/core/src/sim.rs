//! Monte-Carlo tournament simulation.
//!
//! The pieces a statistical study of pairing settings needs, as pure functions
//! reusing the real engine (`prepare_round` → `confirm_round` → results, which
//! complete the round automatically) so a simulated tournament is paired exactly
//! as a live one would be. The CLI ([`crates/sim`](../../sim)) links these directly and runs
//! the loop thousands of times in parallel; the design is written up in
//! `docs/simulation-cli.md`.
//!
//! Three layers:
//! - a **result model** — sample a board's winner from the logistic law
//!   `P(A beats B) = 1/(1+10^((elo_B−elo_A)/400))`;
//! - a **strength model** — each player's ground-truth strength for a run is an
//!   explicit override (treated as known) or a draw from their own `elo.rs`
//!   prior, so injected noise is rating-dependent;
//! - a **run driver** — reset a base tournament, play `rounds` auto-filled rounds,
//!   and return the finishing order plus the per-game strength gaps.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use rand::Rng;
use uuid::Uuid;

use crate::elo::{estimate_elos, player_prior};
use crate::player::Player;
use crate::round::Winner;
use crate::settings::TournamentSettings;
use crate::tournament::{Tournament, TournamentError, MIN_PLAYERS_PER_ROUND};

/// Ground-truth strengths (ELO scale) keyed by player id — the "true" playing
/// strength outcomes are drawn from, distinct from a registration rating.
pub type StrengthMap = HashMap<Uuid, f64>;

/// Errors from a simulation run.
#[derive(Debug, thiserror::Error)]
pub enum SimError {
    /// A tournament operation (pairing, round lifecycle) failed.
    #[error(transparent)]
    Tournament(#[from] TournamentError),
    /// The base tournament has too few players to pair a round.
    #[error("need at least {MIN_PLAYERS_PER_ROUND} players to simulate (have {have})")]
    NotEnoughPlayers { have: usize },
}

/// Cup-format setup for a simulation: the players eligible for the bracket (the
/// caller has already applied whatever eligibility rule it wants — nationality,
/// attendance, …) and the bracket size (a power of two, 8..=64). Passing this to
/// [`simulate_run`] runs the hybrid direct-elimination cup alongside the Swiss.
#[derive(Debug, Clone)]
pub struct CupConfig {
    pub eligible: HashSet<Uuid>,
    pub size: u32,
}

/// The usual cup eligibility for a **historical** base: players whose nationality
/// is in `nations` and who were **not absent** in any of the first
/// `attendance_rounds` real rounds. `nations` must already be upper-cased (player
/// nationalities are stored upper-cased). This is a convenience for the CLI;
/// `simulate_run` itself just takes the resulting id set in a [`CupConfig`].
pub fn cup_eligibility(
    base: &Tournament,
    nations: &HashSet<String>,
    attendance_rounds: usize,
) -> HashSet<Uuid> {
    let absent_early: HashSet<Uuid> = base
        .rounds
        .iter()
        .take(attendance_rounds)
        .flat_map(|r| r.absent.iter().copied())
        .collect();
    base.players
        .iter()
        .filter(|p| {
            p.nationality
                .as_deref()
                .is_some_and(|n| nations.contains(n))
        })
        .filter(|p| !absent_early.contains(&p.id))
        .map(|p| p.id)
        .collect()
}

/// Logistic win probability `P(self beats opp)` on the ELO scale: a 400-point
/// edge is ≈ 91%, equal strength is exactly ½.
pub fn win_probability(elo_self: f64, elo_opp: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf((elo_opp - elo_self) / 400.0))
}

/// One draw from `N(mean, std²)` via the Box–Muller transform. `std == 0` returns
/// the mean exactly (no random draw consumed only when short-circuited by the
/// caller — here it still produces the mean).
fn sample_normal(rng: &mut impl Rng, mean: f64, std: f64) -> f64 {
    // Guard the log against an exact zero from the open-below-1 uniform.
    let u1: f64 = rng.gen::<f64>().max(f64::MIN_POSITIVE);
    let u2: f64 = rng.gen::<f64>();
    let z = (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos();
    mean + std * z
}

/// Sample each player's ground-truth strength for one run.
///
/// The strength is drawn from `N(center, (jitter·σ₀)²)`, where the **center** is
/// the player's `overrides` entry if present — the post-tournament strength from a
/// `--results` table, or an explicit `--strength` value — otherwise the player's
/// registration rating (the `elo.rs` prior mean). The **width** `σ₀` is always the
/// player's rating-dependent prior width (tighter for strong/established players,
/// wider for provisional/unrated ones), so `jitter = 1` samples at the estimator's
/// own uncertainty and `> 1` stress-tests worse-than-assumed ratings.
///
/// So `jitter = 0` pins each player to their center exactly (the override, else the
/// rating); any `jitter > 0` spreads them *around that center* — crucially around
/// the post-tournament ELO when a results table supplied it, not the pre-tournament
/// rating. Players are visited in slice order so the draws — and thus the whole run
/// — are reproducible from the seed, and identical across settings variants.
pub fn sample_strengths(
    players: &[Player],
    settings: &TournamentSettings,
    overrides: &StrengthMap,
    jitter: f64,
    rng: &mut impl Rng,
) -> StrengthMap {
    players
        .iter()
        .map(|p| {
            let (prior_mean, std) = player_prior(p, settings);
            // The override (post-tournament / known) strength is the mean to jitter
            // around; fall back to the registration rating when there is none.
            let center = overrides.get(&p.id).copied().unwrap_or(prior_mean);
            let strength = if jitter <= 0.0 {
                center
            } else {
                sample_normal(rng, center, jitter * std)
            };
            (p.id, strength)
        })
        .collect()
}

/// The absolute strength gap of every **played** game (byes are not boards, so
/// they're naturally excluded), measured against `strengths`.
///
/// Generic in the strength map: the simulator passes the run's ground truth; a
/// caller inspecting a real tournament can pass registration ratings or an
/// updated estimate. A board whose players are missing from the map is skipped.
pub fn game_elo_diffs(tournament: &Tournament, strengths: &StrengthMap) -> Vec<f64> {
    let mut diffs = Vec::new();
    for round in &tournament.rounds {
        for board in &round.boards {
            if board.result.is_none() {
                continue; // unplayed — not a game yet
            }
            if let (Some(&a), Some(&b)) =
                (strengths.get(&board.player1), strengths.get(&board.player2))
            {
                diffs.push((a - b).abs());
            }
        }
    }
    diffs
}

/// The outcome of one simulated tournament — everything the report aggregates.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// Final standings order (rank 1 first) by the configured score/tie-breaks.
    pub final_order: Vec<Uuid>,
    /// Final order (rank 1 first) by the Bayesian ELO estimate instead — the
    /// second lens on ranking fidelity (see the design's §4.3).
    pub estimated_order: Vec<Uuid>,
    /// Absolute ground-truth strength gap of every played game.
    pub game_diffs: Vec<f64>,
    /// The ground-truth strengths used this run, so aggregation can score the
    /// finishing order against the truth it was generated from.
    pub strengths: StrengthMap,
    /// The direct-elimination cup champion this run, when a cup was configured and
    /// its final was decided. `None` for a pure-Swiss run, or if a double no-show
    /// left the final undetermined.
    pub cup_champion: Option<Uuid>,
}

impl RunOutcome {
    /// The tournament winner (rank-1 finisher), or `None` if there were no
    /// players. Uses the existing tie-break chain — no special ties handling.
    pub fn winner(&self) -> Option<Uuid> {
        self.final_order.first().copied()
    }
}

/// A clone of `base` reset to *registration-finalized, zero rounds* under
/// `settings`, ready to pair round 1. Tournament numbers and any prior rounds are
/// cleared and re-derived, so a different settings variant finalizes cleanly.
///
/// The cup is controlled entirely by `cup`, not by the config's `cup_enabled`
/// flag (the config can't carry a bracket size or an eligibility set): `Some`
/// enables the hybrid cup with that eligibility/size, `None` runs pure Swiss.
fn fresh_state(
    base: &Tournament,
    settings: &TournamentSettings,
    cup: Option<&CupConfig>,
) -> Result<Tournament, SimError> {
    if base.players.len() < MIN_PLAYERS_PER_ROUND {
        return Err(SimError::NotEnoughPlayers {
            have: base.players.len(),
        });
    }

    let mut settings = settings.clone().normalized();
    let mut t = base.clone();
    t.rounds.clear();
    t.draft = None;
    t.cup = None;
    t.registration_finalized = false;
    for p in &mut t.players {
        p.tournament_id = None;
        p.eligible = false;
    }

    match cup {
        Some(cup) => {
            settings.cup_enabled = true;
            for p in &mut t.players {
                p.eligible = cup.eligible.contains(&p.id);
            }
            t.settings = settings;
            t.finalize_registration_with(Some(cup.size))?;
        }
        None => {
            settings.cup_enabled = false;
            t.settings = settings;
            t.finalize_registration()?;
        }
    }
    Ok(t)
}

/// The SplitMix64 finalizer — a fast, well-mixing bijection on `u64`, used to
/// fold a game's identity into a pseudo-random draw.
fn splitmix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A deterministic uniform in `[0, 1)` for one game, keyed on the run and the
/// game's *identity* rather than on when it is drawn.
///
/// The key is `(run_seed, low id, high id, rematch)` with the two player ids
/// sorted, so the same pairing yields the same draw no matter which variant
/// produced it or in what board order — that is what makes common random numbers
/// actually couple variants (see the module's game-keyed-RNG note). `rematch`
/// (how many times these two have already met this run) keeps a rare second
/// meeting from reusing the first's outcome.
fn game_uniform(run_seed: u64, lo: Uuid, hi: Uuid, rematch: u32) -> f64 {
    let lo = lo.as_u128();
    let hi = hi.as_u128();
    let mut h = run_seed;
    for part in [
        lo as u64,
        (lo >> 64) as u64,
        hi as u64,
        (hi >> 64) as u64,
        rematch as u64,
    ] {
        h = splitmix64(h ^ part);
    }
    // Top 53 bits → a uniform double in [0, 1), like rand's StandardUniform.
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// How many times `a` and `b` have already met in `prior` rounds (unordered).
fn prior_meetings(prior: &[crate::round::Round], a: Uuid, b: Uuid) -> u32 {
    prior
        .iter()
        .flat_map(|r| &r.boards)
        .filter(|board| {
            (board.player1 == a && board.player2 == b)
                || (board.player1 == b && board.player2 == a)
        })
        .count() as u32
}

/// Decide one board's winner from the game-keyed uniform, expressed relative to
/// the given `(p1, p2)` seating. The draw and the win probability are both taken
/// in the canonical low→high orientation, so swapping `p1`/`p2` (and their
/// strengths) names the *same* physical winner — the invariance that lets common
/// random numbers survive a pairing reshuffle.
fn decide_board(run_seed: u64, p1: Uuid, p2: Uuid, s1: f64, s2: f64, rematch: u32) -> Winner {
    let (lo, hi, s_lo, s_hi) = if p1 <= p2 {
        (p1, p2, s1, s2)
    } else {
        (p2, p1, s2, s1)
    };
    let p_lo = win_probability(s_lo, s_hi); // P(low id beats high id)
    let lo_wins = game_uniform(run_seed, lo, hi, rematch) < p_lo;
    if (lo == p1) == lo_wins {
        Winner::Player1
    } else {
        Winner::Player2
    }
}

/// Fill every undecided board in the last round by sampling each game's winner
/// from the logistic model on the two players' ground-truth strengths.
///
/// Each game's coin flip comes from [`game_uniform`] keyed on `run_seed` and the
/// pairing (not from a sequential stream), evaluated in the *canonical* low→high
/// orientation, so the outcome depends only on who plays whom — independent of
/// board order and of which side the pairer happened to seat as player 1.
fn autofill_last_round(
    tournament: &mut Tournament,
    strengths: &StrengthMap,
    run_seed: u64,
) -> Result<(), SimError> {
    let last_idx = tournament.rounds.len() - 1;
    let round_number = tournament.rounds[last_idx].number;

    // Decide winners first (immutable borrow), then write them (mutable borrow).
    let decisions: Vec<(usize, Winner)> = {
        let (prior, rest) = tournament.rounds.split_at(last_idx);
        let round = &rest[0];
        round
            .boards
            .iter()
            .enumerate()
            .filter(|(_, b)| b.result.is_none())
            .map(|(i, b)| {
                let rematch = prior_meetings(prior, b.player1, b.player2);
                let winner = decide_board(
                    run_seed,
                    b.player1,
                    b.player2,
                    strengths[&b.player1],
                    strengths[&b.player2],
                    rematch,
                );
                (i, winner)
            })
            .collect()
    };

    for (index, winner) in decisions {
        tournament.toggle_board_winner(round_number, index, winner)?;
    }
    Ok(())
}

/// Players ordered by descending ELO estimate, ties broken by tournament number
/// (deterministic), so the estimate gives a total finishing order too.
fn order_by_estimate(players: &[Player], estimate: &HashMap<Uuid, f64>) -> Vec<Uuid> {
    let mut ids: Vec<&Player> = players.iter().collect();
    ids.sort_by(|a, b| {
        let ea = estimate.get(&a.id).copied().unwrap_or(f64::MIN);
        let eb = estimate.get(&b.id).copied().unwrap_or(f64::MIN);
        eb.partial_cmp(&ea)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.tournament_id.cmp(&b.tournament_id))
    });
    ids.into_iter().map(|p| p.id).collect()
}

/// Simulate one whole tournament: sample strengths, then play `rounds` rounds,
/// each paired by the real engine and auto-filled from the result model.
///
/// `overrides` fixes specific players' true strength (see [`sample_strengths`]);
/// `jitter` scales the prior width for the rest. `cup` runs the hybrid
/// direct-elimination cup when `Some` (see [`CupConfig`]). `rng` seeds both the
/// strength draws and, via a per-run key, the game outcomes, so the run is
/// reproducible from its seed. Game outcomes are keyed on the *pairing* rather
/// than drawn sequentially (see [`game_uniform`]), so two variants sharing this
/// run's seed decide any shared matchup identically — genuine common random
/// numbers, robust to the pairings diverging.
pub fn simulate_run(
    base: &Tournament,
    settings: &TournamentSettings,
    overrides: &StrengthMap,
    jitter: f64,
    rounds: u32,
    cup: Option<&CupConfig>,
    rng: &mut impl Rng,
) -> Result<RunOutcome, SimError> {
    let mut tournament = fresh_state(base, settings, cup)?;
    // A per-run key for the game-keyed outcomes. Drawn before the strength jitter
    // so it depends only on the run's seed, not on how many players were jittered —
    // identical across variants of the same run.
    let run_seed = rng.gen::<u64>();
    let strengths = sample_strengths(
        &tournament.players,
        &tournament.settings,
        overrides,
        jitter,
        rng,
    );

    for round_idx in 0..rounds {
        tournament.prepare_round()?;
        // Reproduce the base tournament's real attendance: whoever actually sat
        // out the corresponding real round sits out here too. Without this, every
        // registered player would play every simulated round — a fuller (and
        // different) tournament than the one being studied — and a player who was
        // absent during the cup window, hence *ineligible* for the cup, would
        // silently reappear playing every round. Rounds beyond the base's recorded
        // history carry no attendance, so everyone plays.
        if let Some(real) = base.rounds.get(round_idx as usize) {
            if !real.absent.is_empty() {
                tournament.update_draft(real.absent.clone(), Vec::new(), None)?;
            }
        }
        tournament.confirm_round()?;
        // Filling in every board result completes the round automatically.
        autofill_last_round(&mut tournament, &strengths, run_seed)?;
    }

    let final_order = tournament
        .standings()
        .into_iter()
        .map(|s| s.player_id)
        .collect();
    let estimate = estimate_elos(
        &tournament.players,
        &tournament.settings,
        &tournament.rounds,
    );
    let estimated_order = order_by_estimate(&tournament.players, &estimate);
    let game_diffs = game_elo_diffs(&tournament, &strengths);
    let cup_champion = tournament.cup_podium().and_then(|p| p.champion);

    Ok(RunOutcome {
        final_order,
        estimated_order,
        game_diffs,
        strengths,
        cup_champion,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::NewPlayer;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn rated(name: &str, rating: u32) -> NewPlayer {
        NewPlayer {
            last_name: name.to_string(),
            rating: Some(rating),
            ..Default::default()
        }
    }

    /// A finalized tournament with the given rated players, ready for `simulate_run`
    /// (which resets it anyway, but this mirrors a loaded base).
    fn tournament_with(players: &[(&str, u32)]) -> Tournament {
        let mut t = Tournament::new("Sim").unwrap();
        for &(name, rating) in players {
            t.add_player(rated(name, rating)).unwrap();
        }
        t
    }

    #[test]
    fn win_probability_is_symmetric_and_monotone() {
        assert!((win_probability(1500.0, 1500.0) - 0.5).abs() < 1e-12);
        // A 400-point edge is ~91% (FESA's 10:1-ish odds).
        assert!((win_probability(1900.0, 1500.0) - 0.909).abs() < 1e-3);
        // Complementary.
        assert!(
            (win_probability(1500.0, 1900.0) + win_probability(1900.0, 1500.0) - 1.0).abs() < 1e-12
        );
    }

    #[test]
    fn overrides_are_taken_exactly_at_zero_jitter() {
        let t = tournament_with(&[("A", 1500), ("B", 1500)]);
        let a = t.players[0].id;
        let mut overrides = StrengthMap::new();
        overrides.insert(a, 2222.0);
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        // At jitter 0 the override is the exact ground truth.
        let s = sample_strengths(&t.players, &t.settings, &overrides, 0.0, &mut rng);
        assert_eq!(s[&a], 2222.0);
    }

    #[test]
    fn jitter_spreads_around_the_override_not_the_rating() {
        // A player registered at 1259 whose post-tournament strength (the override)
        // is 1480: with jitter on, the ground truth must scatter around 1480 (the
        // post value), not the 1259 registration rating.
        let t = tournament_with(&[("Real", 1259), ("B", 1500)]);
        let real = t.players[0].id;
        let mut overrides = StrengthMap::new();
        overrides.insert(real, 1480.0);
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let mut samples = Vec::new();
        for _ in 0..4000 {
            let s = sample_strengths(&t.players, &t.settings, &overrides, 1.0, &mut rng);
            samples.push(s[&real]);
        }
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        let spread = {
            let m = mean;
            (samples.iter().map(|x| (x - m).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
        };
        // Centered on the override (1480), clearly away from the rating (1259)...
        assert!((mean - 1480.0).abs() < 15.0, "mean {mean} not ~1480");
        assert!((mean - 1259.0).abs() > 150.0, "mean {mean} sits near the rating");
        // ...with real, moderate spread (jitter actually applied to the override).
        assert!(spread > 10.0, "override was effectively pinned: spread {spread}");
    }

    #[test]
    fn zero_jitter_pins_strength_to_the_rating() {
        let t = tournament_with(&[("A", 1500), ("B", 1800)]);
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let s = sample_strengths(&t.players, &t.settings, &StrengthMap::new(), 0.0, &mut rng);
        assert_eq!(s[&t.players[0].id], 1500.0);
        assert_eq!(s[&t.players[1].id], 1800.0);
    }

    #[test]
    fn jitter_spread_is_rating_dependent() {
        // A strong (established) player has a tighter prior than a weak one, so at
        // the same jitter their sampled strength should vary less. Compare the
        // empirical std over many draws.
        let mut strong = rated("Strong", 2300);
        strong.fesa_games = Some(50);
        let mut weak = rated("Weak", 800);
        weak.fesa_games = Some(50);
        let mut t = Tournament::new("Sim").unwrap();
        let strong_id = t.add_player(strong).unwrap().id;
        let weak_id = t.add_player(weak).unwrap().id;

        let mut rng = ChaCha8Rng::seed_from_u64(7);
        let (mut ss, mut sw) = (Vec::new(), Vec::new());
        for _ in 0..2000 {
            let s = sample_strengths(&t.players, &t.settings, &StrengthMap::new(), 1.0, &mut rng);
            ss.push(s[&strong_id]);
            sw.push(s[&weak_id]);
        }
        let std = |v: &[f64]| {
            let m = v.iter().sum::<f64>() / v.len() as f64;
            (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        assert!(
            std(&ss) < std(&sw),
            "strong prior should be tighter: {} vs {}",
            std(&ss),
            std(&sw)
        );
    }

    #[test]
    fn a_run_decides_every_board_and_ranks_all_players() {
        let base = tournament_with(&[("A", 1600), ("B", 1500), ("C", 1400), ("D", 1300)]);
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let out = simulate_run(
            &base,
            &base.settings,
            &StrengthMap::new(),
            0.0,
            3,
            None,
            &mut rng,
        )
        .unwrap();
        assert_eq!(out.final_order.len(), 4);
        assert_eq!(out.estimated_order.len(), 4);
        // 4 players × 3 rounds = 2 boards/round × 3 = 6 played games, no byes.
        assert_eq!(out.game_diffs.len(), 6);
        assert!(out.game_diffs.iter().all(|d| *d >= 0.0));
    }

    #[test]
    fn simulation_reproduces_the_base_tournaments_absences() {
        // Base round 1: A beats B while C and D sit out (`0-`). A faithful
        // re-simulation must reproduce that attendance — pairing only A and B — so
        // the round yields a single played game, not the two it would if every
        // registered player were pulled back in.
        let grid = "\
[Abs]
Nr Name    Nat Elo  1   Pts
1  [A] [a] FR  2000 [2+] 1
2  [B] [b] FR  1500 [1-] 0
3  [C] [c] FR  1000 [0-] 0
4  [D] [d] FR   900 [0-] 0
";
        let base = crate::import_american_grid(grid).unwrap();
        assert_eq!(base.rounds.len(), 1);
        assert_eq!(base.rounds[0].absent.len(), 2); // C and D really sat out

        let mut rng = ChaCha8Rng::seed_from_u64(7);
        let out =
            simulate_run(&base, &base.settings, &StrengthMap::new(), 0.0, 1, None, &mut rng).unwrap();
        // All four are still ranked, but only the A–B board was actually played.
        assert_eq!(out.final_order.len(), 4);
        assert_eq!(out.game_diffs.len(), 1);
    }

    #[test]
    fn same_seed_reproduces_the_run() {
        let base = tournament_with(&[("A", 1600), ("B", 1500), ("C", 1400), ("D", 1300)]);
        let run = |seed| {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            simulate_run(
                &base,
                &base.settings,
                &StrengthMap::new(),
                0.5,
                3,
                None,
                &mut rng,
            )
            .unwrap()
        };
        let a = run(99);
        let b = run(99);
        assert_eq!(a.final_order, b.final_order);
        assert_eq!(a.game_diffs, b.game_diffs);
        // A different seed should (with overwhelming probability) differ somewhere.
        let c = run(100);
        assert!(a.final_order != c.final_order || a.game_diffs != c.game_diffs);
    }

    #[test]
    fn a_dominant_player_usually_wins() {
        // One far-stronger player among peers should win most of the time with no
        // rating noise — a sanity check on the whole pipeline.
        let base = tournament_with(&[("Star", 2400), ("B", 1500), ("C", 1450), ("D", 1400)]);
        let star = base.players[0].id;
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let mut wins = 0;
        let runs = 200;
        for _ in 0..runs {
            let out = simulate_run(
                &base,
                &base.settings,
                &StrengthMap::new(),
                0.0,
                3,
                None,
                &mut rng,
            )
            .unwrap();
            if out.winner() == Some(star) {
                wins += 1;
            }
        }
        assert!(
            wins as f64 / runs as f64 > 0.7,
            "dominant player won only {wins}/{runs}"
        );
    }

    #[test]
    fn game_keyed_outcome_is_orientation_independent() {
        // The whole point of game-keying: the same matchup decides the same way no
        // matter which player the pairer seats first — so two variants that seat a
        // shared pairing differently still agree. Check across many run keys.
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let (sa, sb) = (1600.0, 1500.0);
        let concrete = |w: Winner, p1: Uuid, p2: Uuid| match w {
            Winner::Player1 => p1,
            Winner::Player2 => p2,
        };
        for seed in 0..1000u64 {
            let w_ab = decide_board(seed, a, b, sa, sb, 0);
            let w_ba = decide_board(seed, b, a, sb, sa, 0);
            assert_eq!(
                concrete(w_ab, a, b),
                concrete(w_ba, b, a),
                "orientation flip disagreed at seed {seed}"
            );
        }
    }

    #[test]
    fn game_uniform_is_in_range_deterministic_and_keyed() {
        let a = Uuid::from_u128(10);
        let b = Uuid::from_u128(20);
        let u0 = game_uniform(7, a, b, 0);
        assert!((0.0..1.0).contains(&u0));
        assert_eq!(u0, game_uniform(7, a, b, 0)); // deterministic
        assert_ne!(u0, game_uniform(7, a, b, 1)); // rematch re-keys the draw
        assert_ne!(u0, game_uniform(8, a, b, 0)); // run seed re-keys the draw
        assert_ne!(u0, game_uniform(7, a, Uuid::from_u128(21), 0)); // opponent matters

        // Roughly uniform across many distinct pairings (mean ≈ 0.5).
        let n = 4000;
        let mean: f64 = (0..n)
            .map(|i| game_uniform(1, Uuid::from_u128(i), Uuid::from_u128(i + 1_000_000), 0))
            .sum::<f64>()
            / n as f64;
        assert!((mean - 0.5).abs() < 0.03, "mean {mean} not ~0.5");
    }

    #[test]
    fn a_cup_config_runs_the_bracket() {
        // Eight players, all eligible for a size-8 cup: the first log2(8)=3 rounds
        // are the bracket, and the run completes with a decided podium.
        let base = tournament_with(&[
            ("A", 2000),
            ("B", 1900),
            ("C", 1800),
            ("D", 1700),
            ("E", 1600),
            ("F", 1500),
            ("G", 1400),
            ("H", 1300),
        ]);
        let eligible: HashSet<Uuid> = base.players.iter().map(|p| p.id).collect();
        let cup = CupConfig { eligible, size: 8 };
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let out = simulate_run(
            &base,
            &base.settings,
            &StrengthMap::new(),
            0.0,
            3,
            Some(&cup),
            &mut rng,
        )
        .unwrap();
        assert_eq!(out.final_order.len(), 8);
        // The cup's three rounds each have 4 boards → 12 decided cup games.
        assert!(out.game_diffs.len() >= 12);
        // With every match decided, the bracket names a champion, and it is one of
        // the eligible players.
        let champion = out.cup_champion.expect("a decided cup has a champion");
        assert!(base.players.iter().any(|p| p.id == champion));
    }

    #[test]
    fn cup_eligibility_filters_by_nationality_and_attendance() {
        use crate::player::NewPlayer;
        let mut t = Tournament::new("T").unwrap();
        let mk = |t: &mut Tournament, last: &str, nat: &str| {
            t.add_player(NewPlayer {
                last_name: last.into(),
                rating: Some(1500),
                nationality: Some(nat.into()),
                ..Default::default()
            })
            .unwrap()
            .id
        };
        let fr_present = mk(&mut t, "A", "fr"); // lower-case → stored FR
        let fr_absent = mk(&mut t, "B", "FR");
        let jp = mk(&mut t, "C", "JP");
        // Round 1 with fr_absent sitting out.
        t.rounds.push(crate::round::Round {
            number: 1,
            boards: Vec::new(),
            bye: None,
            cup_byes: Vec::new(),
            absent: vec![fr_absent],
            completed: true,
        });

        let nations: HashSet<String> = ["FR".to_string()].into_iter().collect();
        let elig = cup_eligibility(&t, &nations, 5);
        assert!(elig.contains(&fr_present));
        assert!(!elig.contains(&fr_absent)); // right nation, but absent in the window
        assert!(!elig.contains(&jp)); // wrong nation
    }

    #[test]
    fn too_few_eligible_for_the_cup_is_rejected() {
        // Cup size 8 but only 2 players → finalization refuses.
        let base = tournament_with(&[("A", 1600), ("B", 1500)]);
        let eligible: HashSet<Uuid> = base.players.iter().map(|p| p.id).collect();
        let cup = CupConfig { eligible, size: 8 };
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let err = simulate_run(
            &base,
            &base.settings,
            &StrengthMap::new(),
            0.0,
            1,
            Some(&cup),
            &mut rng,
        );
        assert!(matches!(err, Err(SimError::Tournament(_))));
    }

    #[test]
    fn too_few_players_is_rejected() {
        let base = tournament_with(&[("Solo", 1500)]);
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let err = simulate_run(
            &base,
            &base.settings,
            &StrengthMap::new(),
            0.0,
            1,
            None,
            &mut rng,
        );
        assert!(matches!(err, Err(SimError::NotEnoughPlayers { have: 1 })));
    }

    #[test]
    fn game_elo_diffs_uses_the_supplied_map_and_skips_byes() {
        // Three players → each round has one board and one bye; the bye contributes
        // no diff. Run one round and check we get exactly one gap, matching the map.
        let base = tournament_with(&[("A", 1700), ("B", 1500), ("C", 1300)]);
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let out = simulate_run(
            &base,
            &base.settings,
            &StrengthMap::new(),
            0.0,
            1,
            None,
            &mut rng,
        )
        .unwrap();
        assert_eq!(out.game_diffs.len(), 1); // one board, one bye
                                             // The single gap equals the strength difference of whichever two met.
        assert!(out.game_diffs[0] > 0.0);
    }
}
