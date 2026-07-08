# osp-sim — pairing-settings simulator

A command-line tool that Monte-Carlo–simulates a tournament many times to compare
**pairing-settings variants**. It answers two questions with numbers:

1. **Fewer boring games?** — does a setting reduce mismatched boards (players far
   apart in strength)?
2. **Same likely winner?** — does it change who tends to win, or how faithfully
   the final ranking tracks real strength?

For the methodology (result model, strength model, metrics) see
[`docs/simulation-cli.md`](../../docs/simulation-cli.md). This file is just how to
run it.

## Build & run

```sh
# from the repo root; --release matters for large --runs
cargo run --release -p osp-sim -- <options>
```

## Choosing a base tournament (exactly one required)

| Flag | Source |
|------|--------|
| `--base <FILE>`    | an `.osp` save (JSON) |
| `--grid <FILE>`    | an American Grid cross-table export |
| `--results <FILE>` | a **FESA post-tournament result table** — the artefact tournaments usually publish. This one *also* supplies each player's true strength (pre-ELO + points gained, or the `*` rating for a pre-unrated player), so no `--strength*` flag is needed. |

The base provides the players and ratings the engine pairs on, and — for `--grid`
/ `--results` — the real rounds, which drive the **observed** row (see below).

## True strength (what outcomes are drawn from)

Outcomes follow the logistic law `P(A beats B) = 1/(1+10^((elo_B−elo_A)/400))` on
each player's *true strength*. By default that is the base rating; override it
with one of (mutually exclusive; not needed with `--results`):

| Flag | True strength |
|------|---------------|
| `--strength <FILE>` | JSON object mapping tournament number → ELO |
| `--strength-fesa-after <YYYY-MM-DD>` | the first FESA rating list after that date (fetched from fesashogi.eu), matched by name |
| `--strength-fesa-list <URL\|PATH>` | a specific FESA rating list, matched by name |
| `--jitter <F>` | not a source, but adds rating-dependent noise: `0` = truth is the rating, `1` = sample from the estimator's own prior, `>1` = stress-test |

## Other options

| Flag | Meaning | Default |
|------|---------|---------|
| `--configs <FILE>...` | one or more settings variants to compare, each a full `TournamentSettings` JSON (export one from the app's **Settings → Export settings** button) | the base's own settings |
| `--runs <N>` | simulated tournaments per variant | `1000` |
| `--rounds <N>` | rounds per simulated tournament | the base's round count |
| `--seed <N>` | master seed; run *i* uses `seed+i`, identical across variants (common random numbers) | `0` |
| `--threshold <T>` | report `P(\|diff\| > T)`; repeatable | `400` |
| `--out <DIR>` | also write `report.json` and per-variant CSV histograms | — |

## Cup format (hybrid direct-elimination bracket)

To simulate the hybrid format — a seeded knock-out cup among the top eligible
players running alongside the Swiss — pass `--cup-size`. Eligibility is derived
from the base's real rounds:

| Flag | Meaning | Default |
|------|---------|---------|
| `--cup-size <N>` | bracket size (8/16/32/64); enables the cup for every variant | off |
| `--cup-nations <CODES>` | comma-separated eligible nationalities, e.g. `FR,BE,CH` (required with `--cup-size`) | — |

A player is eligible if their nationality is listed **and** they were not absent
in any of the cup rounds — the first `log2(size)` real rounds (e.g. 5 for a
size-32 cup). The top `--cup-size` eligible players (by rating) are seeded.
Eligibility needs the base's real rounds, so this is meant for a `--grid` /
`--results` base.

## What it prints

- a **mismatch** table (mean / median / p90 / p95 of `|ELO diff|`, and
  `P(|diff| > T)`) per variant;
- a **fidelity** table — mean Spearman correlation of the finishing order vs true
  strength, by score standings *and* by the ELO estimate;
- **victory probabilities** per player with 95% Wilson confidence intervals;
- an **`observed*`** row / `obs.rank` column when the base has real results, so
  you can sanity-check the model against what actually happened.

## Examples

```sh
# 1. Historical sanity check: replay a real result table under its own settings.
cargo run --release -p osp-sim -- --results results_WOSC_2024.txt

# 2. Compare Swiss vs MacMahon on that tournament (2000 runs each).
cargo run --release -p osp-sim -- \
  --results results_WOSC_2024.txt \
  --configs swiss.json macmahon.json --runs 2000

# 3. From an American grid, using the next FESA list as true strength.
cargo run --release -p osp-sim -- \
  --grid AmericanGrid.txt \
  --configs baseline.json airtight.json \
  --strength-fesa-after 2024-08-05

# 4. Robustness sweep: rating noise, custom thresholds, machine-readable output.
cargo run --release -p osp-sim -- \
  --base tournament.osp --configs a.json b.json \
  --runs 5000 --jitter 1 --threshold 300 --threshold 500 \
  --out report/

# 5. Hybrid cup: a size-16 bracket among FR/BE/CH players who played the first 5 rounds.
cargo run --release -p osp-sim -- \
  --results results_CdF_2025.txt \
  --cup-size 16 --cup-nations FR,BE,CH
```
