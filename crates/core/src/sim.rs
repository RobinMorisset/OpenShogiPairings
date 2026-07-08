//! Monte-Carlo tournament simulation.
//!
//! The pieces a statistical study of pairing settings needs, as pure functions
//! reusing the real engine (`prepare_round` → `confirm_round` → result →
//! `complete_current_round`) so a simulated tournament is paired exactly as a live
//! one would be. The CLI ([`crates/sim`](../../sim)) links these directly and runs
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
/// An explicit `overrides` entry is taken as **known truth** (no jitter).
/// Otherwise the strength is drawn from the player's `elo.rs` prior
/// `N(rating, (jitter·σ₀)²)` — so `jitter = 0` pins truth to the registration
/// rating, `jitter = 1` samples from the estimator's own prior (tighter for
/// strong/established players, wider for provisional/unrated ones), and `>1`
/// stress-tests worse-than-assumed ratings. Players are visited in slice order so
/// the draws — and thus the whole run — are reproducible from the seed.
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
            let strength = match overrides.get(&p.id) {
                Some(&known) => known,
                None => {
                    let (mean, std) = player_prior(p, settings);
                    if jitter <= 0.0 {
                        mean
                    } else {
                        sample_normal(rng, mean, jitter * std)
                    }
                }
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

/// Fill every undecided board in the last round by sampling each game's winner
/// from the logistic model on the two players' ground-truth strengths.
fn autofill_last_round(
    tournament: &mut Tournament,
    strengths: &StrengthMap,
    rng: &mut impl Rng,
) -> Result<(), SimError> {
    let round_number = tournament
        .rounds
        .last()
        .expect("a round was just confirmed")
        .number;

    // Decide winners first (immutable borrow), then write them (mutable borrow),
    // so the `rng` and board mutations don't overlap the read.
    let decisions: Vec<(usize, Winner)> = {
        let round = tournament.rounds.last().expect("round present");
        round
            .boards
            .iter()
            .enumerate()
            .filter(|(_, b)| b.result.is_none())
            .map(|(i, b)| {
                let p = win_probability(strengths[&b.player1], strengths[&b.player2]);
                let winner = if rng.gen::<f64>() < p {
                    Winner::Player1
                } else {
                    Winner::Player2
                };
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
/// direct-elimination cup when `Some` (see [`CupConfig`]). `rng` drives both the
/// strength draws and the game outcomes, so the run is reproducible from its seed.
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
    let strengths = sample_strengths(
        &tournament.players,
        &tournament.settings,
        overrides,
        jitter,
        rng,
    );

    for _ in 0..rounds {
        tournament.prepare_round()?;
        tournament.confirm_round()?;
        autofill_last_round(&mut tournament, &strengths, rng)?;
        tournament.complete_current_round()?;
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

    Ok(RunOutcome {
        final_order,
        estimated_order,
        game_diffs,
        strengths,
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
    fn overrides_are_taken_exactly_without_jitter() {
        let t = tournament_with(&[("A", 1500), ("B", 1500)]);
        let a = t.players[0].id;
        let mut overrides = StrengthMap::new();
        overrides.insert(a, 2222.0);
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        // Even with a huge jitter, an override is never perturbed.
        let s = sample_strengths(&t.players, &t.settings, &overrides, 5.0, &mut rng);
        assert_eq!(s[&a], 2222.0);
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
