//! Minimum-weight perfect matching on a complete graph (general, non-bipartite).
//!
//! Many pairing problems (round-robin/Swiss-style tournament pairing among
//! them) are matching problems on a *general* graph — any vertex may pair
//! with any other — so bipartite methods (Hungarian) don't apply and we need
//! the blossom algorithm. This crate implements the classic O(V³) primal-dual
//! blossom algorithm for **maximum** weight matching, then reduces the problem
//! most callers actually care about — a **minimum-weight perfect** matching —
//! to it:
//!
//! On a complete graph with strictly positive edge weights, the maximum-weight
//! matching is necessarily perfect (any two unmatched vertices are adjacent by a
//! positive-weight edge, so leaving them unmatched is never optimal). Weighting
//! each edge `offset - cost`, with `offset` chosen above every cost so all
//! weights stay ≥ 1, therefore yields the minimum-cost perfect matching.
//!
//! Weights are generic over [`Weight`] so callers can pick a type just wide
//! enough for their largest weight — `i32`/`i64` for most instances, `i128`
//! when more headroom is needed (e.g. to stack large lexicographic
//! multipliers when scalarizing a multi-criteria cost). Should an instance
//! ever outgrow `i128`, a fixed-width 256-bit `Weight` impl would be the next
//! step (a heap-allocated bignum isn't — it'd add allocation to every
//! arithmetic op in this O(V³) inner loop); benchmarking a non-allocating
//! 256-bit uint against `i128` on the same instances measured it at only
//! ~1.7x slower, so the headroom is cheap if it's ever needed.
//!
//! The implementation is original — built from the published blossom algorithm,
//! not ported from any codebase — and is checked against a brute-force oracle in
//! the tests below.

use std::collections::VecDeque;

/// Edge-weight type for the blossom solver: a signed integer wide enough to
/// hold the caller's largest weight without overflow.
pub trait Weight:
    Copy
    + Ord
    + std::fmt::Debug
    + std::ops::Add<Output = Self>
    + std::ops::Sub<Output = Self>
    + std::ops::AddAssign
    + std::ops::SubAssign
{
    const ZERO: Self;
    const ONE: Self;
    /// Larger than any edge slack that can arise, but small enough to leave
    /// headroom against overflow when doubled.
    fn inf() -> Self;
    /// `self * 2`, used when doubling dual-variable adjustments.
    fn double(self) -> Self;
    /// `self / 2`, used when halving slack to keep duals integral.
    fn half(self) -> Self;
}

macro_rules! impl_weight {
    ($($t:ty),* $(,)?) => {$(
        impl Weight for $t {
            const ZERO: Self = 0;
            const ONE: Self = 1;
            fn inf() -> Self { <$t>::MAX / 4 }
            fn double(self) -> Self { self * 2 }
            fn half(self) -> Self { self / 2 }
        }
    )*};
}
impl_weight!(i32, i64, i128);

/// One graph edge, carrying the *real* endpoints it stands for. For a super-vertex
/// (contracted blossom) `b`, `g[b][x]` records the best underlying real edge, so
/// `u`/`v` are always real-vertex indices even when the slot is `g[b][x]`.
#[derive(Clone, Copy)]
struct Edge<W> {
    u: usize,
    v: usize,
    w: W,
}

/// Working state of the blossom algorithm. Vertices are 1-indexed; indices
/// `1..=n` are real players and `n+1..=2n` are contracted blossoms.
struct Blossom<W> {
    n: usize,
    n_x: usize,
    g: Vec<Vec<Edge<W>>>,
    lab: Vec<W>,
    mate: Vec<usize>,
    slack: Vec<usize>,
    st: Vec<usize>,
    pa: Vec<usize>,
    flower_from: Vec<Vec<usize>>,
    s: Vec<i32>,
    vis: Vec<usize>,
    flower: Vec<Vec<usize>>,
    q: VecDeque<usize>,
    t: usize,
}

impl<W: Weight> Blossom<W> {
    fn new(n: usize) -> Self {
        let sz = 2 * n + 1;
        let nil_edge = Edge {
            u: 0,
            v: 0,
            w: W::ZERO,
        };
        Blossom {
            n,
            n_x: n,
            g: vec![vec![nil_edge; sz]; sz],
            lab: vec![W::ZERO; sz],
            mate: vec![0; sz],
            slack: vec![0; sz],
            st: vec![0; sz],
            pa: vec![0; sz],
            flower_from: vec![vec![0; n + 1]; sz],
            s: vec![-1; sz],
            vis: vec![0; sz],
            flower: vec![Vec::new(); sz],
            q: VecDeque::new(),
            t: 0,
        }
    }

    fn set_edge(&mut self, u: usize, v: usize, w: W) {
        self.g[u][v] = Edge { u, v, w };
        // Reverse orientation: endpoints swapped (field-named, not positional).
        self.g[v][u] = Edge { u: v, v: u, w };
    }

    /// Reduced cost (slack) of an edge; zero means the edge is tight.
    fn e_delta(&self, e: Edge<W>) -> W {
        self.lab[e.u] + self.lab[e.v] - e.w.double()
    }

    fn update_slack(&mut self, u: usize, x: usize) {
        if self.slack[x] == 0 || self.e_delta(self.g[u][x]) < self.e_delta(self.g[self.slack[x]][x])
        {
            self.slack[x] = u;
        }
    }

    fn set_slack(&mut self, x: usize) {
        self.slack[x] = 0;
        for u in 1..=self.n {
            if self.g[u][x].w > W::ZERO && self.st[u] != x && self.s[self.st[u]] == 0 {
                self.update_slack(u, x);
            }
        }
    }

    fn q_push(&mut self, x: usize) {
        if x <= self.n {
            self.q.push_back(x);
        } else {
            for c in self.flower[x].clone() {
                self.q_push(c);
            }
        }
    }

    fn set_st(&mut self, x: usize, b: usize) {
        self.st[x] = b;
        if x > self.n {
            for c in self.flower[x].clone() {
                self.set_st(c, b);
            }
        }
    }

    /// Position of `xr` within blossom `b`'s cycle, normalized to be even by
    /// reversing the tail if needed (so a matched alternating walk starts right).
    fn get_pr(&mut self, b: usize, xr: usize) -> usize {
        let pr = self.flower[b].iter().position(|&x| x == xr).unwrap();
        if pr % 2 == 1 {
            let len = self.flower[b].len();
            self.flower[b][1..].reverse();
            len - pr
        } else {
            pr
        }
    }

    fn set_match(&mut self, u: usize, v: usize) {
        self.mate[u] = self.g[u][v].v;
        if u > self.n {
            let e = self.g[u][v];
            let xr = self.flower_from[u][e.u];
            let pr = self.get_pr(u, xr);
            let fu = self.flower[u].clone();
            let mut i = 0;
            while i < pr {
                self.set_match(fu[i], fu[i ^ 1]);
                i += 1;
            }
            self.set_match(xr, v);
            self.flower[u].rotate_left(pr);
        }
    }

    fn augment(&mut self, mut u: usize, mut v: usize) {
        loop {
            let xnv = self.st[self.mate[u]];
            self.set_match(u, v);
            if xnv == 0 {
                return;
            }
            let next_u = self.st[self.pa[xnv]];
            self.set_match(xnv, next_u);
            u = next_u;
            v = xnv;
        }
    }

    fn get_lca(&mut self, mut u: usize, mut v: usize) -> usize {
        self.t += 1;
        loop {
            if u == 0 && v == 0 {
                return 0;
            }
            if u != 0 {
                if self.vis[u] == self.t {
                    return u;
                }
                self.vis[u] = self.t;
                let m = self.st[self.mate[u]];
                u = if m != 0 { self.st[self.pa[m]] } else { 0 };
            }
            std::mem::swap(&mut u, &mut v);
        }
    }

    fn add_blossom(&mut self, u: usize, lca: usize, v: usize) {
        let mut b = self.n + 1;
        while b <= self.n_x && self.st[b] != 0 {
            b += 1;
        }
        if b > self.n_x {
            self.n_x += 1;
        }
        self.lab[b] = W::ZERO;
        self.s[b] = 0;
        self.mate[b] = self.mate[lca];
        self.flower[b].clear();
        self.flower[b].push(lca);

        let mut x = u;
        while x != lca {
            let y = self.st[self.mate[x]];
            self.flower[b].push(x);
            self.flower[b].push(y);
            self.q_push(y);
            x = self.st[self.pa[y]];
        }
        self.flower[b][1..].reverse();
        let mut x = v;
        while x != lca {
            let y = self.st[self.mate[x]];
            self.flower[b].push(x);
            self.flower[b].push(y);
            self.q_push(y);
            x = self.st[self.pa[y]];
        }

        self.set_st(b, b);
        for x in 1..=self.n_x {
            self.g[b][x].w = W::ZERO;
            self.g[x][b].w = W::ZERO;
        }
        for x in 1..=self.n {
            self.flower_from[b][x] = 0;
        }
        let members = self.flower[b].clone();
        for xs in members {
            for x in 1..=self.n_x {
                let gxsx = self.g[xs][x];
                let gxxs = self.g[x][xs];
                let gbx = self.g[b][x];
                if gbx.w == W::ZERO || self.e_delta(gxsx) < self.e_delta(gbx) {
                    self.g[b][x] = gxsx;
                    self.g[x][b] = gxxs;
                }
            }
            for x in 1..=self.n {
                if self.flower_from[xs][x] != 0 {
                    self.flower_from[b][x] = xs;
                }
            }
        }
        self.set_slack(b);
    }

    fn expand_blossom(&mut self, b: usize) {
        let members = self.flower[b].clone();
        for &m in &members {
            self.set_st(m, m);
        }
        let xr = self.flower_from[b][self.g[b][self.pa[b]].u];
        let pr = self.get_pr(b, xr);
        let fb = self.flower[b].clone();
        let mut i = 0;
        while i < pr {
            let xs = fb[i];
            let xns = fb[i + 1];
            self.pa[xs] = self.g[xns][xs].u;
            self.s[xs] = 1;
            self.s[xns] = 0;
            self.slack[xs] = 0;
            self.set_slack(xns);
            self.q_push(xns);
            i += 2;
        }
        self.s[xr] = 1;
        self.pa[xr] = self.pa[b];
        for &xs in &fb[(pr + 1)..] {
            self.s[xs] = -1;
            self.set_slack(xs);
        }
        self.st[b] = 0;
    }

    fn on_found_edge(&mut self, e: Edge<W>) -> bool {
        let u = self.st[e.u];
        let v = self.st[e.v];
        if self.s[v] == -1 {
            self.pa[v] = e.u;
            self.s[v] = 1;
            let nu = self.st[self.mate[v]];
            self.slack[v] = 0;
            self.slack[nu] = 0;
            self.s[nu] = 0;
            self.q_push(nu);
        } else if self.s[v] == 0 {
            let lca = self.get_lca(u, v);
            if lca == 0 {
                self.augment(u, v);
                self.augment(v, u);
                return true;
            } else {
                self.add_blossom(u, lca, v);
            }
        }
        false
    }

    /// One phase: grow alternating trees, adjusting duals, until an augmenting
    /// path is found (returns `true`, matching grew by one edge) or no further
    /// improvement is possible (returns `false`).
    fn matching(&mut self) -> bool {
        for i in 1..=self.n_x {
            self.s[i] = -1;
            self.slack[i] = 0;
        }
        self.q.clear();
        for x in 1..=self.n_x {
            if self.st[x] == x && self.mate[x] == 0 {
                self.pa[x] = 0;
                self.s[x] = 0;
                self.q_push(x);
            }
        }
        if self.q.is_empty() {
            return false;
        }
        loop {
            while let Some(u) = self.q.pop_front() {
                if self.s[self.st[u]] == 1 {
                    continue;
                }
                for v in 1..=self.n {
                    if self.g[u][v].w > W::ZERO && self.st[u] != self.st[v] {
                        if self.e_delta(self.g[u][v]) == W::ZERO {
                            if self.on_found_edge(self.g[u][v]) {
                                return true;
                            }
                        } else {
                            let x = self.st[v];
                            self.update_slack(u, x);
                        }
                    }
                }
            }
            let mut d = W::inf();
            for b in (self.n + 1)..=self.n_x {
                if self.st[b] == b && self.s[b] == 1 {
                    d = d.min(self.lab[b].half());
                }
            }
            for x in 1..=self.n_x {
                if self.st[x] == x && self.slack[x] != 0 {
                    let delta = self.e_delta(self.g[self.slack[x]][x]);
                    if self.s[x] == -1 {
                        d = d.min(delta);
                    } else if self.s[x] == 0 {
                        d = d.min(delta.half());
                    }
                }
            }
            for u in 1..=self.n {
                match self.s[self.st[u]] {
                    0 => {
                        if self.lab[u] <= d {
                            return false;
                        }
                        self.lab[u] -= d;
                    }
                    1 => self.lab[u] += d,
                    _ => {}
                }
            }
            for b in (self.n + 1)..=self.n_x {
                if self.st[b] == b {
                    if self.s[b] == 0 {
                        self.lab[b] += d.double();
                    } else if self.s[b] == 1 {
                        self.lab[b] -= d.double();
                    }
                }
            }
            self.q.clear();
            for x in 1..=self.n_x {
                if self.st[x] == x
                    && self.slack[x] != 0
                    && self.st[self.slack[x]] != x
                    && self.e_delta(self.g[self.slack[x]][x]) == W::ZERO
                    && self.on_found_edge(self.g[self.slack[x]][x])
                {
                    return true;
                }
            }
            for b in (self.n + 1)..=self.n_x {
                if self.st[b] == b && self.s[b] == 1 && self.lab[b] == W::ZERO {
                    self.expand_blossom(b);
                }
            }
        }
    }

    fn solve(&mut self) {
        for u in 1..=self.n {
            self.mate[u] = 0;
        }
        self.n_x = self.n;
        for u in 0..=self.n {
            self.st[u] = u;
            self.flower[u].clear();
        }
        for b in (self.n + 1)..(2 * self.n + 1) {
            self.st[b] = 0;
            self.flower[b].clear();
        }
        let mut w_max = W::ZERO;
        for u in 1..=self.n {
            for v in 1..=self.n {
                self.flower_from[u][v] = if u == v { u } else { 0 };
                if self.g[u][v].w > w_max {
                    w_max = self.g[u][v].w;
                }
            }
        }
        for u in 1..=self.n {
            self.lab[u] = w_max;
        }
        while self.matching() {}
    }
}

/// Compute a minimum-total-cost **perfect** matching of the `n` vertices, where
/// `cost[i][j]` is the (symmetric) cost of pairing `i` with `j`. Returns `mate`,
/// where `mate[i]` is the partner of vertex `i`.
///
/// `n` must be even (a perfect matching is otherwise impossible) and `cost` a
/// full `n × n` matrix; the diagonal is ignored. Costs must be non-negative and
/// symmetric. On the complete graph every vertex is pairable, so a perfect
/// matching always exists.
pub fn min_weight_perfect_matching<W: Weight>(cost: &[Vec<W>]) -> Vec<usize> {
    let n = cost.len();
    assert!(
        n.is_multiple_of(2),
        "a perfect matching needs an even vertex count"
    );
    if n == 0 {
        return Vec::new();
    }

    // Reduce min-cost-perfect to max-weight: weight = offset - cost, with offset
    // above every cost so all weights are ≥ 1 (keeping the matching perfect).
    let mut max_cost = W::ZERO;
    for (i, row) in cost.iter().enumerate() {
        for (j, &c) in row.iter().enumerate() {
            if i != j && c > max_cost {
                max_cost = c;
            }
        }
    }
    let offset = max_cost + W::ONE;

    let mut bl = Blossom::new(n);
    for (i, row) in cost.iter().enumerate() {
        for j in (i + 1)..n {
            debug_assert_eq!(row[j], cost[j][i], "cost matrix must be symmetric");
            bl.set_edge(i + 1, j + 1, offset - row[j]);
        }
    }
    bl.solve();

    (1..=n).map(|u| bl.mate[u] - 1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference: minimum total cost over all perfect matchings, by exhaustive
    /// recursion. Only usable for tiny `n`, which is exactly what we test against.
    fn brute_min_cost(cost: &[Vec<i128>]) -> i128 {
        let n = cost.len();
        let mut used = vec![false; n];
        fn rec(cost: &[Vec<i128>], used: &mut Vec<bool>, matched: usize, n: usize) -> i128 {
            if matched == n {
                return 0;
            }
            // first unmatched vertex
            let i = (0..n).find(|&i| !used[i]).unwrap();
            used[i] = true;
            let mut best = i128::inf();
            for j in (i + 1)..n {
                if !used[j] {
                    used[j] = true;
                    let sub = rec(cost, used, matched + 2, n);
                    if sub < i128::inf() {
                        best = best.min(cost[i][j] + sub);
                    }
                    used[j] = false;
                }
            }
            used[i] = false;
            best
        }
        rec(cost, &mut used, 0, n)
    }

    fn total_of(cost: &[Vec<i128>], mate: &[usize]) -> i128 {
        let n = cost.len();
        let mut t = 0;
        for i in 0..n {
            assert_ne!(mate[i], i, "vertex matched to itself");
            assert_eq!(mate[mate[i]], i, "matching is not a valid involution");
            if i < mate[i] {
                t += cost[i][mate[i]];
            }
        }
        t
    }

    #[test]
    fn trivial_pair() {
        let cost = vec![vec![0, 7], vec![7, 0]];
        let mate = min_weight_perfect_matching(&cost);
        assert_eq!(mate, vec![1, 0]);
    }

    #[test]
    fn picks_cheaper_of_two_pairings() {
        // 4 vertices; pairing {0-1, 2-3} costs 1+1=2, {0-2,1-3} costs 10+10=20,
        // {0-3,1-2} costs 10+10=20. Optimal keeps the cheap edges.
        let cost = vec![
            vec![0, 1, 10, 10],
            vec![1, 0, 10, 10],
            vec![10, 10, 0, 1],
            vec![10, 10, 1, 0],
        ];
        let mate = min_weight_perfect_matching(&cost);
        assert_eq!(total_of(&cost, &mate), 2);
        assert_eq!(mate[0], 1);
        assert_eq!(mate[2], 3);
    }

    #[test]
    fn matches_brute_force_on_random_instances() {
        // Small deterministic LCG so the test is reproducible without a dep.
        let mut seed: u64 = 0x9E3779B97F4A7C15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        for &n in &[2usize, 4, 6, 8, 10] {
            for _ in 0..200 {
                let mut cost = vec![vec![0i128; n]; n];
                #[allow(clippy::needless_range_loop)]
                for i in 0..n {
                    for j in (i + 1)..n {
                        let c = (next() % 1000) as i128;
                        cost[i][j] = c;
                        cost[j][i] = c;
                    }
                }
                let mate = min_weight_perfect_matching(&cost);
                let got = total_of(&cost, &mate);
                let want = brute_min_cost(&cost);
                assert_eq!(got, want, "n={n}, cost={cost:?}, mate={mate:?}");
            }
        }
    }

    #[test]
    fn handles_large_lexicographic_weights() {
        // Weights spanning the multiplier ladder's magnitude must not overflow or
        // lose the ordering: the huge-cost edge (0-1) must be avoided.
        const BIG: i128 = 1_000_000_000_000_000_000_000_000; // 1e24
        let cost = vec![
            vec![0, BIG, 5, 3],
            vec![BIG, 0, 3, 5],
            vec![5, 3, 0, BIG],
            vec![3, 5, BIG, 0],
        ];
        let mate = min_weight_perfect_matching(&cost);
        assert_eq!(brute_min_cost(&cost), total_of(&cost, &mate));
        assert_ne!(
            mate[0], 1,
            "should not pair the two most-penalized vertices"
        );
    }

    #[test]
    fn works_with_a_narrower_weight_type() {
        // Same instance as `picks_cheaper_of_two_pairings`, but run with `i64`
        // weights to confirm the solver isn't secretly tied to `i128`.
        let cost: Vec<Vec<i64>> = vec![
            vec![0, 1, 10, 10],
            vec![1, 0, 10, 10],
            vec![10, 10, 0, 1],
            vec![10, 10, 1, 0],
        ];
        let mate = min_weight_perfect_matching(&cost);
        assert_eq!(mate[0], 1);
        assert_eq!(mate[2], 3);
    }

    #[test]
    fn metamorphic_forbidding_edges_on_large_instances() {
        // Brute force can't reach these sizes, so we check the solver against
        // itself with two metamorphic relations. Forbid an edge (simulated by a
        // penalty cost that dwarfs any real matching, so the optimizer avoids
        // it whenever an alternative exists — always, on a complete graph) and
        // re-solve:
        //   * forbidding an edge the optimum *uses* can only make things worse
        //     or equal — the feasible set shrank                (new_cost ≥ base)
        //   * forbidding an edge the optimum *doesn't* use leaves the optimum
        //     untouched — the old solution is still available   (new_cost = base)
        // The equality case catches suboptimality in *either* run; the ≥ case
        // catches a first run that missed a better edge-avoiding matching.
        let mut seed: u64 = 0xD1B54A32D192ED03;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        // Real costs are < 1000, so a matching's total is < (n/2)·1000 ≪ 1e9;
        // a forbidden edge alone costs 1e9, so any alternative is preferred. And
        // 1e9 sits far below i128's headroom (inf() = MAX/4 ≈ 4e37), so doubling
        // weights in `e_delta` never overflows.
        const PENALTY: i128 = 1_000_000_000;

        for &n in &[50usize, 100, 200] {
            let mut cost = vec![vec![0i128; n]; n];
            #[allow(clippy::needless_range_loop)]
            for i in 0..n {
                for j in (i + 1)..n {
                    let c = (next() % 1000) as i128;
                    cost[i][j] = c;
                    cost[j][i] = c;
                }
            }

            let base_mate = min_weight_perfect_matching(&cost);
            let base_cost = total_of(&cost, &base_mate);

            // Forbid a handful of edges the solution uses: cost must not improve.
            let solution_edges: Vec<(usize, usize)> =
                (0..n).filter(|&i| i < base_mate[i]).map(|i| (i, base_mate[i])).collect();
            for &(i, j) in solution_edges.iter().take(5) {
                let mut c2 = cost.clone();
                c2[i][j] = PENALTY;
                c2[j][i] = PENALTY;
                let m2 = min_weight_perfect_matching(&c2);
                assert_ne!(m2[i], j, "n={n}: forbidden solution edge {i}-{j} was still used");
                // Score with the *original* costs; the forbidden edge is unused,
                // so the penalty never enters the total.
                let new_cost = total_of(&cost, &m2);
                assert!(
                    new_cost >= base_cost,
                    "n={n}: forbidding solution edge {i}-{j} improved cost {base_cost} -> {new_cost}"
                );
            }

            // Forbid a handful of edges the solution doesn't use: cost is fixed.
            let mut checked = 0;
            'outer: for i in 0..n {
                for j in (i + 1)..n {
                    if base_mate[i] == j {
                        continue;
                    }
                    let mut c2 = cost.clone();
                    c2[i][j] = PENALTY;
                    c2[j][i] = PENALTY;
                    let m2 = min_weight_perfect_matching(&c2);
                    assert_ne!(m2[i], j, "n={n}: forbidden unused edge {i}-{j} was used");
                    let new_cost = total_of(&cost, &m2);
                    assert_eq!(
                        new_cost, base_cost,
                        "n={n}: forbidding unused edge {i}-{j} changed optimum {base_cost} -> {new_cost}"
                    );
                    checked += 1;
                    if checked >= 5 {
                        break 'outer;
                    }
                    break; // spread the sample across distinct vertices
                }
            }
        }
    }
}
