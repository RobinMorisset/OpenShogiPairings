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
use crate::round::{Handicap, Round, Winner};
use crate::settings::TournamentSettings;

/// ELO scale factor `s = 400 / ln 10`: a 400-point gap is 10:1 odds.
const S: f64 = 173.717_792_761_565_8;

/// Prior mean for an unrated player — the midpoint of the assumed `[1, 1200]`
/// strength range.
pub const UNRATED_PRIOR_MEAN: f64 = 600.0;

/// Prior standard deviation for an unrated player — the std of a uniform on
/// `[1, 1200]` is `1199/√12 ≈ 346`, rounded to 350.
const UNRATED_PRIOR_STD: f64 = 350.0;

/// A rated player with at least this many FESA games is treated as reliably
/// rated; fewer (or a rating not from the FESA list) makes it *provisional*, and
/// the referee's provisional multiplier widens its prior.
pub const PROVISIONAL_GAMES_THRESHOLD: u32 = 18;

/// Solver limits: coordinate-ascent sweeps and the per-sweep convergence
/// tolerance (in ELO points). The objective is strongly concave, so this
/// converges quickly and deterministically.
const MAX_SWEEPS: usize = 200;
const CONVERGENCE_TOL: f64 = 1e-4;

/// A generous per-update step clamp (ELO points) guarding against a Newton
/// overshoot when a wide-prior player is far from the optimum; repeated sweeps
/// then close the remaining distance.
const MAX_STEP: f64 = 800.0;

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

/// A player's Gaussian prior `(mean, standard deviation)` on the ELO scale.
///
/// A rated player is centered on their registration rating with a width derived
/// from `K = m · K_FESA(rating)` via `σ₀ = √(K · s)` (so `K` is literally their
/// first-game K factor). A **provisionally**-rated player (see
/// [`is_reliably_rated`]) has that `K` further multiplied by `provisional_mult`,
/// widening the prior so their estimate drifts faster. An unrated player gets the
/// wide `N(600, 350²)` prior.
fn prior(player: &Player, k_multiplier: f64, provisional_mult: f64) -> (f64, f64) {
    match player.rating {
        Some(rating) => {
            // Multipliers are clamped ≥ 1%/100% by settings normalization, so K > 0.
            let reliability = if is_reliably_rated(player) {
                1.0
            } else {
                provisional_mult
            };
            let k = k_multiplier * fesa_k(rating) * reliability;
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
/// rounds, using the FESA-K × multiplier prior from `settings`.
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
    let provisional = settings.elo_provisional_multiplier();
    let n = players.len();

    let index: HashMap<Uuid, usize> = players.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    let mut theta = vec![0.0_f64; n];
    let mut mean = vec![0.0_f64; n];
    let mut precision = vec![0.0_f64; n]; // 1/σ₀² — the prior's weight
    for (i, p) in players.iter().enumerate() {
        let (mu0, sigma0) = prior(p, m, provisional);
        mean[i] = mu0;
        theta[i] = mu0; // seed at the prior mean
        precision[i] = 1.0 / (sigma0 * sigma0);
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
            // straight off `result` (and a draw as ½), not the effective winner.
            let score_a = if board.drawn {
                0.5
            } else {
                match board.result {
                    Some(Winner::Player1) => 1.0,
                    Some(Winner::Player2) => 0.0,
                    None => continue, // unplayed
                }
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

    // Coordinate ascent: sweep players, each a 1-D Newton step on the concave
    // penalised log-likelihood. Strong concavity (every player has a finite prior
    // precision) guarantees a unique maximum and convergence.
    for _ in 0..MAX_SWEEPS {
        let mut max_delta = 0.0_f64;
        for i in 0..n {
            // Prior term first, then each game's likelihood contribution.
            let mut gradient = -(theta[i] - mean[i]) * precision[i];
            let mut hessian = -precision[i];
            for &(j, score, offset) in &games[i] {
                let e = expected((theta[i] - theta[j] + offset) / S);
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
    use crate::round::{Board, HandicapGame, PairingSource};

    fn player(rating: Option<u32>) -> Player {
        Player {
            id: Uuid::new_v4(),
            tournament_id: None,
            last_name: "P".into(),
            first_name: String::new(),
            rating,
            grade: None,
            fesa_games: None,
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

    fn handicap_board(
        a: Uuid,
        b: Uuid,
        result: Winner,
        handicap: Handicap,
        giver: Winner,
    ) -> Board {
        Board {
            result: Some(result),
            handicap: Some(HandicapGame { handicap, giver }),
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
        let r = decided(1, vec![win(winner.id, loser.id)]);
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
        let r = decided(1, vec![win(a.id, b.id)]);
        let base = TournamentSettings::default();
        let hot = TournamentSettings {
            elo_k_multiplier_percent: 400,
            ..Default::default()
        };
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
        let r = decided(1, vec![win(newcomer.id, strong.id)]);
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
            vec![win(established.id, o1.id), win(provisional.id, o2.id)],
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
        let r = decided(1, vec![win(few.id, o1.id), win(enough.id, o2.id)]);
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
            giver.id,
            receiver.id,
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
            &[decided(1, vec![win(giver.id, receiver.id)])],
        );
        let handi = estimate_elos(
            &players,
            &settings,
            &[decided(
                1,
                vec![handicap_board(
                    giver.id,
                    receiver.id,
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
                .map(|i| decided(i as u32 + 1, vec![win(hero.id, opps[i].id)]))
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
}
