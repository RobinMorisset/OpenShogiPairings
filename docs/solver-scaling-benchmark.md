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
figure, and it is stable rather than climbing toward 3. Why it sits below the N³
worst case is not investigated here.

Absolute ms are machine-specific; the **exponent** is the portable result.

---

## Run 2 — MacMahon (grade bands, ≥10/group) + est-ELO + tuned Laplace + half-point absences

Same instances, series, and method as Run 1 — only the pairing config differs.
Each instance is paired under a per-instance config from `mm-grades <file> 10`
(variant `c`), with `half_point_absences` added. In the nested `PairingMode` shape
osp-sim deserializes (`pairing.macmahon.{thresholds,source}`):

- **MacMahon thresholds** — on FESA grade boundaries, every group ≥ 10 players.
  Group count grows with the field: 4 / 7 / 13 / 17 / 20 / 20 for
  N = 50 / 100 / 200 / 400 / 800 / 1000 (saturating ~20 as the field outgrows the
  grade boundaries above the floor).
- **`source: from_estimate`** — group by the ELO estimated from results, re-fit
  each round.
- **`k_multiplier: 0`** — pin rated/provisional players, estimate the unrated only.
  Load-bearing: the default is 100, so it must be set explicitly.
- **Tuned Laplace unrated prior** — `prior_shape_unrated: laplace`,
  `unrated_prior_center: 700`, `unrated_k: 260`, `upward_looseness_unrated: 300` —
  the settings-tab "Tuned Laplace" preset verbatim.
- **`half_point_absences: true`** — an absent player scores ½.

> **Correction (supersedes the first Run 2).** The originally-logged Run 2 used
> `mm-grades`' old *flat* config layout (`macmahon_thresholds`, `elo_*`, … at top
> level). That layout predates the `PairingMode` enum refactor; osp-sim nests those
> keys under `pairing` now and drops unknown ones, so the config was silently
> ignored and the run actually measured **default Swiss + half-point absences**
> (≈ 3.6/8.2/30.7/112.6/538.3/865.4 ms/run, ~1.1× Run 1). `mm-grades` now emits the
> nested shape; the numbers below are the real config.

`ms/run` is compared against Run 1 (default Swiss) at the same N:

| N | ms/run | 95% CI | rel. err | Run 1 (Swiss) | ratio |
|---:|---:|:---:|---:|---:|---:|
| 50 | 6.73 | [6.5, 7.0] | 3.8% | 3.38 | 1.99 |
| 100 | 15.33 | [15.1, 15.5] | 1.3% | 7.69 | 1.99 |
| 200 | 58.56 | [58.0, 59.2] | 1.0% | 27.18 | 2.15 |
| 400 | 236.05 | [232.9, 239.2] | 1.3% | 101.27 | 2.33 |
| 800 | 1218.21 | [1203.4, 1233.0] | 1.2% | 469.93 | 2.59 |
| 1000 | 1984.08 | [1958.0, 2010.1] | 1.3% | 749.97 | 2.65 |

**Scaling exponent** (log-log fit of ms/run vs N):

- All 6 points: **b = 1.95** (R² = 0.990)
- N ≥ 200 only: **b = 2.21**
- Local slopes: 50→100: 1.19 · 100→200: 1.93 · 200→400: 2.01 · 400→800: 2.37 · 800→1000: 2.19

**Reading it.** Estimate-based MacMahon is markedly heavier than default Swiss,
and the overhead grows with N — from ~2.0× at N ≤ 100 to ~2.65× at N = 1000 (a
growing, not constant, premium). The log-log exponent is **≈ N²·² at N ≥ 200**
(local slope peaking 2.37 at 400→800), somewhat above Run 1's ≈ N²·¹ and still far
from N³. The cause of the extra cost and steeper slope is not investigated here.

---

## Planned: further configurations

Candidate configs for later runs: floater/club constraints, the hybrid cup,
degressive MacMahon (`drops_after_round`), and **Run 3: pure ELO pairing**
(`pairing.kind = elo`) + the tuned-Laplace unrated prior — a prior experiment
found it ~4× slower at N=100, so recalibrate at N=1000 before a full run. Each
logged as a new "Run N — …" section with the same method, comparable to Runs 1–2.
