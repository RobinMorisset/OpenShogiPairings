//! Deterministic micro-benchmark of `min_weight_perfect_matching`, isolated
//! from the surrounding application. Run with:
//!
//! ```sh
//! cargo run --release -p integer-blossom --example bench
//! ```
//!
//! Two instance families bracket the costs the OpenShogiPairings engine feeds
//! the solver:
//!
//! * `lex` — lexicographic rule ladders: a coarse band difference scaled by a
//!   huge multiplier, plus a smooth ELO-shaped tail. This is the typical shape
//!   outside pure-ELO pairing, and the reason the engine usually instantiates
//!   the solver at `i128`.
//! * `elo` — pure squared-ELO-difference costs, the smooth surface where the
//!   greedy tight-edge seed pre-matches most of the field.
//!
//! Everything (instances and iteration counts) is deterministic, so runs are
//! comparable across code changes on the same machine. Per-(family, n), a few
//! pre-generated instances are cycled so the per-thread solver pool is
//! exercised the way the application exercises it, while generation cost stays
//! outside the timed region.

use integer_blossom::min_weight_perfect_matching;
use std::time::Instant;

fn xorshift(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// Pure squared-ELO-difference costs: ELOs spread over 500..2900.
fn elo_costs(n: usize, seed: u64) -> Vec<i128> {
    let mut s = seed | 1;
    let elos: Vec<i128> = (0..n)
        .map(|_| 500 + (xorshift(&mut s) % 2400) as i128)
        .collect();
    let mut cost = vec![0i128; n * n];
    for i in 0..n {
        for j in (i + 1)..n {
            let d = elos[i] - elos[j];
            cost[i * n + j] = d * d;
        }
    }
    cost
}

/// Lexicographic-ladder costs: band difference times a 1e18 multiplier
/// (dominant rule), squared ELO difference as the tail (minor rule). Bands
/// mimic score groups after most of a 9-round event: ~10 distinct levels.
fn lex_costs(n: usize, seed: u64) -> Vec<i128> {
    let mut s = seed | 1;
    let elos: Vec<i128> = (0..n)
        .map(|_| 500 + (xorshift(&mut s) % 2400) as i128)
        .collect();
    let bands: Vec<i128> = (0..n).map(|_| (xorshift(&mut s) % 10) as i128).collect();
    const M: i128 = 1_000_000_000_000_000_000; // 1e18: dwarfs any tail sum
    let mut cost = vec![0i128; n * n];
    for i in 0..n {
        for j in (i + 1)..n {
            let d = elos[i] - elos[j];
            cost[i * n + j] = (bands[i] - bands[j]).abs() * M + d * d;
        }
    }
    cost
}

/// An instance generator: `(n, seed) -> cost matrix`.
type CostGen = fn(usize, u64) -> Vec<i128>;

fn main() {
    const INSTANCES: usize = 3;
    let families: [(&str, CostGen); 2] = [("lex", lex_costs), ("elo", elo_costs)];
    for (name, gen) in families {
        for n in [200usize, 500, 1000] {
            let instances: Vec<Vec<i128>> = (0..INSTANCES)
                .map(|k| gen(n, 0x9E3779B97F4A7C15 ^ (k as u64 + 1)))
                .collect();
            // Warm up the pool (and the branch predictor) once per size.
            let mut sink = 0usize;
            sink += min_weight_perfect_matching(&instances[0], n)[0];
            // Discard the warm-up's stats so the breakdown covers only the
            // timed iterations.
            #[cfg(feature = "stats")]
            let _ = integer_blossom::stats::take();
            // Iteration count fixed per n so before/after runs do identical work.
            let iters = match n {
                200 => 60,
                500 => 15,
                _ => 4,
            };
            let t0 = Instant::now();
            for it in 0..iters {
                let mate = min_weight_perfect_matching(&instances[it % INSTANCES], n);
                sink += mate[0];
            }
            let ms = t0.elapsed().as_secs_f64() * 1000.0 / iters as f64;
            println!("{name}  n={n:<5} {ms:9.2} ms/solve   (iters={iters}, sink={sink})");
            print_stats(t0.elapsed());
        }
    }
}

/// With `--features stats`, print the cost-shape breakdown accumulated over the
/// timed iterations (the warm-up solve's stats were discarded). Region docs —
/// including which buckets overlap — live on `integer_blossom::stats::Stats`.
#[cfg(feature = "stats")]
fn print_stats(total: std::time::Duration) {
    let s = integer_blossom::stats::take();
    let pct = |d: std::time::Duration| 100.0 * d.as_secs_f64() / total.as_secs_f64();
    let scan_pure = s.t_scan - s.t_ofe_scan;
    println!(
        "      counts: {} solves, {:.1} phases/solve, {:.1} adjustments/phase, \
         greedy {:.1}%, {} augments, {} blossoms (+), {} (-), {} set_slack",
        s.solves,
        s.phases as f64 / s.solves as f64,
        s.adjustments as f64 / s.phases.max(1) as f64,
        100.0 * s.greedy_pairs as f64 / (s.greedy_pairs + s.augments).max(1) as f64,
        s.augments,
        s.blossoms_formed,
        s.blossoms_expanded,
        s.set_slack_calls,
    );
    println!(
        "      time:   init {:4.1}%  phase-init {:4.1}%  scan {:4.1}%  dual {:4.1}%   \
         [ofe-scan {:4.1}%  augment {:4.1}%  blossom+ {:4.1}%  blossom- {:4.1}%  set_slack {:4.1}%]",
        pct(s.t_init),
        pct(s.t_phase_init),
        pct(scan_pure),
        pct(s.t_dual),
        pct(s.t_ofe_scan),
        pct(s.t_augment),
        pct(s.t_add_blossom),
        pct(s.t_expand),
        pct(s.t_set_slack),
    );
}

#[cfg(not(feature = "stats"))]
fn print_stats(_total: std::time::Duration) {}
