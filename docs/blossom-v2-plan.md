# Blossom v2 plan: sparse core with preserved trees

Target: the two structural levers left after the 1.3.0/1.4.0 constant-factor
work on [`integer-blossom`](../crates/matching/src/lib.rs) — **sparsification**
(shrink each scan from O(n) to O(k)) and **tree preservation across
augmentations** (shrink the number of scans) — combined into one staged rewrite.

## Why these two, and why together

Measured (stats build, n=1000, i128 lex family, 2026-07-19):

- The queue-drain edge scan is ~80% of solve time, and it is bound by **row
  count × streaming floor**: ~58k O(n) row-sweeps per solve (~350 per phase ×
  165 phases). Per-row micro-optimization is a measured dead end (±0%).
- `add_blossom` is 14–24% — almost entirely the O(n_x) contracted-row merge,
  ~650 blossom formations per solve.
- Everything else is single digits.

Sparsification divides the *width* of every sweep (n → k); preservation divides
the *count* (one sweep per vertex state-change instead of one per vertex per
phase). They multiply, and they share one prerequisite: replacing the dense
matrix + materialized contracted rows with an **edge list + per-vertex
adjacency (CSR) + candidate-edge slack tracking**. That shared core is Stage A;
sparsification is Stage B; preservation is Stage C. Each stage lands green on
`main` with its own gates.

Cost model (scan entries streamed per lex n=1000 solve):
now ≈ 58k×1000 = 58M → after B ≈ 58k×k ≈ 1M (+ n²/2 ≈ 500k certificate pass)
→ after C ≈ ~10k×k ≈ 160k (+ certificate). Ceiling ~50× on the scan bucket;
overall wall-time hypothesis ~5–10× at n=1000 (Amdahl: `add_blossom` shrinks
with it; init + certificate become the floor). Hypotheses, not promises — each
stage's gate re-measures.

---

## Stage A — edge-list core (same algorithm, new representation)

Replace the dense `gw`/`gu`/`gv` matrices with:

```
ew: Vec<W>,   eu: Vec<Vid>, ev: Vec<Vid>   // edge arrays; edge id = index
adj_start: Vec<u32>, adj_edge: Vec<u32>    // CSR over vertices 1..=n; every
                                           // edge appears in both endpoints' lists
cand: Vec<u32>                             // per component x: best-known edge id
                                           // from an S-vertex into x (NONE = !0)
```

Semantics mapping from the current code:

- `slack_of(e) = lab[eu[e]] + lab[ev[e]] − ew[e]` (weights stay ×4-prescaled;
  all parity/integrality invariants carry over unchanged — they are properties
  of labels and weights, not of storage).
- `slack: Vec<usize>` (best S-*vertex* per component, needing the stored
  `(u, x)` best-edge slot) becomes `cand: Vec<u32>` (best S-*edge* per
  component). `update_slack(u, x)` → `update_cand(x, e)`: keep the incumbent
  unless `slack_of(e)` is strictly smaller. `set_slack(x)` (component rebuild)
  → iterate the adjacency of x's members, filtering to edges whose source side
  has `s[st[src]] == 0`.
- **`add_blossom` loses its row-merge entirely** (the 14–24% bucket): a new
  blossom needs no contracted row, only a `cand[b]` rebuild via its members'
  adjacency — Σ member degrees, same order as the old merge on dense input,
  O(members × k) on sparse. `flower_from` stays as-is (membership queries).
- The scan of vertex u walks `adj[u]`: for each edge, `v = other endpoint`,
  `x = st[v]`; skip if `st[u] == x`; tight → `on_found_edge` (reconstruct the
  real edge from `eu/ev/ew`), else `update_cand(x, e)`.
- The d-computation reads `slack_of(cand[x])`. Validity of `cand` entries is
  unchanged from today's `slack[]`: within a phase S-vertices stay S, and the
  phase-init reset clears everything (staleness only becomes possible in
  Stage C, which adds lazy validation).
- New outcome plumbing: `dual_adjust` returning `d == inf` in perfect mode
  becomes a distinct `Stuck` result (impossible on complete graphs — assert —
  but Stage B's subgraphs need it). Today's `Some(false)` max-weight abort is
  untouched.

Entry points: the public dense API is unchanged; the dense matrix is loaded as
a CSR of all n(n−1)/2 edges. Memory is comparable to the SoA matrices.

**Gates**: all existing oracle/metamorphic/pool tests pass (totals only —
tie-breaks will shift, so osp-sim anchors are re-baselined, single-thread, as
for 1.3.0); bench families **plus a new n=100 point** (real tournaments live
there) within ~10% of 1.4.0 on the dense path; stats show the `add_blossom`
share collapsed; `scan_rows` unchanged (preservation comes later).

## Stage B — sparsification + dual-certificate loop

Pipeline inside `min_weight_perfect_matching` (public signature unchanged),
active above a size threshold (~n ≥ 128, benched; below it, full-graph CSR):

1. **Candidate graph**: any deterministic per-vertex edge set — the seed
   carries **no correctness burden** (steps 3–5 guarantee exactness whatever
   it contains), only a convergence-speed one, so it need not be uniform-k
   and should lean on caller structure. Two seeders:
   - *Solver default* (dense entry, property tests): per vertex, the k
     cheapest incident edges (deterministic `(cost, index)` order; k = 16
     initially, tuned by bench), symmetrized by union.
   - *Caller-supplied* via a new `min_weight_perfect_matching_seeded(cost, n,
     seed_edges)` entry (full cost matrix still required — certificate and
     densify touch arbitrary pairs). osp-core's seeder: per player, the ~8
     cheapest in-points-group pairs plus the ~4 cheapest one-step ascender
     and ~4 descender pairs — ranked by the *real ladder cost* (rematch/club
     penalties included), degrading to whole-group for small groups and to
     plain ELO-distance k-NN in pure-ELO mode. Union symmetrization makes
     multi-step float cascades decompose into covered one-step edges;
     expected convergence 1–2 iterations.
2. **Solve** on the subgraph (Stage A core, perfect mode).
3. **Stuck** (some root exhausts, `d == inf`): the subgraph has no perfect
   matching or its duals can't progress — double k (rebuild, cold re-solve);
   cap at dense. Deterministic and terminating.
4. **Certificate pass** over all omitted pairs (i, j): optimal iff every
   omitted edge has non-negative reduced cost. For `st[i] != st[j]` this is
   `lab[i] + lab[j] − w'(i,j) ≥ 0`. For pairs inside a common final blossom the
   constraint includes the blossom duals: reduced cost gains
   `Σ lab[B]` over all blossoms B containing both i and j (walk the nested
   `flower`/`flower_from` structure from `st[i]` down to the innermost common
   blossom). **The exact scale factor of the z-term is the one correctness
   trap in this plan** — derive it on paper, then lock it in with the tests
   below before trusting it.
5. **Violations** → append those edges to the CSR, cold re-solve, repeat from
   2. Terminates (edge set grows monotonically toward dense); measured
   expectation ≤ 2–3 iterations.

The certificate pass streams n²/2 weights once (~1 ms at n=1000) — it becomes
the floor, on par with greedy init. That's fine.

**Gates**: a new property test — *sparse total == dense total* — on random
instances of both bench families at n up to ~300, **including band-structured
instances engineered to end with large nested blossoms and omitted
intra-blossom edges** (this is what catches a wrong z-term); a stuck/densify
path test (clustered odd groups whose k-NN graph is infeasible); bench: elo and
lex n=1000 substantially faster, n=100 not regressed; stats counters for
`cert_iters`, `cert_violations`, effective k.

## Stage C — tree preservation across augmentations

The deepest change; only start once A+B are stable.

- **Forest becomes persistent.** On augmentation through trees T₁, T₂: only
  their vertices reset to `s = -1`; every other tree keeps its S/T labels,
  parents, and queue-position. Requires per-component `tree_id` (set on
  joining the forest) to enumerate the two trees being dissolved.
- **`cand` single slots become per-component lazy heaps.** Entries `(key,
  edge)`; an entry is valid iff its source is still an S-vertex (checked on
  pop). The load-bearing design fact making plain binary heaps sufficient —
  no Blossom-V per-tree/per-pair queues — is the **global-d invariant**: all
  trees adjust by the same d simultaneously, so within one heap every live
  entry's slack drifts by the same amount and *relative order is stable*
  (source S-labels all move −d together; the target component's state applies
  to the whole heap at once; dissolved sources are lazily invalid).
  Debug-assert popped-key monotonicity; a debug "recompute best by scan and
  compare" pass guards the whole scheme.
- **Heap lifecycle**: blossom formation melds member heaps (append smaller
  into larger); expansion rebuilds the members' heaps by adjacency scan
  (bounded, same as today's `set_slack` accounting).
- **Queue discipline**: only newly-S vertices are pushed (grow steps, blossom
  formation, expansion). Dissolution pushes nothing — surviving trees' scan
  state is exactly what preservation preserves. `scan_rows` collapses from
  O(n·phases) to O(n + state churn).
- **Label updates** stay explicit O(n) per adjustment initially (measured
  ~2%); if they surface after the scans shrink, add the classic clock-offset
  representation (`lab_stored` + per-vertex join time, materialized on read)
  as a follow-up C2 — do not build it speculatively.
- Interaction with Stage B: none structurally — each certificate iteration
  runs its own internally-preserved solve; cold restarts between iterations
  stay (they are rare).

**Gates**: oracle/metamorphic/equivalence tests; `scan_rows`/solve on lex
n=1000 down ≥5× (~58k → ≤ ~10k); wall-time targets (hypotheses): lex n=1000
< ~150 ms, elo < ~80 ms from today's 607/345; n=100 not regressed; osp-sim
3-config N=1000 re-run + anchors re-baselined.

---

## Cross-cutting

- **Determinism & anchors**: every stage changes tie-breaking; totals are the
  invariant, byte-anchors re-baseline per stage (single-thread only — the
  multithread ~1/1000 divergence is a separate open bug). The crate's oracle +
  metamorphic + new equivalence tests carry the correctness burden; osp-sim
  anchor is a smoke test, not the proof.
- **Testing ladder per stage**: unit (oracle n ≤ 10, exhaustive) → metamorphic
  (n ≤ 200) → sparse≡dense equivalence (Stage B+) → osp-sim ST smoke →
  bench + stats gates.
- **Risk register**:
  1. z-term scale factor in the certificate (Stage B step 4) — wrong ⇒
     silently accepts suboptimal matchings. Mitigated by blossom-heavy
     equivalence tests + a debug full-LP check on small n.
  2. Heap-order drift assumption (Stage C) broken by an unforeseen state
     transition ⇒ wrong d ⇒ stall or suboptimal. Mitigated by debug
     monotonicity + recompute-and-compare passes, fuzzing.
  3. n≈100 regression from CSR indirection (real tournaments!). Mitigated by
     the n=100 gate and the dense-threshold fallback.
  4. Effort asymmetry: A and B are mechanical-ish; C is the hairy one. A+B
     alone already deliver the row-width win — C can be deferred if B's
     numbers satisfy.
- **Out of scope, noted**: osp-core builds full O(n²) i128 cost matrices per
  round before the solver ever runs — once the solver is sparse, a direct
  sparse entry from `pairing.rs` (build only seeded candidate costs) is the
  natural next step, but it is an osp-core change, not part of this plan.
  The path there is **block-pruning the certificate**: a violation at (i, j)
  means an omitted edge is cheaper than the pair's dual budget
  (`cost(i,j) < offset − (lab[i]+lab[j])/4`), so with per-group maxima of
  `lab` and caller-known lower bounds on cross-group costs (float penalties
  grow with group distance), whole group×group blocks are excluded in O(1) —
  making the certificate, and hence the whole pipeline, sub-quadratic in
  rule evaluations. Requires a cost-oracle API instead of a materialized
  matrix; design it only after Stage B's measured iteration counts confirm
  the seeded convergence.
