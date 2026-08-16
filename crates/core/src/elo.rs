//! Bayesian ELO estimation for the experimental non-Swiss pairing mode.
//!
//! Each player has a latent strength `θ` on the ELO scale. Game outcomes follow
//! the logistic (Bradley–Terry) model `P(i beats j) = σ((θᵢ − θⱼ)/s)` with
//! `s = 400/ln 10`, and each player has a prior anchoring `θ` to their
//! registration rating (or, for an unrated player, to a wide, referee-tunable
//! prior — by default `N(600, 350²)`). The prior is Gaussian by default, but a
//! referee can switch to a fatter-tailed Huber-smoothed Laplace prior
//! ([`EloPriorShape`]); either shape can also be made **asymmetric** (an upward
//! revision made on less evidence than a downward one — see the per-category
//! `elo_upward_looseness_*` settings). All are log-concave, so the objective
//! stays concave and the solver keeps its unique-maximum guarantee. The
//! estimated ELO is the **MAP** (maximum a-posteriori) point — the maximiser of
//! the penalised log-likelihood — found by coordinate-ascent Newton. The whole
//! thing is a **pure replay** of the completed rounds, recomputed from scratch
//! each time (like [`crate::scoring::compute_scores`]), so it is order-independent
//! and survives result edits / undo.
//!
//! The design, including why the single K multiplier is the only knob and how the
//! per-game drift auto-decelerates, is written up in `docs/archive/elo-pairing-mode.md`.

use std::collections::HashMap;

use uuid::Uuid;

use crate::player::Player;
use crate::round::{Handicap, Outcome, Round, Winner};
use crate::settings::{EloPriorShape, TournamentSettings};
use crate::units::TournamentId;

/// ELO scale factor `s = 400 / ln 10`: a 400-point gap is 10:1 odds. Also the
/// factor in the prior-width law `σ₀ = √(K · s)`, so it's `pub(crate)` for
/// settings that reason about K.
pub(crate) const S: f64 = 173.717_792_761_565_8;

/// Default prior mean for an unrated player — the midpoint of the assumed
/// `[1, 1200]` strength range. The referee can override it
/// ([`TournamentSettings::elo_unrated_prior_center`]); this is the fallback used
/// where no settings are in hand (e.g. a player missing from an estimate map).
pub(crate) const UNRATED_PRIOR_MEAN: f64 = 600.0;

/// Default K for the unrated-player prior. Its width follows the same
/// `σ₀ = √(K · s)` law as a rated player's, so referees tune one familiar
/// quantity; `705` reproduces the historical `σ ≈ 350` (the std of a uniform on
/// `[1, 1200]` is `1199/√12 ≈ 346`, rounded to 350: `350² / s ≈ 705`).
pub(crate) const UNRATED_PRIOR_DEFAULT_K: f64 = 705.0;

/// A rated player with at least this many FESA games is treated as reliably
/// rated; fewer (or a rating not from the FESA list) makes it *provisional*, and
/// the referee's provisional multiplier widens its prior.
pub(crate) const PROVISIONAL_GAMES_THRESHOLD: u32 = 18;

/// Solver limits: coordinate-ascent sweeps and the per-sweep convergence
/// tolerance (in ELO points). The objective is strongly concave, so this
/// converges quickly and deterministically.
const MAX_SWEEPS: usize = 200;
const CONVERGENCE_TOL: f64 = 1e-4;

/// A generous per-update step clamp (ELO points) guarding against a Newton
/// overshoot when a wide-prior player is far from the optimum; repeated sweeps
/// then close the remaining distance.
const MAX_STEP: f64 = 800.0;

/// Half-width (ELO points) of the quadratic Huber core that rounds off the
/// Laplace prior's kink at the registration rating. Small enough that its effect
/// is negligible (a couple of points) yet it restores a finite, strictly
/// negative curvature there so the Newton step is well-defined and can't chatter
/// across the kink. The core is anchored at the mean (`g(0) = 0`), so a player
/// with no games still sits *exactly* at their prior mean regardless of the
/// asymmetry.
const HUBER_DELTA: f64 = 2.0;

/// The rating an all-loss player floors to under a flat ([`EloPriorShape::Flat`])
/// prior — the bottom of the assumed unrated strength range `[1, 1200]`, matching
/// `turnering.py`'s new-rule floor. A flat prior plus a 0 % score leaves the
/// likelihood monotone (no finite maximiser), so a definite value must be supplied.
const FLAT_ALL_LOSS_FLOOR: f64 = 1.0;

// --- FESA handicap treatment ----------------------------------------------
//
// FESA rates a handicap game as if the giver were weaker: their rating is turned
// into a fractional *grade* number (interpolated on the grades' lower-bound
// ratings), the handicap's grade value is subtracted, and the result is turned
// back into a rating. The drop is a fixed rating-point offset `h` applied to the
// giver's effective strength — so the game contributes a logistic term
// `P(giver wins) = σ((θ_giver − θ_recv − h)/s)`. See
// <https://fesashogi.eu/elo-system/> sections 7 (grades) and 8 (handicap).
//
// `h` is derived from the giver's fixed **registration** rating (like FESA),
// which keeps the offset constant and the likelihood log-concave.

/// Lower-bound rating of each FESA grade, from 20 Kyu (weakest) to 5 Dan
/// (strongest). The array index is the grade's integer coordinate, so a handicap
/// worth *v* grades shifts the coordinate by `v`.
const GRADE_LB: [f64; 25] = [
    1.0, // 20 Kyu
    80.0, 160.0, 240.0, 320.0, 400.0, 480.0, 560.0, 640.0, 720.0, 800.0, // 19..11 Kyu
    880.0, 960.0, 1040.0, 1120.0, 1200.0, 1280.0, 1360.0, 1460.0, 1560.0, // 10..1 Kyu
    1680.0, 1800.0, 1920.0, 2080.0, 2240.0, // 1..5 Dan
];

/// The handicap's value expressed as a number of grades (FESA section 8).
fn handicap_grade_value(handicap: Handicap) -> f64 {
    match handicap {
        Handicap::Sente => 0.2,
        Handicap::Lance => 0.6,
        Handicap::Bishop => 1.5,
        Handicap::Rook => 2.1,
        Handicap::RookLance => 2.7,
        Handicap::TwoPiece => 3.6,
        Handicap::FourPiece => 5.0,
        Handicap::FivePiece => 6.5,
        Handicap::SixPiece => 8.0,
    }
}

/// A rating as a fractional grade coordinate, piecewise-linear on [`GRADE_LB`]
/// and linearly extrapolated with the nearest segment's slope past either end.
fn grade_number(rating: f64) -> f64 {
    let n = GRADE_LB.len();
    if rating <= GRADE_LB[0] {
        let slope = GRADE_LB[1] - GRADE_LB[0];
        return (rating - GRADE_LB[0]) / slope;
    }
    for i in 0..n - 1 {
        if rating < GRADE_LB[i + 1] {
            return i as f64 + (rating - GRADE_LB[i]) / (GRADE_LB[i + 1] - GRADE_LB[i]);
        }
    }
    let slope = GRADE_LB[n - 1] - GRADE_LB[n - 2];
    (n - 1) as f64 + (rating - GRADE_LB[n - 1]) / slope
}

/// The inverse of [`grade_number`]: the rating at a fractional grade coordinate.
fn rating_at_grade(grade: f64) -> f64 {
    let n = GRADE_LB.len();
    if grade <= 0.0 {
        let slope = GRADE_LB[1] - GRADE_LB[0];
        return GRADE_LB[0] + grade * slope;
    }
    let i = grade.floor() as usize;
    if i >= n - 1 {
        let slope = GRADE_LB[n - 1] - GRADE_LB[n - 2];
        return GRADE_LB[n - 1] + (grade - (n - 1) as f64) * slope;
    }
    GRADE_LB[i] + (grade - i as f64) * (GRADE_LB[i + 1] - GRADE_LB[i])
}

/// The rating-point handicap effect `h > 0` for a giver of the given (fixed)
/// rating conceding `handicap`: how much weaker the giver plays. Computed by
/// dropping `handicap_grade_value` grades from the giver's grade and measuring
/// the rating difference.
fn handicap_offset(giver_rating: u32, handicap: Handicap) -> f64 {
    let giver = f64::from(giver_rating);
    let shifted = rating_at_grade(grade_number(giver) - handicap_grade_value(handicap));
    giver - shifted
}

/// FESA's rating-dependent development coefficient K (section 1 "Basic formula"
/// of <https://fesashogi.eu/elo-system/>), used only to seed each rated player's
/// prior width (see [`prior`]). The thresholds fall on grade boundaries.
fn fesa_k(rating: u32) -> f64 {
    match rating {
        r if r >= 2240 => 16.0,
        r if r >= 1920 => 20.0,
        r if r >= 1560 => 24.0,
        r if r >= 1280 => 28.0,
        r if r >= 1040 => 32.0,
        r if r >= 720 => 36.0,
        _ => 40.0,
    }
}

/// Whether a rated player's registration rating is reliable enough to trust
/// tightly — in the FESA list with at least [`PROVISIONAL_GAMES_THRESHOLD`]
/// games. A hand-typed rating (no `fesa_games`) or a low game count is
/// provisional.
fn is_reliably_rated(player: &Player) -> bool {
    matches!(player.fesa_games, Some(games) if games >= PROVISIONAL_GAMES_THRESHOLD)
}

/// Whether the ELO estimate has a **scale anchor**: at least one player who is
/// either pinned to a fixed rating (K multiplier 0, see [`estimate_elos`]) or held
/// by a *proper* (non-flat) prior centered on a fixed value — a rated player's
/// registration rating, or an unrated player's `unrated_center`.
///
/// It matters because a flat prior contributes nothing to the objective, so if
/// *every* player is on a flat prior and nothing is pinned, the log-likelihood is
/// invariant under adding the same constant to every `θ`: the scale has a free
/// additive degree of freedom. The estimates are then meaningful only *relative*
/// to each other — their absolute ELO values (and the all-loss floor an all-loss
/// player pins near, [`FLAT_ALL_LOSS_FLOOR`]) are arbitrary. Pairing and
/// estimate-based MacMahon read them as absolute numbers, so an unanchored field
/// yields scale-nonsensical results. Callers validate against this before letting
/// the estimator drive anything (see `Tournament::finalize_registration_with` /
/// `update_settings`).
pub(crate) fn has_scale_anchor(players: &[Player], settings: &TournamentSettings) -> bool {
    let m = settings.elo_k_multiplier();
    players.iter().any(|p| {
        // K multiplier 0 pins a rated player to their registration rating — an
        // absolute anchor, regardless of prior shape.
        if m == 0.0 && p.rating.is_some() {
            return true;
        }
        let shape = match p.rating {
            None => settings.elo_prior_shape_unrated(),
            Some(_) if is_reliably_rated(p) => settings.elo_prior_shape_established(),
            Some(_) => settings.elo_prior_shape_provisional(),
        };
        // A proper prior (Gaussian/Laplace) is centered on a fixed value, so it
        // anchors the scale; a flat prior does not.
        !matches!(shape, EloPriorShape::Flat)
    })
}

/// A player's Gaussian prior `(mean, standard deviation)` on the ELO scale.
///
/// A rated player is centered on their registration rating with a width derived
/// from `K = m · K_FESA(rating)` via `σ₀ = √(K · s)` (so `K` is literally their
/// first-game K factor). A **provisionally**-rated player (see
/// [`is_reliably_rated`]) has that `K` further multiplied by `provisional_mult`,
/// widening the prior so their estimate drifts faster. An unrated player gets a
/// referee-tunable prior centered on `unrated_center` with width `√(unrated_k · s)`
/// — the same `K` law, so `unrated_k` reads on the same scale as a rated player's
/// K (default `705 ≈ σ 350`). The unrated prior is independent of `m` and
/// `provisional_mult`: an unrated player has no registration rating to drift from
/// or be provisional about, so `unrated_k` is itself the only width knob.
fn prior(
    player: &Player,
    k_multiplier: f64,
    provisional_mult: f64,
    unrated_center: f64,
    unrated_k: f64,
) -> (f64, f64) {
    match player.rating {
        Some(rating) => {
            // `k_multiplier` may be 0 here (the "estimate unrated only" mode), giving
            // σ₀ = 0; `estimate_elos` detects that and pins the player instead of
            // using this degenerate prior. The provisional multiplier is clamped ≥ 1.
            let reliability = if is_reliably_rated(player) {
                1.0
            } else {
                provisional_mult
            };
            let k = k_multiplier * fesa_k(rating) * reliability;
            (f64::from(rating), (k * S).sqrt())
        }
        None => (unrated_center, (unrated_k * S).sqrt()),
    }
}

/// A player's prior penalty on the log-posterior, in a form the coordinate-ascent
/// solver can consume directly: given the current deviation `d = θ − μ₀` from the
/// prior mean, it yields that term's contribution to the gradient and Hessian.
///
/// All variants are **log-concave**, so adding them to the (concave)
/// Bradley–Terry log-likelihood keeps the whole objective concave — the unique
/// maximiser and deterministic convergence the solver relies on. The [`Self::Flat`]
/// variant is the improper limit (zero curvature): it is log-concave but supplies
/// no curvature of its own, so a player carrying it needs games (or the solver's
/// degenerate-case guards) to be pinned down.
enum PriorPenalty {
    /// A (possibly asymmetric) Gaussian — a *two-piece normal* with std `σ₀` below
    /// the mean and `r·σ₀` above it, so the linear restoring force `d/σ²` is
    /// gentler on the upward side when `r > 1`. Held as the two precisions
    /// `precision_down = 1/σ₀²` and `precision_up = 1/(r·σ₀)²`. It is `C¹` at the
    /// mode (both arms have zero slope there — no kink to smooth) and strictly
    /// concave, so the solver is unaffected. `r = 1` gives `precision_up =
    /// precision_down`, i.e. the plain symmetric `N(μ₀, σ₀²)`.
    Gaussian {
        precision_down: f64,
        precision_up: f64,
    },
    /// Huber-smoothed **asymmetric Laplace**: outside a `±HUBER_DELTA` core the
    /// penalty is linear with a *constant* restoring force `1/b` — `1/b_up` above
    /// the mean, `1/b_down` below — giving fatter (exponential) tails than the
    /// Gaussian and, when `b_up > b_down`, an easier upward revision. Inside the
    /// core the force is linearised through the origin so the penalty is `C¹`,
    /// strictly convex, and minimised exactly at `d = 0`.
    Laplace { b_down: f64, b_up: f64 },
    /// **Flat** (improper/uniform) prior: contributes `(0, 0)` always, so the
    /// estimate is driven purely by the likelihood — the maximum-likelihood
    /// performance rating, as in `turnering.py`. Carries no width or asymmetry (the
    /// looseness ratio has no arm to widen). A player holding it must be pinned by
    /// their games or by the solver's degenerate-case handling.
    Flat,
}

impl PriorPenalty {
    /// Build the penalty for one player from the chosen `shape`, their (downward)
    /// width `σ₀` (from [`prior`]), and the upward-looseness ratio `r`. In both
    /// shapes the upward arm is widened by `r`: the Gaussian uses `σ_up = r·σ₀`,
    /// and the Laplace scale is variance-matched to it (`b_down = σ₀/√2`,
    /// `b_up = r·b_down`) so the existing K / provisional / unrated-K knobs keep
    /// their meaning. `r = 1` yields the symmetric prior in either shape.
    fn new(shape: EloPriorShape, sigma0: f64, r: f64) -> Self {
        let r = r.max(1.0);
        match shape {
            EloPriorShape::Gaussian => {
                let sigma_up = r * sigma0;
                PriorPenalty::Gaussian {
                    precision_down: 1.0 / (sigma0 * sigma0),
                    precision_up: 1.0 / (sigma_up * sigma_up),
                }
            }
            EloPriorShape::Laplace => {
                let b_down = sigma0 / std::f64::consts::SQRT_2;
                PriorPenalty::Laplace {
                    b_down,
                    b_up: r * b_down,
                }
            }
            // A flat prior has no width and no asymmetry: `sigma0` and `r` are
            // irrelevant.
            EloPriorShape::Flat => PriorPenalty::Flat,
        }
    }

    /// The `(gradient, hessian)` contribution of this prior at deviation `d`.
    /// Both are ≤ 0 curvature-wise (the Hessian term is `≤ 0`), and strictly
    /// negative wherever it matters for a well-defined ascent step: the Gaussian
    /// always, the Laplace inside its core (where a no-games player sits) — and
    /// outside the core any moved player has games, whose likelihood supplies the
    /// strictly negative curvature.
    fn grad_hess(&self, d: f64) -> (f64, f64) {
        match *self {
            PriorPenalty::Gaussian {
                precision_down,
                precision_up,
            } => {
                // Pick the arm by side of the mode. The gradient is continuous at
                // d = 0 (both give 0); the curvature jumps but stays < 0.
                let precision = if d >= 0.0 {
                    precision_up
                } else {
                    precision_down
                };
                (-d * precision, -precision)
            }
            PriorPenalty::Laplace { b_down, b_up } => {
                // g(d) = penalty'(d); the log-posterior takes −penalty, so the
                // gradient contribution is −g and the Hessian is −g'. The core is
                // two straight segments meeting at the origin (g(0) = 0), each
                // running out to the constant ±1/b force at ±HUBER_DELTA.
                let (g, g_prime) = if d >= HUBER_DELTA {
                    (1.0 / b_up, 0.0)
                } else if d <= -HUBER_DELTA {
                    (-1.0 / b_down, 0.0)
                } else if d >= 0.0 {
                    (d / (b_up * HUBER_DELTA), 1.0 / (b_up * HUBER_DELTA))
                } else {
                    (d / (b_down * HUBER_DELTA), 1.0 / (b_down * HUBER_DELTA))
                };
                (-g, -g_prime)
            }
            // Improper flat prior: no restoring force, no curvature. The player's
            // games (or the degenerate-case guards in `estimate_elos`) must supply
            // the curvature that pins them down.
            PriorPenalty::Flat => (0.0, 0.0),
        }
    }
}

/// The **simulation oracle's** prior `(mean, standard deviation)` for a player's
/// *true* strength: the same rating- and reliability-dependent `N(rating, σ₀²)`
/// shape as the estimator's [`prior`], but with the K multiplier fixed at ×1 (the
/// raw FESA K). The oracle is the same physical world for every settings variant,
/// so its width must never depend on a per-variant pairing knob
/// (`elo_k_multiplier`); the simulator scales it globally via its own `jitter`
/// instead. `provisional_mult` and the unrated prior (`unrated_center`,
/// `unrated_k`) are its own oracle-level parameters, distinct from the pairing
/// estimate's settings — the simulator sets them from its CLI so the true world
/// can be studied independently of what a variant *believes*. `provisional_mult`
/// is clamped to ≥ 1 so a provisional rating is never treated as *more* certain
/// than an established one.
///
/// Exposed for the simulator ([`crate::sim`]), which draws each player's
/// ground-truth strength from this prior so the injected noise is rating-dependent
/// and coherent with the estimator rather than a second, flat model.
pub(crate) fn oracle_prior(
    player: &Player,
    provisional_mult: f64,
    unrated_center: f64,
    unrated_k: f64,
) -> (f64, f64) {
    prior(
        player,
        1.0,
        provisional_mult.max(1.0),
        unrated_center,
        unrated_k,
    )
}

/// The oracle prior width (σ₀) for a player *provisionally* rated at `rating`.
///
/// Used by the simulator for a player who was **unrated** at registration but
/// nonetheless has a post-tournament result (an override): earning a rating from a
/// tournament's worth of games makes them *provisionally* rated, not truly unknown,
/// so their true strength should be jittered at this provisional width — the same
/// `√(K · s)` law as a rated player, at the provisional multiplier — rather than the
/// very broad "we know nothing" unrated width. Uses the oracle's ×1 K multiplier,
/// like [`oracle_prior`]; `provisional_mult` is clamped to ≥ 1.
pub(crate) fn oracle_provisional_width(rating: f64, provisional_mult: f64) -> f64 {
    let k = fesa_k(rating.round().max(0.0) as u32) * provisional_mult.max(1.0);
    (k * S).sqrt()
}

/// Expected score `σ(x) = 1/(1+e^−x)` for `x = (θself − θopp)/s`.
fn expected(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Precomputed degenerate-scoreline handling for a flat-prior player (see
/// [`EloPriorShape::Flat`]). A flat prior gives an all-win or all-loss player an
/// unbounded likelihood, so [`estimate_elos`] substitutes `turnering.py`'s rule.
/// Players with a mix of results, or with no games, need no substitution (they are
/// pinned by the likelihood or left at their seed) and carry `None`.
enum FlatDegenerate {
    /// Every counted game lost — pin to [`FLAT_ALL_LOSS_FLOOR`].
    AllLoss,
    /// Every counted game won — add a pseudo-draw against the strongest opponent
    /// (its index into `players`, with that game's handicap `offset`) so the
    /// likelihood has a finite maximum, as in `turnering.py`'s `[best_elo] + …`.
    AllWin { opp: usize, offset: f64 },
}

/// Estimate every player's current ELO (posterior mode) from the completed
/// rounds, using the FESA-K × multiplier prior from `settings`.
///
/// Byes contribute nothing (they are not games) and draws score ½ for each side.
/// Handicap games *are* counted: they use the actual result plus a handicap offset
/// on each side's effective strength (the giver plays as if weaker — see the games
/// loop below), not the effective winner. The returned map has one entry per player
/// in `players`; a player with no counted games sits exactly at their prior mean.
///
/// A K multiplier of 0 (`settings.elo_k_multiplier() == 0`) **pins** every rated
/// player to their registration rating — only unrated players are estimated. This
/// is the "apply estimates to unrated players only" mode: rated players keep their
/// trusted rating and the unrated newcomers are estimated against that fixed field.
pub fn estimate_elos(
    players: &[Player],
    settings: &TournamentSettings,
    rounds: &[Round],
) -> HashMap<Uuid, f64> {
    let m = settings.elo_k_multiplier();
    let provisional = settings.elo_provisional_multiplier();
    let unrated_center = settings.elo_unrated_prior_center();
    let unrated_k = settings.elo_unrated_k();
    let shape_established = settings.elo_prior_shape_established();
    let shape_provisional = settings.elo_prior_shape_provisional();
    let shape_unrated = settings.elo_prior_shape_unrated();
    let looseness_established = settings.elo_upward_looseness_established();
    let looseness_provisional = settings.elo_upward_looseness_provisional();
    let looseness_unrated = settings.elo_upward_looseness_unrated();
    let n = players.len();

    // Boards reference players by tournament number, so index by number here.
    let index: HashMap<TournamentId, usize> = players
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.tournament_id.map(|t| (t, i)))
        .collect();
    let mut theta = vec![0.0_f64; n];
    let mut mean = vec![0.0_f64; n];
    let mut penalties: Vec<PriorPenalty> = Vec::with_capacity(n);
    let mut is_flat = vec![false; n];
    let mut pinned = vec![false; n];
    for (i, p) in players.iter().enumerate() {
        let (mu0, sigma0) = prior(p, m, provisional, unrated_center, unrated_k);
        mean[i] = mu0;
        theta[i] = mu0; // seed at the prior mean
                        // K multiplier 0 pins every *rated* player to their registration rating
                        // (only unrated players are estimated). Their `sigma0` is 0, which would
                        // give a degenerate Gaussian, so mark them pinned and skip their update
                        // entirely; the seed already holds their rating. Unrated players are
                        // unaffected (their prior is independent of `m`).
        if m == 0.0 && p.rating.is_some() {
            pinned[i] = true;
            penalties.push(PriorPenalty::Flat); // unused; the pin skips their update
            continue;
        }
        // The prior shape and the upward-looseness ratio both depend on how much the
        // seed rating is trusted — the same three categories `prior` distinguishes. A
        // fat tail is confined to the category whose true strength is actually
        // heavy-tailed (unrated), keeping established and provisional players on their
        // well-anchored Gaussian.
        let (shape, r) = match p.rating {
            None => (shape_unrated, looseness_unrated),
            Some(_) if is_reliably_rated(p) => (shape_established, looseness_established),
            Some(_) => (shape_provisional, looseness_provisional),
        };
        is_flat[i] = matches!(shape, EloPriorShape::Flat);
        penalties.push(PriorPenalty::new(shape, sigma0, r));
    }

    // Per-player incident games: (opponent index, this player's score in {0, ½, 1},
    // handicap offset added to this player's effective strength). The offset is 0
    // for an even game; for a handicap it is −h on the giver and +h on the receiver
    // (`h` from [`handicap_offset`]), so the giver plays as if weaker.
    let mut games: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); n];
    for round in rounds.iter().filter(|r| r.completed) {
        for board in &round.boards {
            let (Some(&a), Some(&b)) = (index.get(&board.player1), index.get(&board.player2))
            else {
                continue; // an opponent no longer in the tournament
            };
            // Handicaps use the *actual* result (who really won), so score player1
            // straight off the outcome (and a draw as ½), not the effective winner.
            // A draw counts whether or not the decisive replay has finished; a
            // forfeit is not a game and never counts.
            let score_a = match board.outcome() {
                Outcome::Pending { drawn: true } | Outcome::Won { drawn: true, .. } => 0.5,
                Outcome::Won {
                    winner: Winner::Player1,
                    ..
                } => 1.0,
                Outcome::Won {
                    winner: Winner::Player2,
                    ..
                } => 0.0,
                Outcome::Pending { .. } | Outcome::Forfeit { .. } => continue,
            };
            // player1's handicap offset: −h if player1 conceded the odds, +h if it
            // received them, 0 for an even game. Needs the giver's fixed rating.
            let offset_a = match &board.handicap {
                Some(hg) => {
                    let giver = if hg.giver == Winner::Player1 {
                        &players[a]
                    } else {
                        &players[b]
                    };
                    let Some(giver_rating) = giver.rating else {
                        continue; // giver must be rated to size the handicap (always is)
                    };
                    let h = handicap_offset(giver_rating, hg.handicap);
                    if hg.giver == Winner::Player1 {
                        -h
                    } else {
                        h
                    }
                }
                None => 0.0,
            };
            games[a].push((b, score_a, offset_a));
            games[b].push((a, 1.0 - score_a, -offset_a));
        }
    }

    // Degenerate scorelines for flat-prior players, precomputed once from the seeds
    // (theta still holds each player's seed here). A flat prior can't bound an
    // all-win or all-loss likelihood, so those follow `turnering.py`; a mixed
    // scoreline is bounded by the likelihood alone, and a no-games flat player is
    // left at their seed (their Hessian stays 0 and the sweep skips them).
    let flat_degenerate: Vec<Option<FlatDegenerate>> = (0..n)
        .map(|i| {
            if !is_flat[i] || games[i].is_empty() {
                return None;
            }
            if games[i].iter().all(|&(_, s, _)| s == 0.0) {
                Some(FlatDegenerate::AllLoss)
            } else if games[i].iter().all(|&(_, s, _)| s == 1.0) {
                // Strongest opponent by seed rating — a stable pseudo-opponent.
                let &(opp, _, offset) = games[i]
                    .iter()
                    .max_by(|a, b| theta[a.0].total_cmp(&theta[b.0]))
                    .expect("non-empty by the guard above");
                Some(FlatDegenerate::AllWin { opp, offset })
            } else {
                None
            }
        })
        .collect();

    // Coordinate ascent: sweep players, each a 1-D Newton step on the concave
    // penalised log-likelihood. Strong concavity (every player has a finite prior
    // precision, or — for a flat prior — enough game curvature after the degenerate
    // guards) gives a unique maximum and convergence.
    let mut converged = false;
    for _ in 0..MAX_SWEEPS {
        let mut max_delta = 0.0_f64;
        for i in 0..n {
            // A pinned player (rated, with K multiplier 0) never moves off their
            // registration rating.
            if pinned[i] {
                continue;
            }
            // A flat all-loss player has no finite maximum; pin them to the floor.
            if let Some(FlatDegenerate::AllLoss) = flat_degenerate[i] {
                let step = FLAT_ALL_LOSS_FLOOR - theta[i];
                theta[i] = FLAT_ALL_LOSS_FLOOR;
                max_delta = max_delta.max(step.abs());
                continue;
            }
            // Prior term first, then each game's likelihood contribution.
            let (mut gradient, mut hessian) = penalties[i].grad_hess(theta[i] - mean[i]);
            for &(j, score, offset) in &games[i] {
                let e = expected((theta[i] - theta[j] + offset) / S);
                gradient += (score - e) / S;
                hessian -= e * (1.0 - e) / (S * S);
            }
            // A flat all-win player: a pseudo-draw against their strongest opponent
            // gives the otherwise-monotone likelihood a finite maximum.
            if let Some(FlatDegenerate::AllWin { opp, offset }) = flat_degenerate[i] {
                let e = expected((theta[i] - theta[opp] + offset) / S);
                gradient += (0.5 - e) / S;
                hessian -= e * (1.0 - e) / (S * S);
            }
            // hessian < 0 (see `PriorPenalty::grad_hess`) except for a flat prior
            // with no games, whose 0 curvature means nothing to learn — leave them
            // at their seed rather than divide by zero.
            if hessian == 0.0 {
                continue;
            }
            let step = (-gradient / hessian).clamp(-MAX_STEP, MAX_STEP);
            theta[i] += step;
            max_delta = max_delta.max(step.abs());
        }
        if max_delta < CONVERGENCE_TOL {
            converged = true;
            break;
        }
    }
    // Running out of sweeps means the strong concavity argument above no longer
    // holds, and the estimate returned is an arbitrary iterate rather than the
    // maximum — indistinguishable from a converged one to every caller, while it
    // drives pairing, MacMahon groups and the published standings. Purely
    // internal (the objective and the sweep are both ours), so this is a
    // debug-only invariant; release still returns the best iterate it has.
    debug_assert!(
        converged,
        "ELO coordinate ascent did not converge within {MAX_SWEEPS} sweeps"
    );

    // Output keyed by player id (positions align with `players`).
    players
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id, theta[i]))
        .collect()
}

/// The estimate [`estimate_elos`] produced for `id`.
///
/// It returns one entry per player it was handed, and every caller looks players
/// up in the very slice it was given, so a miss means those two lists drifted
/// apart — a bug in this crate rather than anything a save file can cause, hence
/// the debug-only check. Release falls back to the unrated prior mean rather than
/// panicking mid-round; [`compute_standings`](crate::standings::compute_standings)
/// states the same invariant with an `expect`, because a rank has no such
/// fallback.
pub(crate) fn estimate_or_prior(estimates: &HashMap<Uuid, f64>, id: Uuid) -> f64 {
    debug_assert!(
        estimates.contains_key(&id),
        "estimate_elos returns one entry per player, so {id} must have one"
    );
    estimates.get(&id).copied().unwrap_or(UNRATED_PRIOR_MEAN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::RoundExplanation;
    use crate::round::GameRecord;
    use crate::round::{Board, HandicapGame, Outcome, PairingSource};
    use crate::settings::{Ratio, RatioAtLeastOne, UnratedK};
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_TID: AtomicU32 = AtomicU32::new(1);

    fn player(rating: Option<u32>) -> Player {
        let tid = NEXT_TID.fetch_add(1, Ordering::Relaxed);
        Player {
            id: Uuid::new_v4(),
            tournament_id: Some(TournamentId(tid)),
            last_name: "P".into(),
            first_name: String::new(),
            rating,
            pairing_rating: None,
            grade: None,
            fesa_games: None,
            nationality: None,
            club: None,
            eligible: false,
            categories: Vec::new(),
            adjustments: Vec::new(),
        }
    }

    fn decided(number: u32, boards: Vec<Board>) -> Round {
        Round {
            number,
            explanation: RoundExplanation::empty(number),
            boards,
            sitouts: Vec::new(),
            completed: true,
        }
    }

    fn win(a: TournamentId, b: TournamentId) -> Board {
        Board {
            record: GameRecord::Short(Outcome::won(Winner::Player1)),
            ..Board::pending(a, b, 0, PairingSource::Swiss)
        }
    }

    fn handicap_board(
        a: TournamentId,
        b: TournamentId,
        result: Winner,
        handicap: Handicap,
        giver: Winner,
    ) -> Board {
        Board {
            record: GameRecord::Short(Outcome::won(result)),
            handicap: Some(HandicapGame { handicap, giver }),
            ..Board::pending(a, b, 0, PairingSource::Swiss)
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
        let r = decided(
            1,
            vec![win(a.tournament_id.unwrap(), b.tournament_id.unwrap())],
        );
        let elos = estimate_elos(
            &[a.clone(), b.clone()],
            &TournamentSettings::default(),
            &[r],
        );
        assert!(elos[&a.id] > 1500.0, "winner rises");
        assert!(elos[&b.id] < 1500.0, "loser falls");
        // Symmetric priors and opposite results → symmetric shifts.
        assert!(((elos[&a.id] - 1500.0) + (elos[&b.id] - 1500.0)).abs() < 1e-3);
    }

    #[test]
    fn single_game_shift_is_capped_near_k() {
        // An *established* rated 1100 player (FESA K = 32, m = 1) beating a 1600:
        // the first-game move is capped by K = 32 and, since the opponent is much
        // stronger (E ≈ 0.05), lands just under it — matching a plain Elo update.
        // (An established rating avoids the provisional multiplier widening it.)
        let mut winner = player(Some(1100));
        winner.fesa_games = Some(50);
        let loser = player(Some(1600));
        let r = decided(
            1,
            vec![win(
                winner.tournament_id.unwrap(),
                loser.tournament_id.unwrap(),
            )],
        );
        let elos = estimate_elos(
            &[winner.clone(), loser.clone()],
            &TournamentSettings::default(),
            &[r],
        );
        let shift = elos[&winner.id] - 1100.0;
        assert!(
            shift > 25.0 && shift < 32.0,
            "shift was {shift}, expected ~30 (< K=32)"
        );
    }

    #[test]
    fn a_bigger_multiplier_moves_the_estimate_more() {
        let a = player(Some(1500));
        let b = player(Some(1500));
        let r = decided(
            1,
            vec![win(a.tournament_id.unwrap(), b.tournament_id.unwrap())],
        );
        let base = TournamentSettings::default();
        let hot = TournamentSettings::elo_pairing()
            .map_estimator(|e| e.k_multiplier = Ratio::from_percent(400));
        let shift_base =
            estimate_elos(&[a.clone(), b.clone()], &base, std::slice::from_ref(&r))[&a.id] - 1500.0;
        let shift_hot =
            estimate_elos(&[a.clone(), b.clone()], &hot, std::slice::from_ref(&r))[&a.id] - 1500.0;
        assert!(
            shift_hot > shift_base * 2.0,
            "a 4× multiplier should move the estimate much more"
        );
    }

    #[test]
    fn unrated_estimate_swings_far_on_a_big_upset() {
        // An unrated player (wide prior, seeded 600) beating a 1600 should jump
        // hundreds of points — the uncertainty-proportional cap in action.
        let newcomer = player(None);
        let strong = player(Some(1600));
        let r = decided(
            1,
            vec![win(
                newcomer.tournament_id.unwrap(),
                strong.tournament_id.unwrap(),
            )],
        );
        let elos = estimate_elos(
            &[newcomer.clone(), strong.clone()],
            &TournamentSettings::default(),
            &[r],
        );
        assert!(
            elos[&newcomer.id] > 1100.0,
            "unrated upset should swing far, got {}",
            elos[&newcomer.id]
        );
    }

    #[test]
    fn unrated_prior_center_is_configurable() {
        // A no-games unrated player sits exactly at the configured center, not the
        // default 600.
        let unrated = player(None);
        let settings =
            TournamentSettings::elo_pairing().map_estimator(|e| e.unrated_prior_center = 900);
        let elos = estimate_elos(std::slice::from_ref(&unrated), &settings, &[]);
        assert!(
            (elos[&unrated.id] - 900.0).abs() < 1e-6,
            "unrated seeds at the configured center, got {}",
            elos[&unrated.id]
        );
    }

    #[test]
    fn unrated_prior_k_controls_the_drift() {
        // Same upset (unrated beats a 1600), but a larger unrated K is a looser
        // prior, so the estimate moves further. A tiny K barely moves it.
        let newcomer = player(None);
        let strong = player(Some(1600));
        let r = decided(
            1,
            vec![win(
                newcomer.tournament_id.unwrap(),
                strong.tournament_id.unwrap(),
            )],
        );
        let shift = |k: u32| {
            let settings =
                TournamentSettings::elo_pairing().map_estimator(|e| e.unrated_k = UnratedK::new(k));
            estimate_elos(
                &[newcomer.clone(), strong.clone()],
                &settings,
                std::slice::from_ref(&r),
            )[&newcomer.id]
        };
        let tight = shift(20); // a rated-scale K → prior barely budges
        let loose = shift(2000); // very loose → estimate chases the result
        assert!(
            loose > tight + 200.0,
            "a looser unrated prior should drift much further: tight {tight}, loose {loose}"
        );
    }

    #[test]
    fn provisional_ratings_drift_faster_than_established_ones() {
        // Two equally-rated (1500) winners of an identical game, differing only in
        // rating reliability: one established (50 FESA games), one provisional
        // (hand-typed rating, no games). With the default ×2 provisional
        // multiplier, the provisional player's estimate should move further.
        let mut established = player(Some(1500));
        established.fesa_games = Some(50);
        let mut provisional = player(Some(1500));
        provisional.fesa_games = None; // not from FESA → provisional
        let o1 = player(Some(1500));
        let o2 = player(Some(1500));

        let players = vec![
            established.clone(),
            provisional.clone(),
            o1.clone(),
            o2.clone(),
        ];
        let r = decided(
            1,
            vec![
                win(
                    established.tournament_id.unwrap(),
                    o1.tournament_id.unwrap(),
                ),
                win(
                    provisional.tournament_id.unwrap(),
                    o2.tournament_id.unwrap(),
                ),
            ],
        );
        let elos = estimate_elos(&players, &TournamentSettings::default(), &[r]);

        let est_shift = elos[&established.id] - 1500.0;
        let prov_shift = elos[&provisional.id] - 1500.0;
        assert!(est_shift > 0.0 && prov_shift > 0.0);
        assert!(
            prov_shift > est_shift * 1.4,
            "provisional shift {prov_shift} should clearly exceed established {est_shift}"
        );
    }

    #[test]
    fn few_games_is_provisional_but_enough_is_established() {
        // Same rating; the only difference is the FESA game count straddling the
        // reliability threshold. Below it drifts like the provisional case, at/above
        // it drifts like the established case.
        let mut few = player(Some(1500));
        few.fesa_games = Some(PROVISIONAL_GAMES_THRESHOLD - 1);
        let mut enough = player(Some(1500));
        enough.fesa_games = Some(PROVISIONAL_GAMES_THRESHOLD);
        let o1 = player(Some(1500));
        let o2 = player(Some(1500));

        let players = vec![few.clone(), enough.clone(), o1.clone(), o2.clone()];
        let r = decided(
            1,
            vec![
                win(few.tournament_id.unwrap(), o1.tournament_id.unwrap()),
                win(enough.tournament_id.unwrap(), o2.tournament_id.unwrap()),
            ],
        );
        let elos = estimate_elos(&players, &TournamentSettings::default(), &[r]);
        assert!(
            elos[&few.id] - 1500.0 > elos[&enough.id] - 1500.0,
            "a sub-threshold game count should be treated as provisional"
        );
    }

    #[test]
    fn draws_are_scored_as_half_and_leave_equals_level() {
        let a = player(Some(1500));
        let b = player(Some(1500));
        let drawn = Board {
            record: GameRecord::Short(Outcome::Won {
                winner: Winner::Player1,
                drawn: true,
            }),
            ..Board::pending(
                a.tournament_id.unwrap(),
                b.tournament_id.unwrap(),
                0,
                PairingSource::Swiss,
            )
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
    fn grade_number_and_rating_are_inverse_and_handicap_offset_matches_fesa() {
        // Round-trips at a lower bound, a midpoint, and below/above the table.
        for &r in &[1680.0, 1740.0, 2000.0, 300.0, 2400.0] {
            assert!(
                (rating_at_grade(grade_number(r)) - r).abs() < 1e-6,
                "round-trip {r}"
            );
        }
        // Worked FESA example: a 1740 (1 Dan) giver conceding Rook odds (2.1 grades)
        // drops to grade 18.4 → rating 1500, a 240-point effect.
        assert!((handicap_offset(1740, Handicap::Rook) - 240.0).abs() < 1.0);
        // Bigger handicaps are worth more; Sente is almost nothing.
        assert!(handicap_offset(1740, Handicap::SixPiece) > handicap_offset(1740, Handicap::Rook));
        assert!(handicap_offset(1740, Handicap::Sente) < 40.0);
    }

    #[test]
    fn handicap_games_now_update_estimates() {
        // A 1800 giver beats a 1500 receiver at Rook odds — no longer excluded.
        let giver = player(Some(1800));
        let receiver = player(Some(1500));
        let board = handicap_board(
            giver.tournament_id.unwrap(),
            receiver.tournament_id.unwrap(),
            Winner::Player1,
            Handicap::Rook,
            Winner::Player1,
        );
        let elos = estimate_elos(
            &[giver.clone(), receiver.clone()],
            &TournamentSettings::default(),
            &[decided(1, vec![board])],
        );
        assert!(elos[&giver.id] > 1800.0, "the handicap game is rated now");
        assert!(elos[&receiver.id] < 1500.0);
    }

    #[test]
    fn conceding_odds_shrinks_the_gap_so_a_win_counts_for_more() {
        // A 1800 favourite beating a 1500: winning an even game is nearly expected
        // (small gain), but winning *after giving Rook odds* — which shrinks the
        // effective gap — is more of an achievement, so it moves the estimate more.
        let giver = player(Some(1800));
        let receiver = player(Some(1500));
        let players = vec![giver.clone(), receiver.clone()];
        let settings = TournamentSettings::default();

        let even = estimate_elos(
            &players,
            &settings,
            &[decided(
                1,
                vec![win(
                    giver.tournament_id.unwrap(),
                    receiver.tournament_id.unwrap(),
                )],
            )],
        );
        let handi = estimate_elos(
            &players,
            &settings,
            &[decided(
                1,
                vec![handicap_board(
                    giver.tournament_id.unwrap(),
                    receiver.tournament_id.unwrap(),
                    Winner::Player1,
                    Handicap::Rook,
                    Winner::Player1,
                )],
            )],
        );
        assert!(
            handi[&giver.id] > even[&giver.id],
            "winning after giving odds should raise the estimate more than an even win"
        );
        // Symmetrically, the receiver — expected to do better with odds — is
        // punished more for losing them.
        assert!(handi[&receiver.id] < even[&receiver.id]);
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
                .map(|i| {
                    decided(
                        i as u32 + 1,
                        vec![win(
                            hero.tournament_id.unwrap(),
                            opps[i].tournament_id.unwrap(),
                        )],
                    )
                })
                .collect();
            let elos = estimate_elos(&all, &TournamentSettings::default(), &rounds);
            let now = elos[&hero.id];
            let gain = now - prev;
            assert!(gain > 0.0, "estimate should keep rising");
            assert!(
                gain < prev_gain,
                "each win should move it less than the last"
            );
            prev = now;
            prev_gain = gain;
        }
    }

    /// A Laplace prior for *every* player category with the same upward-looseness
    /// `r` (so tests that don't care about the split can pass one number and still
    /// exercise the Laplace mechanics on whatever category their fixture uses).
    fn laplace(looseness_percent: u32) -> TournamentSettings {
        TournamentSettings::elo_pairing().map_estimator(|e| {
            e.prior_shape_established = EloPriorShape::Laplace;
            e.prior_shape_provisional = EloPriorShape::Laplace;
            e.prior_shape_unrated = EloPriorShape::Laplace;
            e.upward_looseness_established = RatioAtLeastOne::from_percent(looseness_percent);
            e.upward_looseness_provisional = RatioAtLeastOne::from_percent(looseness_percent);
            e.upward_looseness_unrated = RatioAtLeastOne::from_percent(looseness_percent);
        })
    }

    /// `n` fresh equal-rated (1500) opponents.
    fn equals(n: usize) -> Vec<Player> {
        (0..n).map(|_| player(Some(1500))).collect()
    }

    #[test]
    fn laplace_no_games_sits_at_registration_even_when_asymmetric() {
        // The Huber core is anchored at the mean, so switching to a (strongly
        // asymmetric) Laplace prior must not budge a player who has played nothing
        // — rated or unrated. This is the invariant the kink-smoothing preserves.
        let rated = player(Some(1800));
        let unrated = player(None);
        let elos = estimate_elos(&[rated.clone(), unrated.clone()], &laplace(300), &[]);
        assert!((elos[&rated.id] - 1800.0).abs() < 1e-6);
        assert!((elos[&unrated.id] - UNRATED_PRIOR_MEAN).abs() < 1e-6);
    }

    #[test]
    fn laplace_upward_revision_is_easier_than_downward_when_asymmetric() {
        // Mirror-image evidence past the Laplace dead zone: `riser` beats six
        // equal-rated opponents (pushed up), `faller` loses to six (pushed down) by
        // a symmetric amount. A symmetric Laplace prior moves them equally;
        // loosening only the upward arm makes the rise exceed the fall — the
        // "under-rating is more common" tilt.
        let riser = player(Some(1500));
        let faller = player(Some(1500));
        let r_opps = equals(6);
        let f_opps = equals(6);
        let mut all = vec![riser.clone(), faller.clone()];
        all.extend(r_opps.iter().cloned());
        all.extend(f_opps.iter().cloned());
        let rounds: Vec<Round> = (0..6)
            .map(|i| {
                decided(
                    i as u32 + 1,
                    vec![
                        win(
                            riser.tournament_id.unwrap(),
                            r_opps[i].tournament_id.unwrap(),
                        ),
                        win(
                            f_opps[i].tournament_id.unwrap(),
                            faller.tournament_id.unwrap(),
                        ),
                    ],
                )
            })
            .collect();

        let shift = |r_pct: u32| {
            let elos = estimate_elos(&all, &laplace(r_pct), &rounds);
            (elos[&riser.id] - 1500.0, 1500.0 - elos[&faller.id])
        };

        // Symmetric prior (r = 1): the mirrored setup rises and falls equally, and
        // the streak clears the dead zone so the movement is substantial.
        let (up_sym, down_sym) = shift(100);
        assert!(
            up_sym > 10.0,
            "the streak should clear the dead zone: {up_sym}"
        );
        assert!(
            (up_sym - down_sym).abs() < 1e-2,
            "symmetric prior should be balanced: up {up_sym}, down {down_sym}"
        );

        // Looser upward (r = 3): the riser climbs further than under the symmetric
        // prior, while the downward arm — untouched by r — never falls more.
        let (up_asym, down_asym) = shift(300);
        assert!(
            up_asym > up_sym + 1.0,
            "loosening the upward arm should raise more: sym {up_sym}, asym {up_asym}"
        );
        assert!(
            down_asym <= down_sym + 1e-2,
            "the downward arm is not loosened: sym {down_sym}, asym {down_asym}"
        );
        assert!(
            up_asym > down_asym + 1.0,
            "an upward revision should clear on less evidence than a downward one: \
             up {up_asym}, down {down_asym}"
        );
    }

    #[test]
    fn laplace_swings_further_than_gaussian_on_a_sustained_upset_streak() {
        // An established 1200 repeatedly beats much-stronger 1800s. Both priors
        // climb, but the fatter-tailed Laplace (constant restoring force) yields to
        // the sustained evidence more than the Gaussian, whose pull stiffens with
        // distance — the motivating behaviour of the fatter tail.
        let mut hero = player(Some(1200));
        hero.fesa_games = Some(50); // established, so no provisional widening
        let opps: Vec<Player> = (0..5).map(|_| player(Some(1800))).collect();
        let mut all = vec![hero.clone()];
        all.extend(opps.iter().cloned());
        let rounds: Vec<Round> = opps
            .iter()
            .enumerate()
            .map(|(i, o)| {
                decided(
                    i as u32 + 1,
                    vec![win(hero.tournament_id.unwrap(), o.tournament_id.unwrap())],
                )
            })
            .collect();

        let est = |settings: &TournamentSettings| estimate_elos(&all, settings, &rounds)[&hero.id];
        let gauss = est(&TournamentSettings::default());
        let lap = est(&laplace(100)); // symmetric, so the gap is purely the tail
        assert!(gauss > 1200.0 && lap > 1200.0, "both priors climb on wins");
        assert!(
            lap > gauss + 10.0,
            "fatter tails should swing further on a sustained streak: \
             gaussian {gauss}, laplace {lap}"
        );
    }

    #[test]
    fn laplace_confined_to_unrated_leaves_rated_players_on_the_gaussian() {
        // The per-category shape split is the whole point: a fat tail on the
        // *unrated* prior lets a newcomer climb further on a sustained upset, while a
        // rated player — whose shape stays Gaussian — is bit-for-bit unaffected. That
        // invariance is what makes "fat tail for unrated only" safe for the field.
        let unrated_only_laplace = TournamentSettings::elo_pairing()
            .map_estimator(|e| e.prior_shape_unrated = EloPriorShape::Laplace);

        // (a) An unrated newcomer beating five 1800s climbs further under the
        //     unrated-Laplace prior than under the all-Gaussian default — proof the
        //     unrated category actually picks up the Laplace shape.
        let newcomer = player(None);
        let opps_u: Vec<Player> = (0..5).map(|_| player(Some(1800))).collect();
        let mut all_u = vec![newcomer.clone()];
        all_u.extend(opps_u.iter().cloned());
        let rounds_u: Vec<Round> = opps_u
            .iter()
            .enumerate()
            .map(|(i, o)| {
                decided(
                    i as u32 + 1,
                    vec![win(
                        newcomer.tournament_id.unwrap(),
                        o.tournament_id.unwrap(),
                    )],
                )
            })
            .collect();
        let climb = |s: &TournamentSettings| estimate_elos(&all_u, s, &rounds_u)[&newcomer.id];
        let gauss_u = climb(&TournamentSettings::default());
        let lap_u = climb(&unrated_only_laplace);
        assert!(
            lap_u > gauss_u + 10.0,
            "the unrated prior should pick up the Laplace shape and swing further: \
             gaussian {gauss_u}, unrated-laplace {lap_u}"
        );

        // (b) An established 1200 in the same upset situation is untouched: no unrated
        //     player is present, so flipping only the unrated shape must leave every
        //     estimate identical.
        let mut hero = player(Some(1200));
        hero.fesa_games = Some(50); // established → Gaussian under both configs
        let opps_e: Vec<Player> = (0..5).map(|_| player(Some(1800))).collect();
        let mut all_e = vec![hero.clone()];
        all_e.extend(opps_e.iter().cloned());
        let rounds_e: Vec<Round> = opps_e
            .iter()
            .enumerate()
            .map(|(i, o)| {
                decided(
                    i as u32 + 1,
                    vec![win(hero.tournament_id.unwrap(), o.tournament_id.unwrap())],
                )
            })
            .collect();
        let est_e = |s: &TournamentSettings| estimate_elos(&all_e, s, &rounds_e)[&hero.id];
        let gauss_e = est_e(&TournamentSettings::default());
        let lap_e = est_e(&unrated_only_laplace);
        assert!(
            (gauss_e - lap_e).abs() < 1e-9,
            "a rated player's estimate must not move when only the unrated shape is \
             Laplace: gaussian {gauss_e}, unrated-laplace {lap_e}"
        );
    }

    /// A flat (improper) prior on the unrated slot — the `turnering.py` performance
    /// rating — leaving the rated categories on their default Gaussian.
    fn flat_unrated() -> TournamentSettings {
        TournamentSettings::elo_pairing()
            .map_estimator(|e| e.prior_shape_unrated = EloPriorShape::Flat)
    }

    #[test]
    fn flat_unrated_reads_off_the_field_and_ignores_the_prior_center() {
        // turnering.py-style: an unrated player's estimate is the maximum-likelihood
        // performance rating over their games, with no pull toward the unrated
        // centre. So moving the centre must not move a flat estimate — the tell of an
        // improper prior — whereas the centre-anchored Gaussian does move.
        let newcomer = player(None);
        let mut a = player(Some(1500));
        a.fesa_games = Some(50);
        let mut b = player(Some(1500));
        b.fesa_games = Some(50);
        let all = vec![newcomer.clone(), a.clone(), b.clone()];
        let rounds = vec![
            decided(
                1,
                vec![win(
                    newcomer.tournament_id.unwrap(),
                    a.tournament_id.unwrap(),
                )],
            ), // newcomer beats A
            decided(
                2,
                vec![win(
                    b.tournament_id.unwrap(),
                    newcomer.tournament_id.unwrap(),
                )],
            ), // B beats newcomer
        ];
        let est = |shape, center: u32| {
            let settings = TournamentSettings::elo_pairing().map_estimator(|e| {
                e.prior_shape_unrated = shape;
                e.unrated_prior_center = center;
            });
            estimate_elos(&all, &settings, &rounds)[&newcomer.id]
        };
        // Flat: identical regardless of the centre.
        let flat_low = est(EloPriorShape::Flat, 600);
        let flat_high = est(EloPriorShape::Flat, 1000);
        // Only solver-tolerance residue separates them (the two runs differ solely in
        // the newcomer's seed, which a flat prior never anchors) — orders of magnitude
        // below the Gaussian's centre-driven shift asserted below.
        assert!(
            (flat_low - flat_high).abs() < 1e-2,
            "a flat (improper) prior must ignore the centre: {flat_low} vs {flat_high}"
        );
        // A 1 win / 1 loss record vs a 1500 field → performance sits right at 1500.
        assert!(
            (flat_low - 1500.0).abs() < 60.0,
            "a 1-1 record vs 1500s should rate near 1500, got {flat_low}"
        );
        // Gaussian: the centre pulls the estimate, so the two centres disagree.
        let g_low = est(EloPriorShape::Gaussian, 600);
        let g_high = est(EloPriorShape::Gaussian, 1000);
        assert!(
            (g_low - g_high).abs() > 20.0,
            "a Gaussian prior should track its centre: {g_low} vs {g_high}"
        );
    }

    #[test]
    fn flat_unrated_all_losses_floors_to_one() {
        // A flat prior can't bound an all-loss likelihood; turnering.py floors it.
        let newcomer = player(None);
        let opps: Vec<Player> = (0..5).map(|_| player(Some(1500))).collect();
        let mut all = vec![newcomer.clone()];
        all.extend(opps.iter().cloned());
        let rounds: Vec<Round> = opps
            .iter()
            .enumerate()
            .map(|(i, o)| {
                decided(
                    i as u32 + 1,
                    vec![win(
                        o.tournament_id.unwrap(),
                        newcomer.tournament_id.unwrap(),
                    )],
                )
            })
            .collect();
        let est = estimate_elos(&all, &flat_unrated(), &rounds)[&newcomer.id];
        assert!(
            (est - FLAT_ALL_LOSS_FLOOR).abs() < 1e-6,
            "an all-loss flat player floors to {FLAT_ALL_LOSS_FLOOR}, got {est}"
        );
    }

    #[test]
    fn flat_unrated_all_wins_is_finite_and_above_the_field() {
        // The pseudo-draw against the strongest opponent bounds the otherwise-
        // monotone all-win likelihood, so the estimate is a finite performance
        // rating above the field — not the unbounded run-away a bare flat prior gives.
        let newcomer = player(None);
        let opps: Vec<Player> = (0..5).map(|_| player(Some(1500))).collect();
        let mut all = vec![newcomer.clone()];
        all.extend(opps.iter().cloned());
        let rounds: Vec<Round> = opps
            .iter()
            .enumerate()
            .map(|(i, o)| {
                decided(
                    i as u32 + 1,
                    vec![win(
                        newcomer.tournament_id.unwrap(),
                        o.tournament_id.unwrap(),
                    )],
                )
            })
            .collect();
        let est = estimate_elos(&all, &flat_unrated(), &rounds)[&newcomer.id];
        assert!(
            est.is_finite(),
            "all-win flat estimate must be finite, got {est}"
        );
        assert!(
            est > 1700.0,
            "an all-win newcomer should rate well above the 1500 field, got {est}"
        );
        assert!(
            est < 2600.0,
            "…but the pseudo-draw keeps it from running away, got {est}"
        );
    }

    #[test]
    fn k_multiplier_zero_estimates_unrated_only_and_pins_rated_players() {
        // "Apply estimates to unrated players only": a rated player stays exactly at
        // their registration rating no matter what they score, while an unrated
        // newcomer is still estimated against that fixed field.
        let mut rated = player(Some(1600));
        rated.fesa_games = Some(50);
        let newcomer = player(None);
        let strong = player(Some(2000));
        let all = vec![rated.clone(), newcomer.clone(), strong.clone()];
        // The rated 1600 loses to the 2000; the newcomer beats the 2000.
        let rounds = vec![
            decided(
                1,
                vec![win(
                    strong.tournament_id.unwrap(),
                    rated.tournament_id.unwrap(),
                )],
            ),
            decided(
                2,
                vec![win(
                    newcomer.tournament_id.unwrap(),
                    strong.tournament_id.unwrap(),
                )],
            ),
        ];
        let settings = TournamentSettings::elo_pairing()
            .map_estimator(|e| e.k_multiplier = Ratio::from_percent(0));
        let est = estimate_elos(&all, &settings, &rounds);
        assert!(
            (est[&rated.id] - 1600.0).abs() < 1e-9,
            "a rated player is pinned to their rating at k=0, got {}",
            est[&rated.id]
        );
        assert!(
            (est[&strong.id] - 2000.0).abs() < 1e-9,
            "the rated 2000 is pinned too, got {}",
            est[&strong.id]
        );
        // The unrated newcomer is still moved by their games (beat a fixed 2000).
        assert!(
            est[&newcomer.id] > UNRATED_PRIOR_MEAN + 100.0,
            "the unrated newcomer should still be estimated upward, got {}",
            est[&newcomer.id]
        );
    }

    #[test]
    fn flat_unrated_with_no_games_stays_at_the_seed() {
        // Zero curvature and no games: the sweep skips them and they sit at the seed
        // (the unrated centre), never a NaN from 0/0.
        let newcomer = player(None);
        let settings = TournamentSettings::elo_pairing().map_estimator(|e| {
            e.prior_shape_unrated = EloPriorShape::Flat;
            e.unrated_prior_center = 900;
        });
        let est = estimate_elos(std::slice::from_ref(&newcomer), &settings, &[])[&newcomer.id];
        assert!(
            (est - 900.0).abs() < 1e-6,
            "a no-games flat player stays at the seed, got {est}"
        );
    }

    #[test]
    fn gaussian_prior_can_be_made_asymmetric_too() {
        // The same split works for the default Gaussian (a two-piece normal, no
        // kink to smooth): widening only the upward arm lets an unrated newcomer
        // climb further on an upset, while a no-games player still sits exactly at
        // the prior mean.
        let newcomer = player(None);
        let strong = player(Some(1600));
        let r = decided(
            1,
            vec![win(
                newcomer.tournament_id.unwrap(),
                strong.tournament_id.unwrap(),
            )],
        );

        // Shape stays Gaussian (default); only the unrated upward looseness moves.
        let climb = |unrated_pct: u32| {
            let settings = TournamentSettings::elo_pairing().map_estimator(|e| {
                e.upward_looseness_unrated = RatioAtLeastOne::from_percent(unrated_pct)
            });
            estimate_elos(
                &[newcomer.clone(), strong.clone()],
                &settings,
                std::slice::from_ref(&r),
            )[&newcomer.id]
        };
        let sym = climb(100); // r = 1 → the plain symmetric Gaussian
        let asym = climb(300); // looser upward arm
        assert!(
            asym > sym + 20.0,
            "a looser upward Gaussian arm should climb further: sym {sym}, asym {asym}"
        );

        // No games → still exactly at the unrated center, asymmetry or not (the
        // two-piece normal's mode is unchanged).
        let settings = TournamentSettings::elo_pairing()
            .map_estimator(|e| e.upward_looseness_unrated = RatioAtLeastOne::from_percent(300));
        let at_rest = estimate_elos(std::slice::from_ref(&newcomer), &settings, &[])[&newcomer.id];
        assert!(
            (at_rest - UNRATED_PRIOR_MEAN).abs() < 1e-6,
            "a no-games unrated player still sits at the center, got {at_rest}"
        );
    }

    /// A Laplace prior (all categories) with a distinct upward-looseness per player
    /// category.
    fn laplace3(established: u32, provisional: u32, unrated: u32) -> TournamentSettings {
        TournamentSettings::elo_pairing().map_estimator(|e| {
            e.prior_shape_established = EloPriorShape::Laplace;
            e.prior_shape_provisional = EloPriorShape::Laplace;
            e.prior_shape_unrated = EloPriorShape::Laplace;
            e.upward_looseness_established = RatioAtLeastOne::from_percent(established);
            e.upward_looseness_provisional = RatioAtLeastOne::from_percent(provisional);
            e.upward_looseness_unrated = RatioAtLeastOne::from_percent(unrated);
        })
    }

    #[test]
    fn each_player_category_reads_only_its_own_upward_looseness() {
        // An *established* hero beats ten opponents. Its upward shift responds to
        // the established looseness knob and is completely unmoved by the
        // provisional/unrated ones — so the three knobs are applied per category,
        // not globally.
        let mut hero = player(Some(1500));
        hero.fesa_games = Some(50); // established
        let opps = equals(10);
        let mut all = vec![hero.clone()];
        all.extend(opps.iter().cloned());
        let rounds: Vec<Round> = (0..10)
            .map(|i| {
                decided(
                    i as u32 + 1,
                    vec![win(
                        hero.tournament_id.unwrap(),
                        opps[i].tournament_id.unwrap(),
                    )],
                )
            })
            .collect();

        let shift = |s: &TournamentSettings| estimate_elos(&all, s, &rounds)[&hero.id] - 1500.0;

        let base = shift(&laplace3(100, 100, 100));
        assert!(base > 10.0, "the streak should clear the dead zone: {base}");

        // Loosening only the established knob raises the established hero further.
        let est_loose = shift(&laplace3(300, 100, 100));
        assert!(
            est_loose > base + 5.0,
            "an established hero should read its own knob: base {base}, loosened {est_loose}"
        );

        // Loosening the *other* categories' knobs leaves it where the symmetric
        // prior put it (the hero is neither provisional nor unrated). Any residual
        // is solver-convergence noise, orders of magnitude below the established
        // knob's ~hundreds-of-points effect and below the sweep tolerance.
        let others_loose = shift(&laplace3(100, 300, 300));
        assert!(
            (others_loose - base).abs() < CONVERGENCE_TOL,
            "an established hero must ignore the provisional/unrated knobs: \
             base {base}, others {others_loose}"
        );
    }
}
