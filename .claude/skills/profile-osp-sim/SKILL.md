---
name: profile-osp-sim
description: How to run and profile the osp-sim Monte-Carlo simulator on this repo — timed benchmarks and a macOS sampling profile that finds hot functions and attributes allocation churn. Use when asked to profile osp-sim, measure its speed, find where time or allocations go, or verify a performance change.
---

# Profiling osp-sim

The benchmark is `osp-sim` running the **WOSC 2024** tournament many times.
`osp-sim` is deterministic at a fixed `--seed`, so its stdout is a valid
correctness anchor: a byte-identical diff before/after a change proves you
didn't alter behavior.

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

Determinism is thread-independent, so multi-thread output must equal
single-thread output — a quick self-check.

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

## Known shape of the profile (baseline, as of the tid-native refactor)

Use these as a sanity check that your measurement is sane:
- **Pairing is ~74% of runtime** — `confirm_round_unordered` → `pair_round_weighted`.
- Within pairing, the **blossom `min_weight_perfect_matching` solver is the
  dominant cost** (~50%+ of total). It allocates almost nothing (reusable
  buffers). Further speedups have to attack the matching algorithm itself.
- **Allocation churn is dominated by `compute_scores`** rebuilding a fresh
  `Scores` every call (~10×/run) — its `index: HashMap<Uuid,u32>` and the
  push-grown `opponents`/`defeated` `Vec`s. Scoring/standings/ELO CPU time is
  each <1%.
