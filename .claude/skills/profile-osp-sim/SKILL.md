---
name: profile-osp-sim
description: How to run and profile the osp-sim Monte-Carlo simulator on this repo — timed benchmarks, a macOS sampling profile that finds hot functions and attributes allocation churn, and a synthetic-tournament scaling benchmark for how cost grows with field size N (the blossom solver's O(N^3)). Use when asked to profile osp-sim, measure its speed or scaling, find where time or allocations go, or verify a performance change.
---

# Profiling osp-sim

The benchmark is `osp-sim` running the **WOSC 2024** tournament many times.
`osp-sim` is deterministic at a fixed `--seed`, so its stdout is a valid
correctness anchor: a byte-identical diff before/after a change proves you
didn't alter behavior.

WOSC (~107 players) is the fixed baseline for a *before/after* comparison. To
measure how cost grows with **field size N** — the O(N³) question for the blossom
solver — use synthetic tournaments instead (§5); the real corpus tops out around
100 players.

All commands run from the repo root. Put scratch files in your session
scratchpad, not the repo.

## 1. Build with debug symbols

Release optimization but with symbols for line-level attribution:

```sh
CARGO_PROFILE_RELEASE_DEBUG=true cargo build --release -p osp-sim
```

(`Cargo.toml` has no `[profile.release] debug`, so you must pass this env var
each time; plain `cargo build --release -p osp-sim` gives faster builds but
function-name-only symbolication.)

## 2. Timed benchmark

The standard invocation (the `--results` file supplies both the players and
each player's true strength, so no `--strength*` flag is needed):

```sh
RAYON_NUM_THREADS=1 /usr/bin/time -l \
  ./target/release/osp-sim --results "test_files/WOSC 2024.txt" --runs 1000 --seed 0 \
  > scratch/out.txt 2> scratch/time.txt
grep -E "real|user|maximum resident" scratch/time.txt
```

Pitfalls that make numbers non-comparable:
- **Threads.** osp-sim uses rayon and runs multi-threaded by default (`user` ≫
  `real`). **Always pin `RAYON_NUM_THREADS=1`** for a stable, comparable
  single-thread wall time. Multi-threaded is only for "how fast in practice"
  (~1.3s vs ~5s single-thread).
- **`--runs`.** Wall time scales with it. Keep it fixed (1000) across
  comparisons — changing runs is the #1 cause of a fake speedup/regression.
- Use the same `--seed 0` every time.

## 3. Correctness anchor (do this around any perf change)

```sh
# baseline before the change, candidate after:
diff -q scratch/baseline.txt scratch/candidate.txt && echo "byte-identical"
```

Output is byte-identical regardless of thread count (multithreaded ==
`RAYON_NUM_THREADS=1`, and repeatable across processes), so either works as an
anchor — but still prefer `RAYON_NUM_THREADS=1` so the *timing* context
matches §2. (This once wasn't true: the pooled blossom solver leaked stale
state between solves on a worker thread, flipping near-tie summary lines under
rayon scheduling — fixed in integer-blossom 1.4.1 alongside canonical tie
orders in the sim, guarded by the
`pooled_solver_matches_a_fresh_thread_exactly` test. If a multithreaded anchor
ever mismatches its single-thread twin again, first check that fix is in your
build.) Also regenerate the baseline from committed source in the same session
— don't trust an anchor file from an earlier working-tree build.

## 4. Sampling profile (where the time goes)

`sample` (built into macOS) symbolicates the release binary. Launch a run long
enough to outlast the sample window, skip startup, then sample:

```sh
RAYON_NUM_THREADS=1 ./target/release/osp-sim \
  --results "test_files/WOSC 2024.txt" --runs 40000 --seed 0 > /dev/null 2>&1 &
PID=$!
sleep 2                        # skip process startup
sample $PID 25 -f scratch/sample.txt
kill $PID 2>/dev/null
```

Ignore `__psynch_cvwait` in the results — that's the parked rayon pool threads
waiting, not CPU work.

### Read self-time (which functions burn CPU)

```sh
awk '/Sort by top of stack/{f=1} f' scratch/sample.txt | head -50
```

### Inclusive time per function (robust parse of the tree)

The call-graph tree uses `.!:|+` drawing chars before the count. To get the max
inclusive sample count for named functions:

```sh
CG=$(awk '/Call graph:/{f=1} /Total number in stack/{f=0} f' scratch/sample.txt)
for fn in confirm_round_inner pair_round_weighted min_weight_perfect \
          on_found_edge compute_scores edge_units estimate_elos standings; do
  n=$(printf '%s\n' "$CG" | grep -F "$fn" | sed -E 's/^[ .!:|+]*//' \
        | awk '{print $1}' | grep -E '^[0-9]+$' | sort -rn | head -1)
  printf "%-28s %s\n" "$fn" "${n:-0}"
done
```

To find which *source line* of `simulate_run` dominates (the per-run phases),
grep the call graph for `simulate_run` — the biggest inclusive count points at
the hot call site (e.g. `sim.rs:590` = `confirm_round_unordered` = pairing).

### Attribute allocation churn to callers

Raw `malloc`/`free` samples are scattered; this script rolls each allocation
frame up to its nearest osp_core / integer_blossom / osp_sim caller:

```sh
python3 .claude/skills/profile-osp-sim/attrib_alloc.py scratch/sample.txt
```

## 5. Scaling with field size (synthetic tournaments)

To measure how cost grows with N, profile fake tournaments from
`scripts/gen_fake_tournament.py`. It samples real FESA players (their joined
pre/post ELO plus per-player attendance) and emits an ordinary FESA result file,
so the graphs the solver sees stay realistic well past the corpus's ~100-player
ceiling. See the script's docstring for why the base's own results don't matter
(osp-sim re-pairs from scratch).

**Prerequisite: the sampling table** `test_files/fesa_results/fesa_elo_pairs.json`.
It is gitignored (it travels in the `fesa_results.zip` archive, not git). If it's
missing, either unzip that archive so the tree lands back at
`test_files/fesa_results/`, or regenerate the pipeline (step 1 fetches ~3800
files from fesashogi.eu, so it needs network and a few minutes):

```sh
python scripts/fesa_fetch_all.py          # 1. fetch+convert every FESA season
python scripts/fesa_filter_valid.py       # 2. move files osp-sim rejects to invalid/ (~2.7%)
python scripts/fesa_extract_elo_pairs.py  # 3. pool the per-player sampling table
python scripts/fesa_anomalies.py          # 4. (optional) refresh ANOMALIES.md
```

Step 4 rewrites `test_files/fesa_results/ANOMALIES.md`: every corpus file the
importer rejects or has to work around, grouped by defect, to report upstream.

**Generate a size series** — fixed rounds and seed, so only N varies:

```sh
for N in 50 100 200 400 800; do
  python scripts/gen_fake_tournament.py $N --rounds 9 --seed 0 --out scratch/fake_$N.txt
done
```

**Time each** exactly as §2 (single-thread, fixed `--runs`, fixed `--seed`):

```sh
for N in 50 100 200 400 800; do
  RAYON_NUM_THREADS=1 /usr/bin/time -l \
    ./target/release/osp-sim --results scratch/fake_$N.txt --runs 200 --seed 0 \
    > /dev/null 2> scratch/t_$N.txt
  printf "N=%-4s " $N; grep real scratch/t_$N.txt
done
```

- A run does ~N/2 boards × 9 rounds, so wall time per run already grows with N;
  fewer `--runs` than WOSC's 1000 is fine, but keep it **fixed across the
  series** (changing it is the #1 cause of a fake trend). Fit `time/run` vs N on
  a log-log scale — slope ≈ 3 if the matching dominates as expected.
- To see *where* the time goes at a chosen N, point the §4 sampling profile at a
  `fake_$N.txt` instead of WOSC — the pairing share should climb with N.
- Both generation and osp-sim are deterministic at a fixed `--seed`, so the
  series is reproducible. Keep `--rounds` ≤ ~12 (deeper than any real event makes
  synthetic attendance flatten); irrelevant to an N-series, which fixes R.

## 6. Capture-replay: solver benchmarks on the real graphs

To attribute time *inside* the matching solver on the exact cost matrices
osp-sim produces, use `examples/bench.rs` — captures are its only diet
(synthetic families were tried and removed: they mispredicted the real cost
shape badly; see below):

```sh
# capture: one file per solve (9 rounds = 9 files for --runs 1); single
# thread so the sequence numbering is canonical
RAYON_NUM_THREADS=1 OSP_MATCHING_DUMP=scratch/cap_swiss \
  ./target/release/osp-sim --results scratch/fake_1000.txt --runs 1 --seed 0 > /dev/null
# replay: add --features stats for the region/counter breakdown
cargo run --release -p integer-blossom --example bench -- scratch/cap_swiss
```

Use one capture directory per config. Files are `solve_*_n{N}.ospm`
(~16·N²/1e6 MB each); the replay reports per-(n, weight-width) ms/solve, where
the width mirrors osp-core's adaptive narrowing — so it also shows which
integer width real instances exercise (measured at N≈1000: Swiss/MacMahon
round 1 `i32`, later rounds `i128`; pure-ELO mostly `i64`).

Known real-graph shape (captured 2026-07-19, N≈900–940): Swiss — greedy seed
covers ~97% of pairs, `add_blossom` ~45% and the dual block ~60% of solve
time, scan only ~23%; MacMahon — same family, ~2× the phases/rows of Swiss;
pure-ELO — scan 53% / blossom-formation 35%. (The removed synthetic families
had predicted a scan-dominated shape and ~6× the real Swiss wall time.)

## Known shape of the profile (baseline, as of the tid-native refactor)

Use these as a sanity check that your measurement is sane:
- **Pairing is ~74% of runtime** — `confirm_round_unordered` → `pair_round_weighted`.
- Within pairing, the **blossom `min_weight_perfect_matching` solver is the
  dominant cost** (~50%+ of total). It allocates almost nothing (reusable
  buffers). Further speedups have to attack the matching algorithm itself — and
  §5 measures its N-scaling directly (its share should grow with field size).
- **Allocation churn is dominated by `compute_scores`** rebuilding a fresh
  `Scores` every call (~10×/run) — its `index: HashMap<Uuid,u32>` and the
  push-grown `opponents`/`defeated` `Vec`s. Scoring/standings/ELO CPU time is
  each <1%.
