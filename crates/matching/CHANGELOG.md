# Changelog

All notable changes to `integer-blossom` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) over its own public API
(independent of the OpenShogiPairings application it lives alongside).

## 1.3.0 - 2026-07-19

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
- **Headroom requirement tightened**: because of the internal ×4 scaling, a
  weight type now needs roughly three bits of headroom above the largest raw
  weight (previously roughly two). Callers near the top of `i32`/`i64` should
  widen; typical lexicographic `i128` stacks are unaffected.
- Tie-breaking among equally-optimal matchings may differ from 1.2.x (results
  remain optimal and deterministic).

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
