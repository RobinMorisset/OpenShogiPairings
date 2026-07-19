# Changelog

All notable changes to `integer-blossom` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) over its own public API
(independent of the OpenShogiPairings application it lives alongside).

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
