//! Replay benchmark for `min_weight_perfect_matching` over **captured real
//! instances** — the cost matrices the OpenShogiPairings engine actually
//! produces, dumped by osp-core's `OSP_MATCHING_DUMP` hook. Synthetic instance
//! families were deliberately removed: measured against captures they
//! mispredicted the real cost shape badly (wrong dominant bucket, ~6× off on
//! Swiss wall time), so captures are the only supported diet.
//!
//! ```sh
//! # capture (one .ospm file per solve; single thread for canonical numbering)
//! RAYON_NUM_THREADS=1 OSP_MATCHING_DUMP=scratch/cap_swiss \
//!   osp-sim --results scratch/fake_1000.txt --runs 1 --seed 0 > /dev/null
//! # replay (add --features stats for the region/counter breakdown)
//! cargo run --release -p integer-blossom --example bench -- scratch/cap_swiss
//! ```
//!
//! Use one capture directory per configuration. Timings are reported per
//! (n, weight width), where the width mirrors osp-core's adaptive narrowing —
//! so the report also shows which integer width real instances exercise.

use integer_blossom::min_weight_perfect_matching;
use std::time::Instant;

/// One captured instance: `b"OSPM1"`, `n` as `u64` LE, then `n*n` `i128` LE
/// values (the format written by osp-core's `OSP_MATCHING_DUMP` hook).
fn read_instance(path: &std::path::Path) -> (usize, Vec<i128>) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
    assert!(
        bytes.len() >= 13 && &bytes[..5] == b"OSPM1",
        "{path:?}: not an OSPM1 capture"
    );
    let n = u64::from_le_bytes(bytes[5..13].try_into().unwrap()) as usize;
    assert_eq!(bytes.len(), 13 + n * n * 16, "{path:?}: truncated");
    let cost = bytes[13..]
        .chunks_exact(16)
        .map(|c| i128::from_le_bytes(c.try_into().unwrap()))
        .collect();
    (n, cost)
}

/// Mirror of osp-core's `solve_matching` width selection, so replay exercises
/// the same solver monomorphization osp-sim would (and reports which width
/// real instances actually use). Returns (`mate[0]`, width name).
fn solve_adaptive(cost: &[i128], n: usize) -> (usize, &'static str) {
    let max = cost.iter().copied().max().unwrap_or(0);
    if max <= i32::MAX as i128 / 16 {
        let narrow: Vec<i32> = cost.iter().map(|&c| c as i32).collect();
        (min_weight_perfect_matching(&narrow, n)[0], "i32")
    } else if max <= i64::MAX as i128 / 16 {
        let narrow: Vec<i64> = cost.iter().map(|&c| c as i64).collect();
        (min_weight_perfect_matching(&narrow, n)[0], "i64")
    } else {
        (min_weight_perfect_matching(cost, n)[0], "i128")
    }
}

/// Replay a directory of captured instances: a warm-up pass, then `REPS` timed
/// passes, reported per (n, weight width).
fn replay(dir: &str) {
    const REPS: usize = 3;
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {dir}: {e}"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "ospm"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no .ospm captures in {dir}");
    let instances: Vec<(usize, Vec<i128>)> = paths.iter().map(|p| read_instance(p)).collect();

    // Warm up the pool (and the branch predictor); discard the warm-up's
    // stats so the breakdown covers only the timed passes.
    let mut sink = 0usize;
    for (n, cost) in &instances {
        sink += solve_adaptive(cost, *n).0;
    }
    #[cfg(feature = "stats")]
    let _ = integer_blossom::stats::take();

    // (n, width) -> (solve count, total seconds)
    let mut groups: std::collections::BTreeMap<(usize, &'static str), (u64, f64)> =
        std::collections::BTreeMap::new();
    let t0 = Instant::now();
    for _ in 0..REPS {
        for (n, cost) in &instances {
            let t = Instant::now();
            let (m, width) = solve_adaptive(cost, *n);
            let e = groups.entry((*n, width)).or_insert((0, 0.0));
            e.0 += 1;
            e.1 += t.elapsed().as_secs_f64();
            sink += m;
        }
    }
    let total = t0.elapsed();
    println!(
        "replay {dir}: {} instances x {REPS} passes (sink={sink})",
        instances.len()
    );
    for ((n, width), (count, secs)) in &groups {
        println!(
            "  n={n:<5} {width:<5} {:9.2} ms/solve   (solves={count})",
            1000.0 * secs / *count as f64
        );
    }
    print_stats(total);
}

fn main() {
    // Sole argument: a capture directory (a leading `--replay` is tolerated
    // for compatibility with older invocations).
    let dir = std::env::args()
        .skip(1)
        .find(|a| a != "--replay")
        .expect("usage: bench [--replay] <capture-dir>  (see module docs)");
    replay(&dir);
}

/// With `--features stats`, print the cost-shape breakdown accumulated over the
/// timed passes (the warm-up pass's stats were discarded). Region docs —
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
        "      scan:   {:.0} rows/solve",
        s.scan_rows as f64 / s.solves.max(1) as f64,
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
