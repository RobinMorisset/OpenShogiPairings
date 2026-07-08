//! Pure statistical aggregation over simulation runs — kept apart from IO so it
//! can be unit-tested directly.

use std::collections::HashMap;

use uuid::Uuid;

/// Summary of a pooled distribution of absolute ELO differences.
#[derive(Debug, Clone)]
pub struct DiffStats {
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
pub fn quantile(sorted: &[f64], q: f64) -> f64 {
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
pub fn diff_stats(diffs: &[f64], thresholds: &[f64]) -> DiffStats {
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
pub struct Proportion {
    pub p: f64,
    pub lo: f64,
    pub hi: f64,
}

/// 95% Wilson score interval for `successes` out of `n`. `z = 1.96`.
pub fn wilson(successes: usize, n: usize) -> Proportion {
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
pub fn spearman(order_a: &[Uuid], order_b: &[Uuid]) -> f64 {
    let n = order_a.len();
    if n != order_b.len() || n < 2 {
        return if n < 2 { 1.0 } else { 0.0 };
    }
    let rank_b: HashMap<Uuid, usize> = order_b.iter().enumerate().map(|(i, id)| (*id, i)).collect();
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
pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
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
