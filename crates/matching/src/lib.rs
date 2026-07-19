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
/// hold the caller's largest weight without overflow. Weights are pre-scaled
/// **×4** internally (see [`Edge`]), and slacks sum two duals, so the type
/// needs roughly three bits of headroom above the largest raw weight.
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

/// Solver instrumentation, compiled only under the `stats` feature (off by
/// default — the hot path carries zero instrumentation otherwise). **Not a
/// stable API**: this exists for profiling the solver's internal cost shape
/// (see `examples/bench.rs --features stats`) and may change freely.
///
/// Counters and region timers accumulate per thread; [`stats::take`] returns
/// and resets them. Region times are attributed at the algorithm's structural
/// boundaries; the doc on each field says what it covers and which fields
/// *overlap* (a sub-region timed inside an enclosing region is included in
/// both — subtract to get exclusive figures).
#[cfg(feature = "stats")]
pub mod stats {
    use std::cell::RefCell;
    use std::time::Duration;

    /// Accumulated solver statistics for the current thread.
    #[derive(Default, Clone, Debug)]
    pub struct Stats {
        /// Completed `solve` calls.
        pub solves: u64,
        /// Phases run (calls to the phase loop that had at least one root).
        pub phases: u64,
        /// Dual adjustments applied (iterations of the phase loop's dual block).
        pub adjustments: u64,
        /// Pairs matched by the greedy tight-edge seed.
        pub greedy_pairs: u64,
        /// Augmentations (successful `on_found_edge` outcomes).
        pub augments: u64,
        /// Blossoms formed / expanded.
        pub blossoms_formed: u64,
        pub blossoms_expanded: u64,
        /// `set_slack` invocations (each is an O(n) column scan).
        pub set_slack_calls: u64,
        /// `solve` initialization: state resets, row-maxima duals, greedy seed.
        pub t_init: Duration,
        /// Per-phase re-initialization: `s`/`slack` resets and root queue build.
        pub t_phase_init: Duration,
        /// The queue-drain edge scan, *including* `on_found_edge` calls made
        /// from it (subtract [`Stats::t_ofe_scan`] for the pure scan).
        pub t_scan: Duration,
        /// `on_found_edge` time at the scan-loop call site (inside `t_scan`).
        pub t_ofe_scan: Duration,
        /// The dual block: `d` computation, label updates, post-adjustment
        /// tight-edge re-scan, and blossom expansions (inclusive of
        /// [`Stats::t_expand`] and any augment/blossom work the re-scan finds).
        pub t_dual: Duration,
        /// `augment` bodies (inside `t_scan` or `t_dual` via `on_found_edge`).
        pub t_augment: Duration,
        /// `add_blossom` bodies, inclusive of their `set_slack` call.
        pub t_add_blossom: Duration,
        /// `expand_blossom` bodies, inclusive of their `set_slack` calls.
        pub t_expand: Duration,
        /// All `set_slack` bodies (inside `t_add_blossom`/`t_expand`).
        pub t_set_slack: Duration,
    }

    thread_local! {
        static STATS: RefCell<Stats> = RefCell::default();
    }

    /// Run `f` on this thread's accumulator.
    pub fn with<R>(f: impl FnOnce(&mut Stats) -> R) -> R {
        STATS.with(|s| f(&mut s.borrow_mut()))
    }

    /// Take this thread's accumulated stats, resetting them to zero.
    pub fn take() -> Stats {
        with(std::mem::take)
    }
}

/// Bump a stats counter: `stat!(|s| s.phases += 1)`. Expands to nothing
/// without the `stats` feature.
#[cfg(feature = "stats")]
macro_rules! stat {
    ($f:expr) => {
        crate::stats::with($f)
    };
}
#[cfg(not(feature = "stats"))]
macro_rules! stat {
    ($f:expr) => {};
}

/// Time `$body`, accumulating into the named [`stats::Stats`] duration field.
/// Expands to plain `$body` without the `stats` feature. `$body` must not
/// `return`/`break` out of the macro (the accumulation would be skipped) —
/// bind the result and branch outside instead.
#[cfg(feature = "stats")]
macro_rules! timed {
    ($field:ident, $body:expr) => {{
        let __start = std::time::Instant::now();
        let __r = $body;
        crate::stats::with(|s| s.$field += __start.elapsed());
        __r
    }};
}
#[cfg(not(feature = "stats"))]
macro_rules! timed {
    ($field:ident, $body:expr) => {
        $body
    };
}

/// A vertex index. The algorithm works in `usize` (loop counters, array indices),
/// but the large `O(n²)` tables — the [`Blossom::gu`]/[`Blossom::gv`] endpoint
/// matrices and [`Blossom::flower_from`] — *store* vertices, and for the field
/// sizes we pair (hundreds) a 2-byte id keeps those tables small and their scans
/// cheap. Stored as `Vid`, used as `usize`. A stored value can be a blossom index
/// (up to `2n`), so the supported instance size is `2n ≤ u16::MAX`, i.e.
/// **n ≤ 32767** — asserted at the public entry points, and far above any
/// realistic tournament field.
type Vid = u16;

/// One graph edge, carrying the *real* endpoints it stands for. For a super-vertex
/// (contracted blossom) `b`, slot `(b, x)` records the best underlying real edge,
/// so `u`/`v` are always real-vertex indices even for a super-vertex slot.
///
/// This is a *materialized view*: edges are stored column-split across the
/// [`Blossom::gw`]/[`Blossom::gu`]/[`Blossom::gv`] matrices (so the hot
/// weight-only scans never touch endpoint bytes), and an `Edge` is assembled by
/// [`Blossom::g`] only on the paths that need the endpoints.
///
/// `w` holds the caller's weight **pre-scaled by 4** (see [`Blossom::set_edge`]):
/// the ×2 every slack computation needs is baked in at write time instead of
/// being recomputed in [`Blossom::e_delta`] (the hottest expression), and the
/// second ×2 keeps every initial dual — including the per-vertex duals the
/// perfect-matching path uses — **even**, which is the parity invariant that
/// keeps all slacks halvable without truncation.
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
    /// The `sz × sz` edge matrices, row-major in single allocations: entry
    /// `(u, v)` is at `u * stride + v`. Structure-of-arrays: `gw` holds the
    /// (pre-scaled) weights, `gu`/`gv` the real endpoints of the edge each slot
    /// stands for. The hot scans (`matching`'s edge loop, the
    /// `set_slack`/`update_slack` column scans) read only `gw` — and `gv` when a
    /// blossom column makes the far endpoint non-implied — so splitting the
    /// weights from the endpoints cuts the bytes those loops stream by 2–3×
    /// versus an array-of-`Edge` layout (which also carried padding at `i128`).
    /// For a slot `(u, v)` with `u, v ≤ n` both real, the endpoints are always
    /// exactly `(u, v)` (blossom formation only rewrites super-vertex rows and
    /// columns), so the real–real fast paths never load `gu`/`gv` at all.
    gw: Vec<W>,
    gu: Vec<Vid>,
    gv: Vec<Vid>,
    /// Row stride of `gw`/`gu`/`gv` — the allocated width `2 * n_cap + 1` for the
    /// largest instance seen, so a reused buffer keeps a consistent layout.
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
    /// Whether this solve seeks a *perfect* matching (the min-cost reduction) or
    /// the general max-weight matching. It selects the dual initialization and
    /// the termination rule — see [`Blossom::solve`].
    perfect: bool,
}

impl<W: Weight> Blossom<W> {
    fn new(n: usize) -> Self {
        let sz = 2 * n + 1;
        Blossom {
            n,
            n_x: n,
            gw: vec![W::ZERO; sz * sz],
            gu: vec![0; sz * sz],
            gv: vec![0; sz * sz],
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
            perfect: false,
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
            // is nothing to copy). The matrices are laid out at the new stride from
            // here on.
            self.stride = sz;
            self.gw = vec![W::ZERO; sz * sz];
            self.gu = vec![0; sz * sz];
            self.gv = vec![0; sz * sz];
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

    /// Materialize edge `(u, v)` from the split matrices — the general accessor,
    /// for paths that need the real endpoints (either index may be a blossom).
    #[inline]
    fn g(&self, u: usize, v: usize) -> Edge<W> {
        let i = u * self.stride + v;
        Edge {
            u: self.gu[i],
            v: self.gv[i],
            w: self.gw[i],
        }
    }

    /// Weight of slot `(u, v)` alone — the hot accessor; touches no endpoint bytes.
    #[inline]
    fn w_at(&self, u: usize, v: usize) -> W {
        self.gw[u * self.stride + v]
    }

    /// Slack of slot `(u, x)` for a **real** row `u ≤ n` (so the near endpoint is
    /// `u` itself) and an arbitrary column `x`: loads the weight and only the far
    /// endpoint. Equivalent to `e_delta(self.g(u, x))` minus the `gu` load.
    #[inline]
    fn slack_at(&self, u: usize, x: usize) -> W {
        let i = u * self.stride + x;
        debug_assert_eq!(self.gu[i] as usize, u, "row {u} is not the near endpoint");
        self.lab[u] + self.lab[self.gv[i] as usize] - self.gw[i]
    }

    /// Store `e` into slot `(a, b)` of the split matrices.
    #[inline]
    fn set_g(&mut self, a: usize, b: usize, e: Edge<W>) {
        let i = a * self.stride + b;
        self.gu[i] = e.u;
        self.gv[i] = e.v;
        self.gw[i] = e.w;
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
        // Stored pre-scaled ×4 (see `Edge`): `e_delta` then needs no doubling,
        // and initial duals (half a stored row-maximum) are always even.
        let w = w.double().double();
        let (u, v) = (u as Vid, v as Vid);
        self.set_g(u as usize, v as usize, Edge { u, v, w });
        // Reverse orientation: endpoints swapped (field-named, not positional).
        self.set_g(v as usize, u as usize, Edge { u: v, v: u, w });
    }

    /// Reduced cost (slack) of an edge; zero means the edge is tight. The
    /// stored weight is pre-scaled (see [`Blossom::set_edge`]), so this is a
    /// plain add/sub — it is the hottest expression in the solver.
    fn e_delta(&self, e: Edge<W>) -> W {
        self.lab[e.u as usize] + self.lab[e.v as usize] - e.w
    }

    fn update_slack(&mut self, u: usize, x: usize) {
        if self.slack[x] == 0 || self.slack_at(u, x) < self.slack_at(self.slack[x], x) {
            self.slack[x] = u;
        }
    }

    fn set_slack(&mut self, x: usize) {
        stat!(|s| s.set_slack_calls += 1);
        timed!(t_set_slack, {
            self.slack[x] = 0;
            for u in 1..=self.n {
                if self.w_at(u, x) > W::ZERO && self.st[u] != x && self.s[self.st[u]] == 0 {
                    self.update_slack(u, x);
                }
            }
        })
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
        timed!(
            t_augment,
            loop {
                let xnv = self.st[self.mate[u]];
                self.set_match(u, v);
                if xnv == 0 {
                    break;
                }
                let next_u = self.st[self.pa[xnv]];
                self.set_match(xnv, next_u);
                u = next_u;
                v = xnv;
            }
        )
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
        stat!(|s| s.blossoms_formed += 1);
        timed!(t_add_blossom, self.add_blossom_inner(u, lca, v))
    }

    fn add_blossom_inner(&mut self, u: usize, lca: usize, v: usize) {
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
            // Weight zero marks the slot unset; the stale endpoints beside it are
            // never read (every reader checks the weight, or the copy loop below
            // overwrites the slot on its first member before comparing).
            self.gw[b * self.stride + x] = W::ZERO;
            self.gw[x * self.stride + b] = W::ZERO;
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
                let gbx = self.g(b, x);
                if gbx.w == W::ZERO || self.e_delta(gxsx) < self.e_delta(gbx) {
                    let gxxs = self.g(x, xs);
                    self.set_g(b, x, gxsx);
                    self.set_g(x, b, gxxs);
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
        stat!(|s| s.blossoms_expanded += 1);
        timed!(t_expand, self.expand_blossom_inner(b))
    }

    fn expand_blossom_inner(&mut self, b: usize) {
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
                stat!(|s| s.augments += 1);
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
        timed!(t_phase_init, {
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
        });
        if self.q.is_empty() {
            return false;
        }
        stat!(|s| s.phases += 1);
        loop {
            // The queue-drain edge scan. The labeled block (rather than a
            // `return` inside) lets the `timed!` accumulation complete before
            // an augmentation exits the phase.
            let augmented = timed!(t_scan, 'scan: {
                while let Some(u) = self.q.pop_front() {
                    if self.s[self.st[u]] == 1 {
                        continue;
                    }
                    // `u` (from the queue) and `v` are both real vertices, so slot
                    // `(u, v)` always has implied endpoints: the scan reads only the
                    // contiguous `gw` row plus `lab`/`st`, never `gu`/`gv`. `st[u]`
                    // is re-read per iteration — `on_found_edge` may contract `u`
                    // into a fresh blossom mid-scan.
                    let row = u * self.stride;
                    for v in 1..=self.n {
                        let w = self.gw[row + v];
                        if w > W::ZERO && self.st[u] != self.st[v] {
                            if self.lab[u] + self.lab[v] - w == W::ZERO {
                                let e = Edge {
                                    u: u as Vid,
                                    v: v as Vid,
                                    w,
                                };
                                let hit = timed!(t_ofe_scan, self.on_found_edge(e));
                                if hit {
                                    break 'scan true;
                                }
                            } else {
                                let x = self.st[v];
                                self.update_slack(u, x);
                            }
                        }
                    }
                }
                false
            });
            if augmented {
                return true;
            }
            // The dual block: pick `d`, shift labels, re-scan the now-tight
            // edges, expand spent blossoms. `Some(r)` exits the phase with `r`.
            let outcome = timed!(t_dual, self.dual_adjust());
            if let Some(r) = outcome {
                return r;
            }
        }
    }

    /// One dual adjustment of the current phase (the phase loop's non-scan
    /// half). Returns `Some(true)` when the re-scan augments, `Some(false)` on
    /// the max-weight zero-dual termination, `None` to continue the phase.
    fn dual_adjust(&mut self) -> Option<bool> {
        stat!(|s| s.adjustments += 1);
        {
            let mut d = W::inf();
            for b in (self.n + 1)..=self.n_x {
                if self.st[b] == b && self.s[b] == 1 {
                    d = d.min(self.lab[b].half());
                }
            }
            for x in 1..=self.n_x {
                if self.st[x] == x && self.slack[x] != 0 {
                    let delta = self.slack_at(self.slack[x], x);
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
                        // Max-weight termination: an outer dual about to hit 0
                        // means its root is best left unmatched. Sound only
                        // because uniform initialization keeps every root's
                        // dual in lockstep (they reach 0 together). The perfect
                        // path skips it — its LP has unrestricted duals, and
                        // its tighter per-vertex initialization gives roots
                        // *different* duals, under which aborting on the first
                        // zero would wrongly strand still-matchable roots.
                        if !self.perfect && self.lab[u] <= d {
                            return Some(false);
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
                    && self.slack_at(self.slack[x], x) == W::ZERO
                    && self.on_found_edge(self.g(self.slack[x], x))
                {
                    return Some(true);
                }
            }
            for b in (self.n + 1)..=self.n_x {
                if self.st[b] == b && self.s[b] == 1 && self.lab[b] == W::ZERO {
                    self.expand_blossom(b);
                }
            }
        }
        None
    }

    /// Solve the instance currently in `g`. `perfect` selects the perfect-
    /// matching mode (tighter per-vertex dual initialization, no zero-dual
    /// termination) versus the general max-weight mode (uniform duals, whose
    /// lockstep root trajectories make the zero-dual termination sound).
    fn solve(&mut self, perfect: bool) {
        stat!(|s| s.solves += 1);
        timed!(t_init, self.solve_init(perfect));
        while self.matching() {}
    }

    /// `solve`'s pre-phase work: state resets, dual initialization, greedy seed.
    fn solve_init(&mut self, perfect: bool) {
        self.perfect = perfect;
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
        // Row maxima, gathered into `lab` (they become the duals below).
        for u in 1..=self.n {
            let mut m = W::ZERO;
            let row = u * self.stride;
            for v in 1..=self.n {
                self.set_flower_from(u, v, if u == v { u } else { 0 });
                let w = self.gw[row + v];
                if w > m {
                    m = w;
                }
            }
            self.lab[u] = m;
        }
        if perfect {
            // Per-vertex initialization: half the (×4-scaled) row maximum, the
            // tightest feasible symmetric dual — `lab[u] + lab[v] ≥ w` holds
            // because each side is at least half the edge's own weight, and a
            // mutual-maximum pair starts exactly tight, feeding the greedy
            // seed below. Halving a ×4 weight is exact and even, so all duals
            // share a parity and every slack stays halvable.
            for u in 1..=self.n {
                self.lab[u] = self.lab[u].half();
            }
        } else {
            // Uniform initialization, required by the zero-dual termination
            // (see `matching`): every dual starts at half the global maximum.
            let mut w_max = W::ZERO;
            for u in 1..=self.n {
                if self.lab[u] > w_max {
                    w_max = self.lab[u];
                }
            }
            let w_max = w_max.half();
            for u in 1..=self.n {
                self.lab[u] = w_max;
            }
        }
        // Greedy seed: match any two unmatched vertices joined by an initially
        // tight edge. Each pair found is an augmenting path of length one the
        // phase loop no longer has to grow a forest to discover, and matching
        // only tight edges preserves complementary slackness, so the phases
        // continue correctly from this primal state.
        for u in 1..=self.n {
            if self.mate[u] != 0 {
                continue;
            }
            let row = u * self.stride;
            for v in 1..=self.n {
                // Real–real slot: endpoints implied, slack straight from `gw`.
                let w = self.gw[row + v];
                if v != u
                    && self.mate[v] == 0
                    && w > W::ZERO
                    && self.lab[u] + self.lab[v] - w == W::ZERO
                {
                    stat!(|s| s.greedy_pairs += 1);
                    self.mate[u] = v;
                    self.mate[v] = u;
                    break;
                }
            }
        }
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
    perfect: bool,
    set_edges: impl FnOnce(&mut Blossom<W>),
    map_mate: impl Fn(usize) -> T,
) -> Vec<T> {
    POOL.with(|pool| {
        let mut pool = pool.borrow_mut();
        let bl = pool.get::<W>(n);
        bl.reset(n);
        set_edges(bl);
        bl.solve(perfect);
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
    assert!(
        2 * n <= Vid::MAX as usize,
        "instance too large: the solver stores vertex ids as u16, supporting n ≤ 32767"
    );
    if n == 0 {
        return Vec::new();
    }

    solve_pooled(
        n,
        false,
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
    assert!(
        2 * n <= Vid::MAX as usize,
        "instance too large: the solver stores vertex ids as u16, supporting n ≤ 32767"
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
        true,
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
/// solver's buffers are the dominant allocation — the edge matrices alone are
/// three `(2n+1)²` tables —
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
    fn perfect_matching_survives_asymmetric_duals() {
        // Regression guard for the perfect path's termination rule. With the
        // tight per-vertex dual initialization, roots start with *different*
        // duals; the max-weight zero-dual abort (still active for
        // `max_weight_matching`) would fire here on vertex 0 — whose edges are
        // all expensive, so its dual starts tiny — while a perfect matching
        // still needs it matched. The perfect path must ignore that abort and
        // keep adjusting duals (negative if need be) until the matching is
        // perfect. Constructed so that after the greedy seed pairs 2-3 (the
        // mutually-cheapest pair), roots 0 and 1 remain with very different
        // initial duals.
        let cost = vec![
            vec![0, 100, 96, 100],
            vec![100, 0, 51, 100],
            vec![96, 51, 0, 1],
            vec![100, 100, 1, 0],
        ];
        let mate = min_weight_perfect_matching(&flat(&cost), cost.len());
        assert_eq!(total_of(&cost, &mate), brute_min_cost(&cost));
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
