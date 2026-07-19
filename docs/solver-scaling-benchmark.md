# Solver scaling benchmark

How the pairing cost scales with **field size N**. The matching in
[`integer-blossom`](../crates/matching/src/lib.rs) is O(N³) in the worst case;
this measures what it actually does on realistic tournament graphs, which the real
corpus can't reach (its largest event is ~100 players).

Instances are synthetic tournaments from
[`scripts/gen_fake_tournament.py`](../scripts/gen_fake_tournament.py), which
samples real FESA players (joined pre/post ELO + per-player attendance) so the
graphs stay realistic well past 100 players. See the `profile-osp-sim` skill (§5)
for the method this follows.

## Method

Common to every run below unless a section says otherwise:

| | |
|---|---|
| Date | 2026-07-19 |
| Machine | AMD Ryzen 9 3900X (12C/24T), Windows 10 |
| Binary | `target/release/osp-sim` (plain release, no debug symbols) |
| Commit | `2483e1b`, osp-sim 1.2.0 |
| Threads | **single-thread** (`RAYON_NUM_THREADS=1`) for comparable per-run wall time |
| Rounds | 9 (`--rounds 9`) |
| Generation seed | 0 (one fixed instance per N) |
| Timing | `--runs 10`, fixed across the series |
| Repeats | K=10 per N, each with a **disjoint** seed (`--seed r*10`), so the CI reflects real run-to-run spread, not just OS jitter |
| CI | 95%, t-based (df=9) on the per-run wall time |

**Metric.** Total `osp-sim` per-run wall time = (invocation wall) / `--runs`.
This is the whole pipeline (pairing + result model + standings + `estimate_elos`),
not the solver in isolation — but pairing is the dominant term (see the skill's
"known shape"), so the exponent is mostly the solver's.

Reproduce:

```sh
for N in 50 100 200 400 800 1000; do
  python scripts/gen_fake_tournament.py $N --rounds 9 --seed 0 --out scratch/fake_$N.txt
done
# then, per N, K=10 timed invocations at --runs 10 with --seed 0,10,20,…,90
# (RAYON_NUM_THREADS=1), per-run = wall/10; mean ± t·s/√K across the K repeats.
```

---

## Run 1 — default Swiss rules

**Configuration: default Swiss.** No `--configs` was passed, so the single variant
is the generated tournament's own `TournamentSettings::default()` — plain Swiss
pairing with **no MacMahon bands, default floater handling, no club/cup
constraints**. This is the baseline the complex-configuration runs (below) will be
compared against.

| N | ms/run | 95% CI | rel. err |
|---:|---:|:---:|---:|
| 50 | 3.38 | [3.08, 3.69] | 8.9% |
| 100 | 7.69 | [7.53, 7.86] | 2.1% |
| 200 | 27.18 | [26.67, 27.69] | 1.9% |
| 400 | 101.27 | [99.91, 102.63] | 1.3% |
| 800 | 469.93 | [458.38, 481.48] | 2.5% |
| 1000 | 749.97 | [723.00, 776.95] | 3.6% |

**Scaling exponent** (log-log fit of ms/run vs N):

- All 6 points: **b = 1.85** (R² = 0.991)
- N ≥ 200 only: **b = 2.08**
- Local slopes: 50→100: 1.19 · 100→200: 1.82 · 200→400: 1.90 · 400→800: 2.21 · 800→1000: 2.09

**Reading it.** Over N = 50–1000 the cost scales like **≈ N²·¹**, not N³. The low
end is dragged down by fixed per-run/startup cost amortized over only 10 runs
(50→100 slope 1.2), so the N ≥ 200 fit (~2.1) is the trustworthy asymptotic
figure, and it is stable rather than climbing toward 3.

This sub-N³ behavior comes from the **weights**, not the graph size. The pairing
graph is **complete** — every pair of players is an edge, with forbidden pairings
(rematches, float violations, …) carried as large penalty weights rather than
removed — so E = O(N²) regardless. What keeps the solver below its worst case is
how those weights are distributed on realistic fields: the score+rating structure
makes good augmenting paths easy to find, so the primal-dual search forms few
blossoms and converges in far fewer steps than an adversarial weighting would
force. The N³ bound is a worst-case guarantee; on realistic default-Swiss weight
distributions the solver behaves closer to N².

Absolute ms are machine-specific; the **exponent** is the portable result.

---

## Run 2 — MacMahon (grade bands, ≥10/group) + est-ELO + tuned Laplace + half-point absences

Same instances, series, and method as Run 1 — only the pairing config differs.
Each instance is paired under a per-instance config from `mm-grades <file> 10`
(variant `c`), with `half_point_absences` added:

- **`macmahon_thresholds`** — on FESA grade boundaries, every group ≥ 10 players.
  Group count grows with the field: 4 / 7 / 13 / 17 / 20 / 20 for
  N = 50 / 100 / 200 / 400 / 800 / 1000 (saturating ~20 as the field outgrows the
  grade boundaries above the floor).
- **`macmahon_from_estimated_elo: true`** — group by the ELO estimated from
  results, re-fit each round.
- **`elo_k_multiplier_percent: 0`** — pin rated/provisional players, estimate the
  unrated only. Load-bearing: the default is 100, so it must be set explicitly.
- **Tuned Laplace unrated prior** — `elo_prior_shape_unrated: laplace`,
  `elo_unrated_prior_center: 700`, `elo_unrated_k: 260`,
  `elo_upward_looseness_unrated_percent: 300` — the settings-tab "Tuned Laplace"
  preset verbatim.
- **`half_point_absences: true`** — an absent player scores ½.

`ms/run` is compared against Run 1 (default Swiss) at the same N:

| N | ms/run | 95% CI | rel. err | Run 1 (Swiss) | ratio |
|---:|---:|:---:|---:|---:|---:|
| 50 | 3.56 | [3.22, 3.90] | 9.6% | 3.38 | 1.05 |
| 100 | 8.23 | [8.04, 8.42] | 2.3% | 7.69 | 1.07 |
| 200 | 30.74 | [30.42, 31.05] | 1.0% | 27.18 | 1.13 |
| 400 | 112.57 | [110.88, 114.26] | 1.5% | 101.27 | 1.11 |
| 800 | 538.33 | [528.20, 548.45] | 1.9% | 469.93 | 1.15 |
| 1000 | 865.36 | [855.73, 875.00] | 1.1% | 749.97 | 1.15 |

**Scaling exponent** (log-log fit of ms/run vs N):

- All 6 points: **b = 1.88** (R² = 0.992)
- N ≥ 200 only: **b = 2.09**
- Local slopes: 50→100: 1.21 · 100→200: 1.90 · 200→400: 1.87 · 400→800: 2.26 · 800→1000: 2.13

**Reading it.** The richer rule set costs a roughly **constant ~10–15% overhead**
at N ≥ 200 (the per-round est-ELO fit and MacMahon grouping), but the **exponent
is unchanged — still ≈ N²·¹**, matching Run 1 within noise. MacMahon reshapes the
edge *weights* (score bands from estimated ELO) but the graph is still complete
and the weight distribution still keeps the blossom solver off its worst case; the
added work is per-round bookkeeping proportional to the field, i.e. a
multiplicative constant, not a higher-order term. So on realistic fields a denser,
more constrained config buys the referee its behavior at a constant-factor price,
not a worse scaling class.

---

## Planned: further configurations

Later runs can push on the parts most likely to bend the exponent — floater/club
constraints and the hybrid cup (which add structured penalty weights), or
degressive MacMahon (`drops_after_round`) — each logged as a new "Run N — …"
section with the same method, comparable to Runs 1–2.
