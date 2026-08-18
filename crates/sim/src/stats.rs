//! Pure statistical aggregation over simulation runs — kept apart from IO so it
//! can be unit-tested directly.

use std::collections::HashMap;

#[cfg(test)]
use uuid::Uuid;

/// Summary of a pooled distribution of absolute ELO differences.
#[derive(Debug, Clone)]
pub(crate) struct DiffStats {
    /// Number of games pooled.
    pub count: usize,
    pub mean: f64,
    pub median: f64,
    pub p90: f64,
    pub p95: f64,
    /// For each referee threshold `T`, the fraction of games with `|diff| > T`.
    pub exceed: Vec<(f64, f64)>,
}

/// The `q`-quantile (0..=1) of an already-sorted slice, by linear interpolation.
/// Returns 0.0 for an empty slice.
pub(crate) fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let frac = pos - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

/// Summarise pooled absolute ELO differences, including `P(|diff| > T)` for each
/// threshold in `thresholds`.
pub(crate) fn diff_stats(diffs: &[f64], thresholds: &[f64]) -> DiffStats {
    let count = diffs.len();
    if count == 0 {
        return DiffStats {
            count: 0,
            mean: 0.0,
            median: 0.0,
            p90: 0.0,
            p95: 0.0,
            exceed: thresholds.iter().map(|&t| (t, 0.0)).collect(),
        };
    }
    let mut sorted = diffs.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean = sorted.iter().sum::<f64>() / count as f64;
    let exceed = thresholds
        .iter()
        .map(|&t| {
            let over = sorted.iter().filter(|&&d| d > t).count();
            (t, over as f64 / count as f64)
        })
        .collect();
    DiffStats {
        count,
        mean,
        median: quantile(&sorted, 0.5),
        p90: quantile(&sorted, 0.90),
        p95: quantile(&sorted, 0.95),
        exceed,
    }
}

/// A proportion with a 95% Wilson score interval — the honest error bar on a
/// victory probability estimated from `n` runs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Proportion {
    pub p: f64,
    pub lo: f64,
    pub hi: f64,
}

/// 95% Wilson score interval for `successes` out of `n`. `z = 1.96`.
pub(crate) fn wilson(successes: usize, n: usize) -> Proportion {
    if n == 0 {
        return Proportion {
            p: 0.0,
            lo: 0.0,
            hi: 0.0,
        };
    }
    let z = 1.959_963_984_540_054_f64;
    let n = n as f64;
    let p = successes as f64 / n;
    let z2 = z * z;
    let denom = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denom;
    let margin = (z / denom) * ((p * (1.0 - p) / n) + z2 / (4.0 * n * n)).sqrt();
    Proportion {
        p,
        lo: (center - margin).max(0.0),
        hi: (center + margin).min(1.0),
    }
}

/// Spearman rank correlation between two orderings of the **same** set of ids
/// (each a permutation, rank 0 = first). Returns 1.0 for identical orders, -1.0
/// for exact reverse. Falls back to 0.0 if the two don't cover the same ids.
pub(crate) fn spearman<T: Copy + Eq + std::hash::Hash>(order_a: &[T], order_b: &[T]) -> f64 {
    let n = order_a.len();
    if n != order_b.len() || n < 2 {
        return if n < 2 { 1.0 } else { 0.0 };
    }
    let rank_b: HashMap<T, usize> = order_b.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut sum_d2 = 0i64;
    for (rank_a, id) in order_a.iter().enumerate() {
        let Some(&rb) = rank_b.get(id) else {
            return 0.0; // id sets differ — undefined, report no correlation
        };
        let d = rank_a as i64 - rb as i64;
        sum_d2 += d * d;
    }
    let n = n as f64;
    // No ties: both are strict permutations, so the closed form is exact.
    1.0 - (6.0 * sum_d2 as f64) / (n * (n * n - 1.0))
}

/// The mean of a slice, or 0.0 if empty.
pub(crate) fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// The sample standard deviation (Bessel-corrected), or 0.0 for fewer than two
/// values.
pub(crate) fn std_dev(values: &[f64]) -> f64 {
    let n = values.len();
    if n < 2 {
        return 0.0;
    }
    let m = mean(values);
    let var = values.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1) as f64;
    var.sqrt()
}

/// A point estimate of a mean with its 95% confidence half-width (`1.96·SE`),
/// treating the values as independent replications. The half-width is 0 for fewer
/// than two values.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MeanCi {
    pub mean: f64,
    pub ci: f64,
}

const Z_95: f64 = 1.959_963_984_540_054_f64;

/// Mean and 95% CI half-width of `values` (normal approximation).
pub(crate) fn mean_ci95(values: &[f64]) -> MeanCi {
    let n = values.len();
    if n < 2 {
        return MeanCi {
            mean: mean(values),
            ci: 0.0,
        };
    }
    let se = std_dev(values) / (n as f64).sqrt();
    MeanCi {
        mean: mean(values),
        ci: Z_95 * se,
    }
}

/// A **paired** mean difference `mean(b − a)` with its 95% CI half-width and a
/// z-statistic, for a common-random-numbers A/B where `a[i]` and `b[i]` come from
/// the same run/seed. Pairing subtracts per-run first, so any shared run-to-run
/// variation cancels. `None` if the slices differ in length or have fewer than two
/// pairs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PairedDiff {
    /// `mean(b − a)` — positive means `b` scores higher on the metric.
    pub delta: f64,
    pub ci: f64,
    /// `delta / SE`; magnitude over ~1.96 is significant at p < 0.05.
    pub z: f64,
}

pub(crate) fn paired_diff(a: &[f64], b: &[f64]) -> Option<PairedDiff> {
    if a.len() != b.len() || a.len() < 2 {
        return None;
    }
    let diffs: Vec<f64> = a.iter().zip(b).map(|(x, y)| y - x).collect();
    let delta = mean(&diffs);
    let se = std_dev(&diffs) / (diffs.len() as f64).sqrt();
    let z = if se > 0.0 { delta / se } else { 0.0 };
    Some(PairedDiff {
        delta,
        ci: Z_95 * se,
        z,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_interpolate() {
        let v = [0.0, 10.0, 20.0, 30.0, 40.0];
        assert_eq!(quantile(&v, 0.5), 20.0);
        assert_eq!(quantile(&v, 0.0), 0.0);
        assert_eq!(quantile(&v, 1.0), 40.0);
        assert_eq!(quantile(&v, 0.25), 10.0);
    }

    #[test]
    fn diff_stats_counts_exceedances() {
        let diffs = [100.0, 200.0, 500.0, 600.0];
        let s = diff_stats(&diffs, &[400.0, 550.0]);
        assert_eq!(s.count, 4);
        assert_eq!(s.mean, 350.0);
        // Two of four exceed 400, one of four exceeds 550.
        assert_eq!(s.exceed[0], (400.0, 0.5));
        assert_eq!(s.exceed[1], (550.0, 0.25));
    }

    #[test]
    fn empty_diffs_are_safe() {
        let s = diff_stats(&[], &[400.0]);
        assert_eq!(s.count, 0);
        assert_eq!(s.exceed[0], (400.0, 0.0));
    }

    #[test]
    fn wilson_interval_brackets_the_estimate() {
        let w = wilson(50, 100);
        assert!((w.p - 0.5).abs() < 1e-12);
        assert!(w.lo < 0.5 && w.hi > 0.5);
        // Wider interval for fewer trials.
        let few = wilson(5, 10);
        assert!(few.hi - few.lo > w.hi - w.lo);
        // Degenerate cases stay in range.
        assert_eq!(wilson(0, 0).p, 0.0);
        let all = wilson(10, 10);
        assert!(all.hi <= 1.0 && all.p == 1.0);
    }

    #[test]
    fn mean_ci_shrinks_with_more_samples() {
        // Same spread, 4× the samples → half the CI (SE ∝ 1/√n).
        let small: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let big: Vec<f64> = (0..10).flat_map(|i| [i as f64; 4]).collect();
        let cs = mean_ci95(&small);
        let cb = mean_ci95(&big);
        assert!((cs.mean - cb.mean).abs() < 1e-9); // same mean
                                                   // ~2× tighter (not exactly, since Bessel's correction differs at n=10 vs 40).
        let ratio = cs.ci / cb.ci;
        assert!((1.9..2.3).contains(&ratio), "ratio {ratio}");
        // Degenerate: fewer than two values has no interval.
        assert_eq!(mean_ci95(&[42.0]).ci, 0.0);
    }

    #[test]
    fn paired_diff_cancels_shared_variation() {
        // b is a shifted-by-5 copy of a: every paired difference is exactly 5, so
        // the delta is 5 with a zero-width interval even though a itself varies a lot.
        let a = [1.0, 100.0, -30.0, 42.0, 7.0];
        let b: Vec<f64> = a.iter().map(|x| x + 5.0).collect();
        let d = paired_diff(&a, &b).unwrap();
        assert!((d.delta - 5.0).abs() < 1e-9);
        assert!(d.ci < 1e-9); // no residual variation once paired
                              // Mismatched lengths / too few pairs: undefined.
        assert!(paired_diff(&a, &b[..3]).is_none());
        assert!(paired_diff(&[1.0], &[2.0]).is_none());
    }

    #[test]
    fn paired_diff_sign_and_significance() {
        // A tiny but perfectly consistent improvement is significant; noise isn't.
        let a = [10.0, 10.0, 10.0, 10.0, 10.0, 10.0];
        let b = [10.5, 10.4, 10.6, 10.5, 10.5, 10.5]; // ~+0.5, low variance
        let d = paired_diff(&a, &b).unwrap();
        assert!(d.delta > 0.0);
        assert!(d.z.abs() > 1.96); // significant at 5%
    }

    #[test]
    fn spearman_is_1_for_identical_and_minus1_for_reverse() {
        let a = [
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            Uuid::from_u128(3),
            Uuid::from_u128(4),
        ];
        let rev: Vec<Uuid> = a.iter().rev().copied().collect();
        assert!((spearman(&a, &a) - 1.0).abs() < 1e-12);
        assert!((spearman(&a, &rev) + 1.0).abs() < 1e-12);
    }

    #[test]
    fn spearman_handles_a_single_swap() {
        // Swapping the last two of four: ρ = 1 - 6*(2)/(4*15) = 0.8.
        let a = [
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            Uuid::from_u128(3),
            Uuid::from_u128(4),
        ];
        let b = [
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            Uuid::from_u128(4),
            Uuid::from_u128(3),
        ];
        assert!((spearman(&a, &b) - 0.8).abs() < 1e-12);
    }
}
