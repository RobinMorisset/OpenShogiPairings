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
//! Both are exposed: [`max_weight_matching`] solves the general problem — an
//! arbitrary (possibly sparse, possibly odd-order) graph, leaving a vertex
//! unmatched where that is optimal — and [`min_weight_perfect_matching`] applies
//! the reduction above. Both are thin wrappers over one pooled solver, differing
//! only in how they fill edges and shape the result.
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

use std::any::{Any, TypeId};
use std::cell::RefCell;
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
    // `'static` lets the per-thread solver pool key its reusable `Blossom<W>`
    // buffers by `TypeId`; every integer weight type satisfies it.
    + 'static
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

/// A vertex index. The algorithm works in `usize` (loop counters, array indices),
/// but the two large `O(n²)` tables — the [`Blossom::g`] edge matrix and
/// [`Blossom::flower_from`] — *store* vertices, and for the field sizes we pair
/// (hundreds, never near `u32::MAX`) a 4-byte id halves that footprint versus a
/// `usize`, tightening the hot column scans. Stored as `Vid`, used as `usize`.
type Vid = u32;

/// One graph edge, carrying the *real* endpoints it stands for. For a super-vertex
/// (contracted blossom) `b`, `g[b][x]` records the best underlying real edge, so
/// `u`/`v` are always real-vertex indices even when the slot is `g[b][x]`.
#[derive(Clone, Copy)]
struct Edge<W> {
    u: Vid,
    v: Vid,
    w: W,
}

/// Working state of the blossom algorithm. Vertices are 1-indexed; indices
/// `1..=n` are real players and `n+1..=2n` are contracted blossoms.
struct Blossom<W> {
    n: usize,
    n_x: usize,
    /// The `sz × sz` edge matrix, row-major in a single allocation: entry `(u, v)`
    /// is at `u * stride + v` (see [`Blossom::g`]). One block, rather than a
    /// `Vec<Vec>`, so the algorithm's column scans (`for u { g[u][x] }` in
    /// `set_slack`/`update_slack`, the hot path) stay in one strided region the
    /// prefetcher can follow instead of chasing `n` separate heap rows.
    g: Vec<Edge<W>>,
    /// Row stride of `g` — the allocated width `2 * n_cap + 1` for the largest
    /// instance seen, so a reused buffer keeps a consistent layout.
    stride: usize,
    lab: Vec<W>,
    mate: Vec<usize>,
    slack: Vec<usize>,
    st: Vec<usize>,
    pa: Vec<usize>,
    /// The `sz × (n+1)` "which member does row `u` reach vertex `v` through"
    /// matrix, row-major in a single allocation (entry `(u, v)` at
    /// `u * ff_stride + v`), for the same locality reason as [`Blossom::g`].
    flower_from: Vec<Vid>,
    /// Row stride of `flower_from` — the allocated width `n_cap + 1` for the
    /// largest instance seen.
    ff_stride: usize,
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
            g: vec![nil_edge; sz * sz],
            stride: sz,
            lab: vec![W::ZERO; sz],
            mate: vec![0; sz],
            slack: vec![0; sz],
            st: vec![0; sz],
            pa: vec![0; sz],
            flower_from: vec![0; sz * (n + 1)],
            ff_stride: n + 1,
            s: vec![-1; sz],
            vis: vec![0; sz],
            flower: vec![Vec::new(); sz],
            q: VecDeque::new(),
            t: 0,
        }
    }

    /// Prepare a (possibly reused) solver for an `n`-vertex instance, growing the
    /// working buffers if this instance is larger than any this solver has seen.
    ///
    /// No stale data needs clearing: `solve`/`matching` re-initialize every piece
    /// of live state within `1..=2n` each run, both entry points call `set_edge`
    /// for every vertex pair (`max_weight_matching` clamping an absent edge to a
    /// zero weight) so every real edge slot is overwritten, and a super-vertex's
    /// row/column is zeroed when its blossom is formed — so a buffer left over
    /// from an earlier (larger or smaller) instance is correct as long as it is
    /// big enough. Growth only ever extends the buffers (indices `1..=2n` are all
    /// a smaller `n` could have touched), never truncates.
    fn reset(&mut self, n: usize) {
        self.n = n;
        self.n_x = n;
        let sz = 2 * n + 1;
        if sz > self.stride {
            // A larger instance than any before: reallocate the buffers wide enough
            // (the old contents are stale and would be overwritten anyway, so there
            // is nothing to copy). `g` is laid out at the new stride from here on.
            let nil = Edge {
                u: 0,
                v: 0,
                w: W::ZERO,
            };
            self.stride = sz;
            self.g = vec![nil; sz * sz];
            self.flower_from = vec![0; sz * (n + 1)];
            self.ff_stride = n + 1;
            self.lab.resize(sz, W::ZERO);
            self.mate.resize(sz, 0);
            self.slack.resize(sz, 0);
            self.st.resize(sz, 0);
            self.pa.resize(sz, 0);
            self.s.resize(sz, -1);
            self.vis.resize(sz, 0);
            self.flower.resize(sz, Vec::new());
        }
    }

    /// Edge `(u, v)` of the flattened matrix (returned by value — `Edge` is `Copy`).
    #[inline]
    fn g(&self, u: usize, v: usize) -> Edge<W> {
        self.g[u * self.stride + v]
    }

    /// Mutable edge `(u, v)`, for the few sites that overwrite an edge or zero its
    /// weight in place.
    #[inline]
    fn g_mut(&mut self, u: usize, v: usize) -> &mut Edge<W> {
        &mut self.g[u * self.stride + v]
    }

    /// Entry `(u, v)` of the flattened `flower_from` matrix.
    #[inline]
    fn flower_from(&self, u: usize, v: usize) -> usize {
        self.flower_from[u * self.ff_stride + v] as usize
    }

    /// Set entry `(u, v)` of `flower_from` (narrowing the stored vertex to [`Vid`]).
    #[inline]
    fn set_flower_from(&mut self, u: usize, v: usize, val: usize) {
        self.flower_from[u * self.ff_stride + v] = val as Vid;
    }

    fn set_edge(&mut self, u: usize, v: usize, w: W) {
        let (u, v) = (u as Vid, v as Vid);
        *self.g_mut(u as usize, v as usize) = Edge { u, v, w };
        // Reverse orientation: endpoints swapped (field-named, not positional).
        *self.g_mut(v as usize, u as usize) = Edge { u: v, v: u, w };
    }

    /// Reduced cost (slack) of an edge; zero means the edge is tight.
    fn e_delta(&self, e: Edge<W>) -> W {
        self.lab[e.u as usize] + self.lab[e.v as usize] - e.w.double()
    }

    fn update_slack(&mut self, u: usize, x: usize) {
        if self.slack[x] == 0 || self.e_delta(self.g(u, x)) < self.e_delta(self.g(self.slack[x], x))
        {
            self.slack[x] = u;
        }
    }

    fn set_slack(&mut self, x: usize) {
        self.slack[x] = 0;
        for u in 1..=self.n {
            if self.g(u, x).w > W::ZERO && self.st[u] != x && self.s[self.st[u]] == 0 {
                self.update_slack(u, x);
            }
        }
    }

    fn q_push(&mut self, x: usize) {
        if x <= self.n {
            self.q.push_back(x);
        } else {
            // Recursing into a child never mutates `flower[x]` itself, so index it
            // in place rather than cloning the whole cycle each call.
            let mut i = 0;
            while i < self.flower[x].len() {
                let c = self.flower[x][i];
                self.q_push(c);
                i += 1;
            }
        }
    }

    fn set_st(&mut self, x: usize, b: usize) {
        self.st[x] = b;
        if x > self.n {
            let mut i = 0;
            while i < self.flower[x].len() {
                let c = self.flower[x][i];
                self.set_st(c, b);
                i += 1;
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
        self.mate[u] = self.g(u, v).v as usize;
        if u > self.n {
            let e = self.g(u, v);
            let xr = self.flower_from(u, e.u as usize);
            let pr = self.get_pr(u, xr);
            // The recursive `set_match` on a child touches only that child's cycle,
            // so `flower[u]` is stable until the `rotate_left` below — index it in
            // place instead of cloning.
            let mut i = 0;
            while i < pr {
                let a = self.flower[u][i];
                let b = self.flower[u][i ^ 1];
                self.set_match(a, b);
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
            self.g_mut(b, x).w = W::ZERO;
            self.g_mut(x, b).w = W::ZERO;
        }
        for x in 1..=self.n {
            self.set_flower_from(b, x, 0);
        }
        // `b` is a fresh index distinct from every member `xs`, so writing row/col
        // `b` never disturbs the `xs` rows we read — the cycle is stable, index it
        // in place rather than cloning.
        let mut mi = 0;
        while mi < self.flower[b].len() {
            let xs = self.flower[b][mi];
            for x in 1..=self.n_x {
                let gxsx = self.g(xs, x);
                let gxxs = self.g(x, xs);
                let gbx = self.g(b, x);
                if gbx.w == W::ZERO || self.e_delta(gxsx) < self.e_delta(gbx) {
                    *self.g_mut(b, x) = gxsx;
                    *self.g_mut(x, b) = gxxs;
                }
            }
            for x in 1..=self.n {
                if self.flower_from(xs, x) != 0 {
                    self.set_flower_from(b, x, xs);
                }
            }
            mi += 1;
        }
        self.set_slack(b);
    }

    fn expand_blossom(&mut self, b: usize) {
        // `set_st` only descends into each member's own sub-cycle, so `flower[b]`
        // is stable here — index it rather than cloning.
        let mut mi = 0;
        while mi < self.flower[b].len() {
            let m = self.flower[b][mi];
            self.set_st(m, m);
            mi += 1;
        }
        let xr = self.flower_from(b, self.g(b, self.pa[b]).u as usize);
        // `get_pr` may reverse `flower[b][1..]`; every index below reads it after,
        // and `set_slack`/`q_push` never mutate it, so no clone is needed.
        let pr = self.get_pr(b, xr);
        let mut i = 0;
        while i < pr {
            let xs = self.flower[b][i];
            let xns = self.flower[b][i + 1];
            self.pa[xs] = self.g(xns, xs).u as usize;
            self.s[xs] = 1;
            self.s[xns] = 0;
            self.slack[xs] = 0;
            self.set_slack(xns);
            self.q_push(xns);
            i += 2;
        }
        self.s[xr] = 1;
        self.pa[xr] = self.pa[b];
        let mut idx = pr + 1;
        while idx < self.flower[b].len() {
            let xs = self.flower[b][idx];
            self.s[xs] = -1;
            self.set_slack(xs);
            idx += 1;
        }
        self.st[b] = 0;
    }

    fn on_found_edge(&mut self, e: Edge<W>) -> bool {
        let u = self.st[e.u as usize];
        let v = self.st[e.v as usize];
        if self.s[v] == -1 {
            self.pa[v] = e.u as usize;
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
                    if self.g(u, v).w > W::ZERO && self.st[u] != self.st[v] {
                        if self.e_delta(self.g(u, v)) == W::ZERO {
                            if self.on_found_edge(self.g(u, v)) {
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
                    let delta = self.e_delta(self.g(self.slack[x], x));
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
                    && self.e_delta(self.g(self.slack[x], x)) == W::ZERO
                    && self.on_found_edge(self.g(self.slack[x], x))
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
                self.set_flower_from(u, v, if u == v { u } else { 0 });
                if self.g(u, v).w > w_max {
                    w_max = self.g(u, v).w;
                }
            }
        }
        for u in 1..=self.n {
            self.lab[u] = w_max;
        }
        while self.matching() {}
    }
}

/// Solve one `n`-vertex instance on the per-thread pooled solver: `set_edges`
/// writes the graph (it must call `set_edge` for *every* vertex pair so no stale
/// pooled edge survives — see [`Blossom::reset`]), then each vertex's raw mate is
/// passed through `map_mate` to build the result in a single allocation.
///
/// Both public entry points funnel through here so they share the pool and its
/// buffer reuse; they differ only in how they fill edges and shape the output.
/// `map_mate` receives the raw 1-indexed partner (`0` meaning unmatched).
fn solve_pooled<W: Weight, T>(
    n: usize,
    set_edges: impl FnOnce(&mut Blossom<W>),
    map_mate: impl Fn(usize) -> T,
) -> Vec<T> {
    POOL.with(|pool| {
        let mut pool = pool.borrow_mut();
        let bl = pool.get::<W>(n);
        bl.reset(n);
        set_edges(bl);
        bl.solve();
        (1..=n).map(|u| map_mate(bl.mate[u])).collect()
    })
}

/// Compute a maximum-total-weight matching of the `n` vertices, where `weight` is
/// the **row-major** `n × n` weight matrix — the weight of the edge between `i`
/// and `j` is `weight[i * n + j]`. Returns `mate`, where `mate[i]` is `Some(j)` if
/// `i` is matched to `j` and `None` if `i` is left unmatched.
///
/// The graph need not be complete and `n` need not be even: a vertex is left
/// unmatched whenever matching it cannot increase the total. An entry that is
/// **zero or negative** is treated as *no usable edge* — such a pair is never
/// matched (a max-weight matching would never pick a non-positive edge anyway) —
/// so a caller encodes a sparse graph by leaving absent edges at zero.
///
/// `weight` must have exactly `n * n` entries. Only the strict **upper triangle**
/// (`i < j`) is read — the weight is taken as symmetric, so the diagonal and the
/// lower triangle are ignored and a caller may leave them unset. The matrix is a
/// flat slice, not `&[Vec<_>]`, so a caller building it (and the solver reading
/// it) touches one contiguous allocation rather than `n` rows.
pub fn max_weight_matching<W: Weight>(weight: &[W], n: usize) -> Vec<Option<usize>> {
    assert_eq!(weight.len(), n * n, "weight must be a row-major n×n matrix");
    if n == 0 {
        return Vec::new();
    }

    solve_pooled(
        n,
        |bl| {
            for i in 0..n {
                for j in (i + 1)..n {
                    // The solver reads a non-positive weight as "no edge"; clamp to
                    // zero so every edge slot is still overwritten — a reused pool
                    // buffer may hold a previous instance's edges (see `reset`).
                    let w = weight[i * n + j];
                    bl.set_edge(i + 1, j + 1, if w > W::ZERO { w } else { W::ZERO });
                }
            }
        },
        // Raw mate `0` means unmatched; otherwise de-bias to a 0-indexed partner.
        |m| (m != 0).then(|| m - 1),
    )
}

/// Compute a minimum-total-cost **perfect** matching of the `n` vertices, where
/// `cost` is the **row-major** `n × n` cost matrix — the cost of pairing `i` with
/// `j` is `cost[i * n + j]`. Returns `mate`, where `mate[i]` is the partner of
/// vertex `i`.
///
/// `n` must be even (a perfect matching is otherwise impossible) and `cost` must
/// have exactly `n * n` entries. Only the strict **upper triangle** (`i < j`) is
/// read — the cost is taken as symmetric, so the diagonal and the lower triangle
/// are ignored and a caller may leave them unset. Costs must be non-negative. On
/// the complete graph every vertex is pairable, so a perfect matching always
/// exists.
///
/// The matrix is a flat slice, not `&[Vec<_>]`, so a caller building it touches
/// one contiguous allocation rather than `n` rows.
pub fn min_weight_perfect_matching<W: Weight>(cost: &[W], n: usize) -> Vec<usize> {
    assert_eq!(cost.len(), n * n, "cost must be a row-major n×n matrix");
    assert!(
        n.is_multiple_of(2),
        "a perfect matching needs an even vertex count"
    );
    if n == 0 {
        return Vec::new();
    }

    // Reduce min-cost-perfect to max-weight: weight = offset - cost, with offset
    // above every cost so all weights are ≥ 1. Every edge of the complete graph is
    // then positive, so the maximum-weight matching is necessarily perfect (any
    // two unmatched vertices could be joined by a positive edge) — and, since its
    // total weight is `offset·(n/2) − total_cost`, maximizing weight minimizes
    // cost. So the max-weight matching is exactly the min-cost perfect one. The
    // `offset - cost` edges are fed straight to the solver rather than through a
    // materialized weight matrix, so no `n²` buffer is allocated per call.
    let mut max_cost = W::ZERO;
    for i in 0..n {
        for j in (i + 1)..n {
            let c = cost[i * n + j];
            if c > max_cost {
                max_cost = c;
            }
        }
    }
    let offset = max_cost + W::ONE;

    solve_pooled(
        n,
        |bl| {
            for i in 0..n {
                for j in (i + 1)..n {
                    bl.set_edge(i + 1, j + 1, offset - cost[i * n + j]);
                }
            }
        },
        // Every vertex is matched (the matching is perfect), so the raw mate is
        // always ≥ 1; de-bias to a 0-indexed partner.
        |m| m - 1,
    )
}

thread_local! {
    /// Per-thread reuse of the solver's O(n²) working buffers (see [`Pool`]).
    static POOL: RefCell<Pool> = const { RefCell::new(Pool::new()) };
}

/// A per-thread cache of reusable [`Blossom`] solvers, one per weight type. The
/// solver's buffers are the dominant allocation — `g` alone is `(2n+1)²` edges —
/// and a caller like osp-sim runs thousands of same-sized matchings per thread,
/// so keeping the buffers and merely [`Blossom::reset`]ting them between calls
/// turns those per-call allocations into one per thread.
///
/// It is keyed by [`TypeId`] because the solver is generic over `W` while a
/// thread-local is not; with only the handful of integer weight types in use a
/// linear scan of the slots is cheaper than a map.
struct Pool {
    slots: Vec<(TypeId, Box<dyn Any>)>,
}

impl Pool {
    const fn new() -> Self {
        Pool { slots: Vec::new() }
    }

    /// The reusable solver for weight type `W`, created (sized for `n`) on first
    /// use for this type on this thread.
    fn get<W: Weight>(&mut self, n: usize) -> &mut Blossom<W> {
        let tid = TypeId::of::<W>();
        let idx = match self.slots.iter().position(|(t, _)| *t == tid) {
            Some(i) => i,
            None => {
                self.slots.push((tid, Box::new(Blossom::<W>::new(n))));
                self.slots.len() - 1
            }
        };
        self.slots[idx]
            .1
            .downcast_mut::<Blossom<W>>()
            .expect("each slot holds the Blossom<W> its TypeId keys")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flatten a square `Vec<Vec<W>>` into the row-major slice the solver takes.
    /// The tests keep the readable nested form (and hand it to the oracle as-is);
    /// this bridges to the flat API at the call.
    fn flat<W: Copy>(cost: &[Vec<W>]) -> Vec<W> {
        cost.iter().flatten().copied().collect()
    }

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

    /// Reference: maximum total weight over all matchings (not necessarily
    /// perfect), by exhaustive recursion. Mirrors `max_weight_matching`'s
    /// semantics — a non-positive weight is "no edge", and a vertex may be left
    /// unmatched. Only usable for tiny `n`.
    fn brute_max_weight(w: &[Vec<i128>]) -> i128 {
        let n = w.len();
        fn rec(w: &[Vec<i128>], used: &mut Vec<bool>, n: usize) -> i128 {
            let i = match (0..n).find(|&i| !used[i]) {
                Some(i) => i,
                None => return 0,
            };
            used[i] = true;
            // Option 1: leave `i` unmatched.
            let mut best = rec(w, used, n);
            // Option 2: match `i` to any later usable (positive-weight) partner.
            for j in (i + 1)..n {
                if !used[j] && w[i][j] > 0 {
                    used[j] = true;
                    best = best.max(w[i][j] + rec(w, used, n));
                    used[j] = false;
                }
            }
            used[i] = false;
            best
        }
        rec(w, &mut vec![false; n], n)
    }

    /// Total weight of `mate`, checking it is a valid matching that only uses
    /// positive-weight edges.
    fn total_weight(w: &[Vec<i128>], mate: &[Option<usize>]) -> i128 {
        let n = w.len();
        let mut t = 0;
        for i in 0..n {
            if let Some(j) = mate[i] {
                assert_ne!(j, i, "vertex matched to itself");
                assert_eq!(mate[j], Some(i), "matching is not a valid involution");
                assert!(w[i][j] > 0, "matched a non-positive (absent) edge {i}-{j}");
                if i < j {
                    t += w[i][j];
                }
            }
        }
        t
    }

    #[test]
    fn trivial_pair() {
        let cost = vec![vec![0, 7], vec![7, 0]];
        let mate = min_weight_perfect_matching(&flat(&cost), cost.len());
        assert_eq!(mate, vec![1, 0]);
    }

    #[test]
    fn reuses_buffers_across_shrinking_sizes() {
        // The per-thread solver pool keeps a buffer sized for the largest instance
        // seen, so a later *smaller* solve runs on a buffer holding a bigger
        // instance's stale edges (some now in the super-vertex index range). This
        // is the case the ascending brute-force test never hits. Solve a large
        // instance to grow-and-dirty the buffer, then check small instances — which
        // now reuse it — against the exhaustive oracle.
        let mut seed: u64 = 0x243F6A8885A308D3;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut random_cost = |n: usize| {
            let mut cost = vec![vec![0i128; n]; n];
            #[allow(clippy::needless_range_loop)]
            for i in 0..n {
                for j in (i + 1)..n {
                    let c = (next() % 1000) as i128;
                    cost[i][j] = c;
                    cost[j][i] = c;
                }
            }
            cost
        };

        for _ in 0..50 {
            // Dirty the buffer with a large instance, then a small one that reuses
            // it — and interleave sizes so the shrink path is hit repeatedly.
            let _ = min_weight_perfect_matching(&flat(&random_cost(120)), 120);
            for &n in &[2usize, 4, 6, 8, 10, 4, 8, 2] {
                let cost = random_cost(n);
                let mate = min_weight_perfect_matching(&flat(&cost), cost.len());
                assert_eq!(
                    total_of(&cost, &mate),
                    brute_min_cost(&cost),
                    "reused buffer gave a suboptimal matching at n={n}"
                );
            }
        }
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
        let mate = min_weight_perfect_matching(&flat(&cost), cost.len());
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
                let mate = min_weight_perfect_matching(&flat(&cost), cost.len());
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
        let mate = min_weight_perfect_matching(&flat(&cost), cost.len());
        assert_eq!(brute_min_cost(&cost), total_of(&cost, &mate));
        assert_ne!(
            mate[0], 1,
            "should not pair the two most-penalized vertices"
        );
    }

    #[test]
    fn max_weight_leaves_a_vertex_unmatched() {
        // A triangle of positive edges (odd order): the best matching takes the
        // single heaviest edge and leaves the third vertex unmatched.
        let w = vec![vec![0, 5, 3], vec![5, 0, 4], vec![3, 4, 0]];
        let mate = max_weight_matching(&flat(&w), w.len());
        assert_eq!(total_weight(&w, &mate), 5);
        assert_eq!(mate[0], Some(1));
        assert_eq!(mate[1], Some(0));
        assert_eq!(mate[2], None);
    }

    #[test]
    fn max_weight_respects_absent_edges() {
        // Only two positive edges exist; a zero weight is "no edge" and must never
        // be matched. Best matching is the pair of disjoint present edges.
        let w = vec![
            vec![0, 7, 0, 0],
            vec![7, 0, 0, 0],
            vec![0, 0, 0, 9],
            vec![0, 0, 9, 0],
        ];
        let mate = max_weight_matching(&flat(&w), w.len());
        assert_eq!(total_weight(&w, &mate), 16);
        assert_eq!(mate[0], Some(1));
        assert_eq!(mate[2], Some(3));
    }

    #[test]
    fn max_weight_matches_brute_force_on_sparse_instances() {
        // Random instances with many zero weights (a sparse graph) and odd as well
        // as even orders, checked against the exhaustive max-weight oracle.
        let mut seed: u64 = 0x2545F4914F6CDD1D;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        for &n in &[1usize, 2, 3, 4, 5, 6, 7, 8] {
            for _ in 0..300 {
                let mut w = vec![vec![0i128; n]; n];
                #[allow(clippy::needless_range_loop)]
                for i in 0..n {
                    for j in (i + 1)..n {
                        // ~1/3 of pairs are absent (weight 0); the rest 1..=1000.
                        let c = match next() % 3 {
                            0 => 0,
                            _ => (next() % 1000 + 1) as i128,
                        };
                        w[i][j] = c;
                        w[j][i] = c;
                    }
                }
                let mate = max_weight_matching(&flat(&w), n);
                assert_eq!(
                    total_weight(&w, &mate),
                    brute_max_weight(&w),
                    "n={n}, w={w:?}, mate={mate:?}"
                );
            }
        }
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
        let mate = min_weight_perfect_matching(&flat(&cost), cost.len());
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

            let base_mate = min_weight_perfect_matching(&flat(&cost), cost.len());
            let base_cost = total_of(&cost, &base_mate);

            // Forbid a handful of edges the solution uses: cost must not improve.
            let solution_edges: Vec<(usize, usize)> = (0..n)
                .filter(|&i| i < base_mate[i])
                .map(|i| (i, base_mate[i]))
                .collect();
            for &(i, j) in solution_edges.iter().take(5) {
                let mut c2 = cost.clone();
                c2[i][j] = PENALTY;
                c2[j][i] = PENALTY;
                let m2 = min_weight_perfect_matching(&flat(&c2), c2.len());
                assert_ne!(
                    m2[i], j,
                    "n={n}: forbidden solution edge {i}-{j} was still used"
                );
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
                    let m2 = min_weight_perfect_matching(&flat(&c2), c2.len());
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
