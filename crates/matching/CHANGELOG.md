# Changelog

All notable changes to `integer-blossom` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) over its own public API
(independent of the OpenShogiPairings application it lives alongside).

## 1.3.0 - 2026-07-19

### Added
- `examples/bench.rs`: a dependency-free replay benchmark over **captured real
  instances** (`cargo run --release --example bench -- <dir>`, reading the
  `.ospm` blobs written by osp-core's `OSP_MATCHING_DUMP` hook), reported per
  (n, weight-width) with the same adaptive width selection as osp-core.
  Synthetic instance families were tried and removed: measured against
  captures they mispredicted the real cost shape badly.
- `stats` feature (off by default, **not a stable API**): per-thread counters
  and region timers over the solver's internal phases, printed by
  `examples/bench.rs` when built with `--features stats`. Zero hot-path cost
  when disabled.

### Changed
- Much faster solves, especially on smooth cost surfaces (up to ~3× end-to-end
  in the osp-sim ELO-pairing benchmark at N=1000):
  - Tighter dual initialization: `min_weight_perfect_matching` starts each
    vertex's dual at half its own best edge weight (per-vertex) instead of a
    uniform global maximum; `max_weight_matching` keeps the uniform start its
    zero-dual termination rule requires.
  - Greedy seed: initially-tight edges between unmatched vertices are matched
    before the first phase, skipping one full alternating-tree phase per pair.
  - Edge weights are stored pre-scaled ×4, removing the doubling from the slack
    computation (the innermost hot expression) and keeping all initial duals
    even (the parity invariant behind exact slack halving).
  - Structure-of-arrays edge storage: three parallel matrices (weights / near
    endpoints / far endpoints) instead of an array of `Edge` structs. The hot
    scans read only the weight matrix (plus the far endpoint on blossom
    columns), cutting the bytes they stream by 2–3×.
- Vertex ids in the big `O(n²)` tables shrank from `u32` to `u16`, halving the
  endpoint and `flower_from` tables. **New documented limit: n ≤ 32767**,
  asserted at both entry points (far above any realistic field; ids must hold
  blossom indices up to `2n`).
- **Headroom requirement tightened**: because of the internal ×4 scaling, a
  weight type now needs roughly three bits of headroom above the largest raw
  weight (previously roughly two). Callers near the top of `i32`/`i64` should
  widen; typical lexicographic `i128` stacks are unaffected.
- Tie-breaking among equally-optimal matchings may differ from 1.2.x (results
  remain optimal and deterministic).

### Fixed
- Pooled solves are **history-independent**: which of several cost-tied
  optimal matchings is returned no longer depends on what the same thread's
  reused solver buffers held from earlier solves. Blossom formation wrote an
  intra-blossom member edge's weight onto the super-vertex's diagonal slot,
  and a later larger solve read that stale diagonal in its row-maximum dual
  initialization; a weight-0 (absent) edge could also enter the blossom
  best-edge comparison through stale endpoint bytes. Results were always
  optimal, but callers running many solves per thread (osp-sim under rayon)
  saw work-stealing-dependent tie choices. Guarded by the
  `pooled_solver_matches_a_fresh_thread_exactly` test.

## 1.2.1 - 2026-07-19

### Fixed
- Restored the `max_weight_matching` summary on docs.rs. A refactor had left its
  doc comment attached to a private helper, so the function rendered without a
  description. Docs only — no API or behavior change.
- Corrected the `repository` metadata URL (wrong GitHub username), so the
  crates.io "Repository" link resolves.

## 1.2.0 - 2026-07-19

Initial crates.io release. (Earlier `1.x` numbers belong to the surrounding
application; the crate's published history starts here.)

### Added
- `max_weight_matching`: maximum-total-weight matching on an arbitrary graph —
  possibly sparse (a zero/negative entry is "no edge") and possibly of odd order,
  leaving a vertex unmatched where that is optimal.
- `min_weight_perfect_matching`: minimum-total-cost perfect matching on a complete
  graph, as the reduction onto the maximum-weight solver.
- Generic integer edge weights via the `Weight` trait (`i32`, `i64`, `i128`).
- Per-thread solver pool that reuses the `O(n²)` working buffers across calls.
