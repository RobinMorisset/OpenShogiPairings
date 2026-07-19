# integer-blossom

Minimum-weight **perfect** matching on a complete, general (non-bipartite) graph,
with integer edge weights.

Many pairing problems — Swiss-style tournament pairing among them — are matching
problems on a *general* graph: any vertex may pair with any other, so bipartite
methods (the Hungarian algorithm) don't apply. This crate implements the classic
`O(V³)` primal–dual blossom algorithm for **maximum**-weight matching, and also
exposes the problem most callers actually want — a **minimum-weight perfect**
matching — as a reduction on top of it.

The implementation is original — built from the published blossom algorithm, not
ported from any existing codebase — and is checked against a brute-force oracle and
metamorphic relations in the test suite.

## Features

- **Maximum-weight matching** on an arbitrary graph — possibly sparse, possibly of
  odd order — leaving a vertex unmatched where that is optimal.
- **Minimum-cost perfect matching** on a complete graph via a single call, built
  as a reduction on top of the maximum-weight solver.
- **Generic integer weights** (`i32`, `i64`, `i128`): pick a type just wide enough
  for your largest weight. `i128` gives room to stack large lexicographic
  multipliers when scalarizing a multi-criteria cost.
- **No dependencies** — `std` only.
- **Buffer reuse**: a per-thread solver pool keeps the `O(n²)` working buffers
  around, so running many same-sized matchings on a thread (e.g. a Monte-Carlo
  simulation) allocates once rather than once per call.

## Usage

### Minimum-cost perfect matching

```rust
use integer_blossom::min_weight_perfect_matching;

// 4 vertices. Costs as a row-major n×n matrix (only the strict upper triangle
// i < j is read; the cost is taken as symmetric). Costs must be non-negative,
// and n must be even.
let n = 4;
let cost = [
    0, 1, 10, 10,
    1, 0, 10, 10,
    10, 10, 0, 1,
    10, 10, 1, 0,
];

let mate = min_weight_perfect_matching(&cost, n);

// Pairs up the cheap edges: {0-1} and {2-3}.
assert_eq!(mate[0], 1);
assert_eq!(mate[2], 3);
```

`min_weight_perfect_matching` returns a `Vec<usize>` where `mate[i]` is the partner
of vertex `i`. On a complete graph every vertex is pairable, so a perfect matching
always exists as long as `n` is even.

### Maximum-weight matching

```rust
use integer_blossom::max_weight_matching;

// A triangle of 3 vertices. Weights as a row-major n×n matrix (upper triangle,
// symmetric). A zero or negative entry means "no edge", so a sparse graph is
// encoded by leaving absent edges at zero. n need not be even.
let n = 3;
let weight = [
    0, 5, 3,
    5, 0, 4,
    3, 4, 0,
];

let mate = max_weight_matching(&weight, n);

// Takes the single heaviest edge {0-1} and leaves vertex 2 unmatched.
assert_eq!(mate[0], Some(1));
assert_eq!(mate[2], None);
```

`max_weight_matching` returns a `Vec<Option<usize>>` where `mate[i]` is `Some(j)`
if `i` is matched to `j`, or `None` if `i` is left unmatched.

## How it works

On a complete graph with strictly positive edge weights, the maximum-weight
matching is necessarily perfect (any two unmatched vertices are joined by a
positive-weight edge, so leaving them unmatched is never optimal). Weighting each
edge `offset - cost`, with `offset` chosen above every cost so all weights stay
`≥ 1`, therefore turns the minimum-cost perfect matching into a maximum-weight
matching, which the blossom solver computes directly.

## Minimum supported Rust version

Rust 1.87.

## License

MIT
