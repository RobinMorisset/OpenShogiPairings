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
//! - a **strength model** — each player's ground-truth strength for a run is drawn
//!   from `N(center, (jitter·σ₀)²)`, the `center` being an override else the
//!   registration rating and `σ₀` the oracle prior width, so the noise is
//!   rating-dependent (see [`sample_strengths`]);
//! - a **run driver** — reset a base tournament, play `rounds` auto-filled rounds,
//!   and return the finishing order plus the per-game strength gaps.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use rand::Rng;
use uuid::Uuid;

use crate::elo::{estimate_elos, oracle_prior, UNRATED_PRIOR_DEFAULT_K, UNRATED_PRIOR_MEAN};
use crate::player::Player;
use crate::round::Winner;
use crate::settings::TournamentSettings;
use crate::tournament::{Tournament, TournamentError, MIN_PLAYERS_PER_ROUND};

/// Ground-truth strengths (ELO scale) keyed by player id — the "true" playing
/// strength outcomes are drawn from, distinct from a registration rating.
pub type StrengthMap = HashMap<Uuid, f64>;

/// The settings-independent "truth model" the simulator draws each player's
/// ground-truth strength from — the oracle-prior knobs, shared by every settings
/// variant of a run (a variant's pairing knobs must never move the true world).
/// The oracle counterparts of the estimator's `elo_*` settings, set from the CLI.
#[derive(Debug, Clone, Copy)]
pub struct OracleModel {
    /// Global multiplier on every player's oracle-prior width: `0` pins each
    /// player to their center, `>0` scatters them around it (`--jitter`).
    pub jitter: f64,
    /// Extra width multiplier for a provisionally-rated player, clamped ≥ 1 in
    /// [`oracle_prior`] (`--oracle-provisional`).
    pub provisional: f64,
    /// Center (mean) of the oracle prior for an unrated player
    /// (`--oracle-unrated-center`).
    pub unrated_center: f64,
    /// K setting the width of the unrated oracle prior, `σ = √(K·s)`
    /// (`--oracle-unrated-k`).
    pub unrated_k: f64,
}

impl OracleModel {
    /// The oracle model with the default unrated prior (center
    /// [`UNRATED_PRIOR_MEAN`], K [`UNRATED_PRIOR_DEFAULT_K`]) and the given global
    /// jitter and provisional-width multiplier — the common case in tests and
    /// when the unrated defaults are untouched.
    pub fn new(jitter: f64, provisional: f64) -> Self {
        OracleModel {
            jitter,
            provisional,
            unrated_center: UNRATED_PRIOR_MEAN,
            unrated_k: UNRATED_PRIOR_DEFAULT_K,
        }
    }
}

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
/// registration rating (the `elo.rs` prior mean). The **width** `σ₀` is the
/// oracle prior ([`oracle_prior`]): the player's rating-dependent width at the raw
/// FESA K (tighter for strong/established players, wider for provisional/unrated
/// ones), widened for provisional players by `provisional_mult`.
///
/// Crucially the width does **not** depend on any [`TournamentSettings`]: the true
/// strengths are one physical world shared by every settings variant, so a
/// per-variant pairing knob must never move them. `oracle.jitter` is the
/// simulator's own global scale on that width — `0` pins each player to their
/// center exactly (the override, else the rating), and `>0` spreads them around it
/// (around the post-tournament ELO when a results table supplied it, not the
/// pre-tournament rating). `oracle`'s provisional and unrated knobs shape the
/// per-player width the same way the estimator's settings do. Players are visited
/// in slice order so the draws — and thus the whole run — are reproducible from
/// the seed and identical across variants.
pub fn sample_strengths(
    players: &[Player],
    overrides: &StrengthMap,
    oracle: &OracleModel,
    rng: &mut impl Rng,
) -> StrengthMap {
    players
        .iter()
        .map(|p| {
            let (prior_mean, std) = oracle_prior(
                p,
                oracle.provisional,
                oracle.unrated_center,
                oracle.unrated_k,
            );
            // The override (post-tournament / known) strength is the mean to jitter
            // around; fall back to the registration rating when there is none.
            let center = overrides.get(&p.id).copied().unwrap_or(prior_mean);
            let strength = if oracle.jitter <= 0.0 {
                center
            } else {
                sample_normal(rng, center, oracle.jitter * std)
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

/// Binary (Shannon) entropy of a Bernoulli(`p`) outcome, in bits — the "suspense"
/// in a game the model gives win probability `p`: 1 bit at `p = 0.5` (a coin
/// flip), 0 as `p → 0` or `1` (a foregone conclusion). Orientation-free, since
/// `H(p) = H(1 − p)`.
fn binary_entropy(p: f64) -> f64 {
    if p <= 0.0 || p >= 1.0 {
        return 0.0;
    }
    -(p * p.log2() + (1.0 - p) * (1.0 - p).log2())
}

/// Sen welfare `μ·(1 − Gini)` of non-negative `values`: a mean discounted by
/// inequality, so it rewards *both* a higher level and a more even spread (bare
/// Gini, being scale-free, ignores the level). With the values sorted ascending it
/// is the rank-weighted mean `(1/n²)·Σ_k (2(n−k)+1)·x₍ₖ₎` — the smallest value
/// weighted `2n−1` down to the largest weighted `1`. Returns 0 for an empty slice.
/// See [`weighted_sen_welfare`] for the frequency-weighted generalization.
pub fn sen_welfare(values: &[f64]) -> f64 {
    weighted_sen_welfare(values, &vec![1.0; values.len()])
}

/// Frequency-weighted Sen welfare `μ_w·(1 − G_w)`: as [`sen_welfare`] but each
/// value carries a `weight` (here, a player's game count), so lightly-observed —
/// hence noisier — values are discounted. Exact for integer weights (equivalent to
/// expanding each value into `weight` copies): with the values sorted ascending
/// and `W_j` the cumulative weight through `j` (`W_0 = 0`, `M = ΣW`), it is
/// `(1/M²)·Σ_j x₍ⱼ₎·[(2M+1)·w_j − (W_j(W_j+1) − W_{j-1}(W_{j-1}+1))]`. Reduces to
/// [`sen_welfare`] at equal weights; the weighted mean `μ_w = Σw·x/Σw` here is
/// exactly the mean per-game interest. Returns 0 for empty or zero total weight.
pub fn weighted_sen_welfare(values: &[f64], weights: &[f64]) -> f64 {
    let (brackets, total) = rank_brackets(values, weights);
    if total <= 0.0 {
        return 0.0;
    }
    let acc: f64 = values.iter().zip(&brackets).map(|(x, b)| x * b).sum();
    acc / (total * total)
}

/// Each value's rank-slice weight in the [`weighted_sen_welfare`] sum, returned in
/// *input* order, together with the total weight `M`. `Σ bracketⱼ = M²`, so
/// `W = Σ xⱼ·bracketⱼ / M²`.
fn rank_brackets(values: &[f64], weights: &[f64]) -> (Vec<f64>, f64) {
    let n = values.len();
    let total: f64 = weights.iter().sum();
    let mut brackets = vec![0.0; n];
    if n == 0 || total <= 0.0 {
        return (brackets, total);
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| values[a].partial_cmp(&values[b]).unwrap_or(Ordering::Equal));
    let mut cum = 0.0;
    for &i in &idx {
        let w = weights[i];
        let prev = cum;
        cum += w;
        brackets[i] = (2.0 * total + 1.0) * w - (cum * (cum + 1.0) - prev * (prev + 1.0));
    }
    (brackets, total)
}

/// Each value's contribution to the welfare *shortfall* `1 − weighted_sen_welfare`,
/// in input order: `dⱼ = bracketⱼ·(1 − xⱼ)/M²`, which sum to exactly `1 − W`. The
/// largest `dⱼ` mark the values that most lowered the welfare — the lowest values,
/// discounted by their weight (so at equal weights it is simply "lowest first").
/// Assumes values in `[0, 1]`.
pub fn welfare_shortfall(values: &[f64], weights: &[f64]) -> Vec<f64> {
    let (brackets, total) = rank_brackets(values, weights);
    if total <= 0.0 {
        return vec![0.0; values.len()];
    }
    values
        .iter()
        .zip(&brackets)
        .map(|(x, b)| b * (1.0 - x) / (total * total))
        .collect()
}

/// Each player's game interest for one tournament: `(player, Σ entropy, #games)`,
/// where a game's interest is its outcome entropy ([`binary_entropy`]) on the two
/// true strengths. Byes are not games (skipped); a player with no games is absent
/// from the result. The building block for [`interest_welfare`].
pub fn player_game_interest(
    tournament: &Tournament,
    strengths: &StrengthMap,
) -> Vec<(Uuid, f64, u32)> {
    let mut sum: HashMap<Uuid, f64> = HashMap::new();
    let mut count: HashMap<Uuid, u32> = HashMap::new();
    for round in &tournament.rounds {
        for board in &round.boards {
            if board.result.is_none() {
                continue;
            }
            if let (Some(&a), Some(&b)) =
                (strengths.get(&board.player1), strengths.get(&board.player2))
            {
                let h = binary_entropy(win_probability(a, b));
                for id in [board.player1, board.player2] {
                    *sum.entry(id).or_default() += h;
                    *count.entry(id).or_default() += 1;
                }
            }
        }
    }
    sum.into_iter().map(|(id, s)| (id, s, count[&id])).collect()
}

/// The **game-interest** metric: the game-count-weighted Sen welfare of the
/// players' mean game interest. It is 1 when every player's games are coin flips,
/// and falls toward 0 as games become foregone conclusions *and* as that dullness
/// concentrates on fewer players (the welfare's inequality aversion). Each player
/// is weighted by their number of games, so someone who sat out most rounds — and
/// so has a noisy interest — is discounted. Higher = better, like fidelity.
pub fn interest_welfare(tournament: &Tournament, strengths: &StrengthMap) -> f64 {
    let per_player = player_game_interest(tournament, strengths);
    let values: Vec<f64> = per_player.iter().map(|&(_, s, c)| s / c as f64).collect();
    let weights: Vec<f64> = per_player.iter().map(|&(_, _, c)| c as f64).collect();
    weighted_sen_welfare(&values, &weights)
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
    /// The game-interest metric for this run: the game-weighted Sen welfare of the
    /// players' mean game interest (see [`interest_welfare`]). Higher = better.
    pub interest: f64,
    /// Per-player game interest this run — `(player, Σ entropy, #games)` — retained
    /// so aggregation can pool it across runs and name the players whose dull games
    /// most lowered `interest` (see [`player_game_interest`]).
    pub player_interest: Vec<(Uuid, f64, u32)>,
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
            (board.player1 == a && board.player2 == b) || (board.player1 == b && board.player2 == a)
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
/// `oracle` is the settings-independent truth model (jitter, provisional width,
/// and the unrated prior), not part of `settings` (which only drives pairing).
/// `cup` runs the hybrid direct-elimination cup when `Some` (see [`CupConfig`]).
/// `rng` seeds both the strength draws and, via a per-run key, the game outcomes,
/// so the run is reproducible from its seed. Game outcomes are keyed on the
/// *pairing* rather than drawn sequentially (see [`game_uniform`]), so two variants
/// sharing this run's seed decide any shared matchup identically — genuine common
/// random numbers, robust to the pairings diverging.
pub fn simulate_run(
    base: &Tournament,
    settings: &TournamentSettings,
    overrides: &StrengthMap,
    oracle: &OracleModel,
    rounds: u32,
    cup: Option<&CupConfig>,
    rng: &mut impl Rng,
) -> Result<RunOutcome, SimError> {
    let mut tournament = fresh_state(base, settings, cup)?;
    // A per-run key for the game-keyed outcomes. Drawn before the strength jitter
    // so it depends only on the run's seed, not on how many players were jittered —
    // identical across variants of the same run.
    let run_seed = rng.gen::<u64>();
    let strengths = sample_strengths(&tournament.players, overrides, oracle, rng);

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
    let player_interest = player_game_interest(&tournament, &strengths);
    let interest = {
        let values: Vec<f64> = player_interest
            .iter()
            .map(|&(_, s, c)| s / c as f64)
            .collect();
        let weights: Vec<f64> = player_interest.iter().map(|&(_, _, c)| c as f64).collect();
        weighted_sen_welfare(&values, &weights)
    };

    Ok(RunOutcome {
        final_order,
        estimated_order,
        game_diffs,
        strengths,
        cup_champion,
        interest,
        player_interest,
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
        let s = sample_strengths(
            &t.players,
            &overrides,
            &OracleModel::new(0.0, 2.0),
            &mut rng,
        );
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
            let s = sample_strengths(
                &t.players,
                &overrides,
                &OracleModel::new(1.0, 2.0),
                &mut rng,
            );
            samples.push(s[&real]);
        }
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        let spread = {
            let m = mean;
            (samples.iter().map(|x| (x - m).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
        };
        // Centered on the override (1480), clearly away from the rating (1259)...
        assert!((mean - 1480.0).abs() < 15.0, "mean {mean} not ~1480");
        assert!(
            (mean - 1259.0).abs() > 150.0,
            "mean {mean} sits near the rating"
        );
        // ...with real, moderate spread (jitter actually applied to the override).
        assert!(
            spread > 10.0,
            "override was effectively pinned: spread {spread}"
        );
    }

    #[test]
    fn zero_jitter_pins_strength_to_the_rating() {
        let t = tournament_with(&[("A", 1500), ("B", 1800)]);
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let s = sample_strengths(
            &t.players,
            &StrengthMap::new(),
            &OracleModel::new(0.0, 2.0),
            &mut rng,
        );
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
            let s = sample_strengths(
                &t.players,
                &StrengthMap::new(),
                &OracleModel::new(1.0, 2.0),
                &mut rng,
            );
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
    fn oracle_provisional_widens_only_provisional_players() {
        // Same rating, but one player is established (enough FESA games) and the
        // other provisional (none). The oracle provisional multiplier should widen
        // the provisional player's true-strength spread and leave the established
        // player's untouched.
        let mut established = rated("Est", 1500);
        established.fesa_games = Some(crate::PROVISIONAL_GAMES_THRESHOLD);
        let provisional = rated("Prov", 1500); // fesa_games None → provisional
        let mut t = Tournament::new("Sim").unwrap();
        let est_id = t.add_player(established).unwrap().id;
        let prov_id = t.add_player(provisional).unwrap().id;

        let std = |v: &[f64]| {
            let m = v.iter().sum::<f64>() / v.len() as f64;
            (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        let spreads = |prov_mult: f64| {
            let mut rng = ChaCha8Rng::seed_from_u64(9);
            let (mut e, mut p) = (Vec::new(), Vec::new());
            for _ in 0..4000 {
                let s = sample_strengths(
                    &t.players,
                    &StrengthMap::new(),
                    &OracleModel::new(1.0, prov_mult),
                    &mut rng,
                );
                e.push(s[&est_id]);
                p.push(s[&prov_id]);
            }
            (std(&e), std(&p))
        };
        let (e1, p1) = spreads(1.0);
        let (e4, p4) = spreads(4.0);
        // Established player: reliability is 1.0 regardless, so spread is unchanged.
        assert!(
            (e4 - e1).abs() < 0.08 * e1,
            "established moved: {e1} vs {e4}"
        );
        // Provisional player: variance scales with the multiplier, so std ∝ √mult —
        // ×4 should roughly double it (√4 = 2).
        assert!(
            p4 > p1 * 1.7,
            "provisional did not widen enough: {p1} → {p4}"
        );
    }

    #[test]
    fn oracle_unrated_center_and_k_control_the_unrated_true_strength() {
        use crate::player::NewPlayer;
        let mut t = Tournament::new("Sim").unwrap();
        let uid = t
            .add_player(NewPlayer {
                last_name: "New".into(),
                rating: None, // unrated
                ..Default::default()
            })
            .unwrap()
            .id;

        // Jitter 0 pins the unrated player to the oracle's unrated center, not 600.
        let mut rng = ChaCha8Rng::seed_from_u64(1);
        let pinned = sample_strengths(
            &t.players,
            &StrengthMap::new(),
            &OracleModel {
                jitter: 0.0,
                provisional: 2.0,
                unrated_center: 900.0,
                unrated_k: 705.0,
            },
            &mut rng,
        );
        assert!(
            (pinned[&uid] - 900.0).abs() < 1e-6,
            "unrated pinned to the oracle center, got {}",
            pinned[&uid]
        );

        // At jitter 1, a bigger unrated K widens the true-strength spread (σ ∝ √K).
        let std = |v: &[f64]| {
            let m = v.iter().sum::<f64>() / v.len() as f64;
            (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        let spread = |k: f64| {
            let mut rng = ChaCha8Rng::seed_from_u64(9);
            let vals: Vec<f64> = (0..4000)
                .map(|_| {
                    sample_strengths(
                        &t.players,
                        &StrengthMap::new(),
                        &OracleModel {
                            jitter: 1.0,
                            provisional: 2.0,
                            unrated_center: 600.0,
                            unrated_k: k,
                        },
                        &mut rng,
                    )[&uid]
                })
                .collect();
            std(&vals)
        };
        // K ×4 → σ ×2 (√4). A generous margin keeps the sampling test robust.
        assert!(
            spread(2800.0) > spread(700.0) * 1.7,
            "a larger unrated K should widen the spread"
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
            &OracleModel::new(0.0, 2.0),
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
    fn binary_entropy_peaks_at_a_coin_flip() {
        assert!((binary_entropy(0.5) - 1.0).abs() < 1e-12);
        assert_eq!(binary_entropy(1.0), 0.0);
        assert_eq!(binary_entropy(0.0), 0.0);
        assert!((binary_entropy(0.1) - binary_entropy(0.9)).abs() < 1e-12); // H(p)=H(1−p)
        assert!(binary_entropy(0.9) < binary_entropy(0.6)); // more lopsided → less suspense
    }

    #[test]
    fn sen_welfare_rewards_level_and_equality() {
        // All-equal returns the plain mean (Gini 0).
        assert!((sen_welfare(&[0.5, 0.5, 0.5, 0.5]) - 0.5).abs() < 1e-12);
        // Maximum inequality for two points: (0,1) → 0.25 = 0.5·(1−0.5).
        assert!((sen_welfare(&[0.0, 1.0]) - 0.25).abs() < 1e-12);
        // Level: a uniformly higher set scores higher.
        assert!(sen_welfare(&[0.4, 0.4]) < sen_welfare(&[0.6, 0.6]));
        // Equality: same mean, but concentration is penalized.
        let spread = sen_welfare(&[0.5, 0.5, 0.5, 0.5]);
        let concentrated = sen_welfare(&[0.0, 0.5, 0.5, 1.0]); // same mean 0.5
        assert!(concentrated < spread);
    }

    #[test]
    fn weighted_sen_welfare_matches_unweighted_and_discounts_light_weights() {
        // Equal weights ≡ the unweighted Sen welfare.
        assert!((weighted_sen_welfare(&[0.0, 1.0], &[1.0, 1.0]) - 0.25).abs() < 1e-12);
        // Integer weights ≡ expanding into copies: (0 w1, 1 w3) ≡ [0,1,1,1] → 0.5625.
        assert!((weighted_sen_welfare(&[0.0, 1.0], &[1.0, 3.0]) - 0.5625).abs() < 1e-12);
        // A low value carrying little weight is discounted: welfare rises as the
        // weight on the low (noisy) value shrinks.
        let heavy_low = weighted_sen_welfare(&[0.0, 1.0], &[3.0, 1.0]);
        let light_low = weighted_sen_welfare(&[0.0, 1.0], &[1.0, 3.0]);
        assert!(light_low > heavy_low);
    }

    #[test]
    fn welfare_shortfall_sums_to_the_deficit_and_blames_the_lowest() {
        let vals = [0.2, 0.9, 0.5, 0.1];
        let wts = [1.0; 4];
        let d = welfare_shortfall(&vals, &wts);
        // The deficits sum to exactly 1 − W.
        let w = weighted_sen_welfare(&vals, &wts);
        assert!((d.iter().sum::<f64>() - (1.0 - w)).abs() < 1e-12);
        // At equal weights the biggest shortfall is the lowest value (index 3 = 0.1).
        let worst = (0..4)
            .max_by(|&a, &b| d[a].partial_cmp(&d[b]).unwrap())
            .unwrap();
        assert_eq!(worst, 3);
        // A low value with a tiny weight is discounted below a mid value with a big
        // weight: shortfall blame shifts off the noisy player.
        let d2 = welfare_shortfall(&[0.05, 0.4], &[1.0, 20.0]);
        assert!(d2[1] > d2[0]);
    }

    #[test]
    fn interest_welfare_is_one_for_coin_flips_and_falls_with_blowouts() {
        // 4 players, one round: A–B and C–D.
        let grid = "\
[Dull]
Nr Name    Nat Elo  1   Pts
1  [A] [a] FR  1500 [2+] 1
2  [B] [b] FR  1500 [1-] 0
3  [C] [c] FR  1500 [4+] 1
4  [D] [d] FR  1500 [3-] 0
";
        let t = crate::import_american_grid(grid).unwrap();
        let id = |last| t.players.iter().find(|p| p.last_name == last).unwrap().id;

        // Equal strengths → every game a coin flip → perfect interest (1).
        let mut equal = StrengthMap::new();
        for p in &t.players {
            equal.insert(p.id, 1500.0);
        }
        assert!((interest_welfare(&t, &equal) - 1.0).abs() < 1e-9);

        // A–B a blowout while C–D stays even: interest falls below 1.
        let mut mixed = StrengthMap::new();
        mixed.insert(id("A"), 2200.0);
        mixed.insert(id("B"), 1000.0);
        mixed.insert(id("C"), 1500.0);
        mixed.insert(id("D"), 1500.0);
        let w = interest_welfare(&t, &mixed);
        assert!(w > 0.0 && w < 1.0, "interest {w} out of range");
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
        let out = simulate_run(
            &base,
            &base.settings,
            &StrengthMap::new(),
            &OracleModel::new(0.0, 2.0),
            1,
            None,
            &mut rng,
        )
        .unwrap();
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
                &OracleModel::new(0.5, 2.0),
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
                &OracleModel::new(0.0, 2.0),
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
            &OracleModel::new(0.0, 2.0),
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
            &OracleModel::new(0.0, 2.0),
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
            &OracleModel::new(0.0, 2.0),
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
            &OracleModel::new(0.0, 2.0),
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
