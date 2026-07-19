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

## Planned: complex configurations

The next runs repeat this exact series with richer `--configs` (MacMahon bands,
floater/club constraints, hybrid cup, …) to see whether denser or more
constrained graphs push the exponent up toward N³. Each will be logged here as a
new "Run N — …" section with the same method, so they are directly comparable to
Run 1's default-Swiss baseline.
