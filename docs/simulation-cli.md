# `osp-sim` — pairing-settings simulator

`osp-sim` answers, with numbers, two questions a referee has when tuning pairing
settings:

1. **Fewer boring games?** Does a settings change reduce *mismatched* games —
   boards where one player is far stronger, so the result is a foregone
   conclusion?
2. **Same likely winner, fair ranking?** Does it change *who* tends to win, or
   how faithfully the final ranking reflects real strength?

It takes a historical tournament as the baseline, then for each **settings
variant** replays it thousands of times under a probabilistic result model and
compares the resulting distributions — side by side, with confidence intervals
and paired significance.

```
osp-sim \
  --results "test_files/CdF 2025.txt" \   # baseline (also supplies true strengths)
  --cup-size 16 --cup-nations FR \        # optional hybrid cup
  --jitter 1 \                            # uncertainty in the true strengths
  --configs a.json b.json c.json          # each a full TournamentSettings JSON
```

## Architecture

All the logic lives in [`osp_core::sim`](../crates/core/src/sim.rs) as pure
functions; the [`crates/sim`](../crates/sim) CLI links `osp_core` directly and
runs the replay loop **in-process, in parallel** (`rayon`), seeded, touching no
live server state. The same engine functions the referee's actions ultimately
call — `prepare_round` → `confirm_round` → record results — are reused verbatim,
so a simulated tournament is paired exactly as a live one. There is **no server
involvement and no REST endpoint**: the loop would only re-wrap the same core
functions it already calls.

```
osp_core::sim   pure: strength oracle, result model, single-run driver, metrics
   └── crates/sim   CLI: variants × runs loop, comparative report, JSON/CSV output
```

## Inputs

### Base sources (exactly one)

- `--base FILE.osp` — an `.osp` save (JSON).
- `--grid FILE.txt` — an American Grid cross-table
  ([`import_american_grid`](../crates/core/src/grid_import.rs)).
- `--results FILE.txt` — a **FESA post-tournament result table** (the artefact
  tournaments usually publish; see
  [`fesa_results`](../crates/core/src/fesa_results.rs)). This source is special:
  it reconstructs the rounds *and* supplies every player's **true strength**
  directly — pre-ELO + points gained for a rated player, or the assigned `*`
  rating for a pre-unrated one — so it needs no separate `--strength*` flag and
  covers 100% of players with no name-matching gaps. Round-cell annotations
  (`(-r )`, `(+b )`, handicap marks, …) are stripped; because the round model
  holds one bye per round, extra `0+` walkover wins are demoted to absences — a
  tail-only rounding in the *observed* standings that leaves the games and the
  simulation untouched.

### True-strength overrides (when not using `--results`)

The "true strength" is what outcomes are actually drawn from, distinct from the
rating the engine *pairs* on (which always comes from the base). Supply it via:

- `--strength FILE` — a JSON object mapping tournament number (as a string) to an
  ELO.
- `--strength-fesa-after YYYY-MM-DD` — use the first FESA rating list published
  *after* the event (it already reflects the results), fetched from fesashogi.eu
  and matched by name. FESA publishes on 1 Jan / 1 Jun.
- `--strength-fesa-list URL|PATH` — a specific FESA list instead.

These are mutually exclusive. Unmatched players (unrated, or a differently-spelled
name) fall back to their registration rating. None of them changes the pairing
ratings.

### Other flags

| flag | meaning |
|---|---|
| `--configs A.json …` | one or more variants, each a full `TournamentSettings` JSON (§ Config files) |
| `--runs N` | simulated tournaments per variant (default 1000) |
| `--rounds N` | rounds per run (default: the base's round count) |
| `--seed S` | master seed; run *i* uses `seed + i`, shared across variants (§ Common random numbers) |
| `--jitter J` | scale on the true-strength spread (§ The strength oracle); default 0 |
| `--oracle-provisional M` | how much wider the true-strength prior is for a provisional player (default 2.0) |
| `--cup-size N` | run the hybrid cup with a bracket of 8/16/32/64 (needs `--cup-nations`) |
| `--cup-nations FR,BE,…` | nationalities eligible for the cup |
| `--threshold T` | `|diff|` cut for `P(|diff|>T)`; repeatable; default 400 |
| `--out DIR` | write `report.json` + per-variant ELO-diff CSV histograms |
| `--dump-runs FILE` | per-run metrics CSV, for ad-hoc paired analysis |
| `--dump-strengths FILE` | per-player sampled true strength for every run |

### Config files

Each `--configs` file is a complete
[`TournamentSettings`](../crates/core/src/settings.rs) JSON object (not a patch) —
the same shape the server accepts and `normalized()` cleans up. Unset fields take
their defaults, so a minimal variant is small, e.g. a single ELO MacMahon band:

```json
{ "macmahon_thresholds": [ { "criterion": { "kind": "elo", "value": 1486 } } ] }
```

The Settings tab in the app has an **"Export settings"** button that downloads
the current settings as such a JSON file, so a referee can tune the knobs in the
UI and drop the file straight into `--configs`.

## The strength oracle

Each player's ground-truth strength for a run is drawn from
`N(center, (jitter·σ₀)²)`:

- **center** — the player's override (the post-tournament strength from
  `--results`, a `--strength` value, or a matched FESA-list rating) if present,
  otherwise their registration rating. With `--results`, *every* player has an
  override, so the center is their post-tournament ELO.
- **σ₀** — the oracle prior width from
  [`oracle_prior`](../crates/core/src/elo.rs): the same rating-/reliability-
  dependent shape the ELO estimator uses, but pinned to the **raw FESA K** (×1),
  widened for a provisionally-rated player by `--oracle-provisional`. Tighter for
  strong/established players, wider for weak/provisional/unrated ones.
- **jitter** — the global scale on that width. `0` pins each player to their
  center exactly; `1` samples at the oracle's own prior width; `>1` stress-tests
  worse-than-assumed ratings.

Two properties matter:

- **Centered on the truth we have.** At `jitter > 0` a `--results` player scatters
  around their *post-tournament* ELO (which already reflects the event), not their
  pre-tournament rating.
- **Settings-independent.** The true strengths are one physical world shared by
  every variant, so the oracle width never depends on a per-variant pairing knob
  (`elo_k_multiplier`, `elo_provisional_multiplier`); the simulator scales it
  globally with `jitter` and `--oracle-provisional` instead. Players are sampled
  in slice order, so the draws — and the whole run — are reproducible from the
  seed and identical across variants.

## How a run works

[`simulate_run`](../crates/core/src/sim.rs) clones the base, resets it to
*registration-finalized, zero rounds* under the variant's settings, samples the
true strengths, then for each round: `prepare_round` → apply that round's
attendance → `confirm_round` → auto-fill the boards.

- **Absence reproduction.** Each simulated round sits out exactly the players who
  were absent in the *corresponding real round* of the base. Without this every
  registered player would play every round — a fuller, different tournament — and
  a player absent during the cup window (hence cup-ineligible) would silently
  reappear. Rounds past the base's recorded history have no attendance, so
  everyone plays.
- **Result model.** A board's winner is sampled from the logistic law
  `P(A beats B) = 1/(1 + 10^((elo_B − elo_A)/400))` on the two true strengths —
  the same law the estimator fits. Draws are not modelled (a win/loss Bernoulli);
  byes are not boards and are excluded from every game statistic.
- **The cup.** With `--cup-size`, a hybrid direct-elimination cup runs alongside
  the Swiss. Eligibility is computed once from the base: nationality in
  `--cup-nations` **and** present through the first `log₂(size)` rounds. The
  bracket seeds the top `size` eligible players by rating; MacMahon and floater
  settings don't touch it, so its outcome distribution is the same across
  variants (given common random numbers).

## Common random numbers

Every variant sees the same per-run seed (`seed + i`), so a settings A/B is
measured on the *same simulated worlds*. To make that coupling real rather than
incidental, each game's coin flip is **keyed on the game's identity** —
`hash(run_seed, low player id, high player id, rematch count)`, evaluated in a
canonical low→high orientation — instead of being drawn from a sequential stream.
So the *same matchup decides the same way* regardless of board order, which side
was seated first, or which variant produced it. Combined with the
settings-independent strengths, two variants that happen to pair the same players
resolve those games identically; the difference between variants is then only the
pairings the settings actually changed.

## Metrics

Reported per variant, aggregated over the runs.

### Mismatch — "boring games"

Pooled `|Δstrength|` over every played game: **mean & median**, the **90th/95th
percentiles**, and **`P(|Δ| > T)`** for each `--threshold` (default 400 ≈ a 90%
favourite) — the headline "fraction of games that were foregone conclusions."

### Ranking fidelity

Two Spearman rank-correlations against the true-strength order, averaged over
runs: **fidelity(score)** for the final standings, and **fidelity(est)** for the
[`estimate_elos`](../crates/core/src/elo.rs) posterior order. Reporting both
matters because they need not move together: for the *estimate*, an even game
(`p ≈ 0.5`) is the **most** informative (Fisher information `p(1−p)` peaks at
0.5), whereas the score standings recover a global order only from *decisive*
results. So whether minimizing mismatch costs ranking fidelity is contingent on
which statistic you rank by — the two columns let you measure it rather than
assume it.

### Game interest — `interest(W)`

A distributional view of boring games that the mean can't see: are the foregone
games *spread out* or *dumped on a few players*? For each game, its **interest**
is the outcome entropy `H₂(p)` of its win probability (1 bit at a coin flip, 0 for
a foregone conclusion — threshold-free). Per player, average their games'
interest; then combine across players with the **game-count-weighted Sen welfare**
`μ_w·(1 − Gini)`, which rewards *both* a higher level (fewer foregone games) *and*
a more even spread (Gini penalizes concentration). Weighting by games discounts a
player who sat out most rounds (their interest is noisy), and makes the level term
exactly the mean per-game suspense. `interest(W)` runs 0…1, **higher is better**
(like fidelity).

The report also lists the **least-interesting players** — the union of each
variant's five biggest contributions to the welfare shortfall (`1 − W`, which
decomposes exactly per player), with their pooled mean interest and mean games.
This surfaces effects the aggregate hides — e.g. that a threshold-MacMahon variant
buys a better average by burying strong *unrated* players (who meet no ELO/grade
threshold) in foregone games.

### Who wins

- **Open winner** probability per player: fraction of runs finishing rank 1 in the
  final standings, with a **Wilson 95% CI**, plus each player's real finishing
  rank in the observed event.
- **Cup champion** probability per player (Wilson CI), when a cup is configured.

## Statistical comparison

Point estimates are not exact, so a dedicated block reports, for the decision
metrics (mean|d|, `P(|d|>T)`, fidelity, interest): each metric's value **±95% CI**
(from run-to-run variation), and for every other variant the **paired Δ** against
the first variant, starred by significance. The pairing uses common random numbers
— the difference is taken per run, cancelling shared variation — so it is a
genuinely paired test, most powerful for player-level outcomes (e.g. who wins) and
weaker for pairing-structure metrics, where the variants genuinely pair different
people. `--dump-runs` writes the per-run values if you want a different baseline,
exact non-parametric tests, or multiplicity control in Python.

## The observed column

When the base has real played rounds, the report adds an **`observed*`** row: the
same metrics computed on the base's actual results (nominal strengths, no jitter)
— a real-world yardstick beside the simulated variants. It is a single realization,
so it typically tracks true strength less tightly than the run-averaged variants;
at `jitter > 0` its mismatch uses registration ratings, so compare it loosely.

## Outputs

- **stdout** — the human report: the metric tables, the statistical comparison,
  the winner / cup-champion / least-interesting-players tables.
- **`--out DIR`** — `report.json` (all aggregates, for scripting/plotting) and one
  `elo-diff-<variant>.csv` histogram per variant.
- **`--dump-runs FILE`** — one row per variant × run (`mean_diff`, `frac_exceed`,
  `fidelity`, `interest`, `winner`); rows sharing a `run` are paired.
- **`--dump-strengths FILE`** — the sampled true strength of every player in every
  run, for inspecting the oracle (e.g. that `--jitter` scatters a player around
  their post-tournament ELO).

## Caveats

- **Self-consistency.** With `--jitter 0` the true strengths *equal* the pairing
  ratings, so the run measures the format in a known-correct world — it cannot
  reveal how the format copes when ratings are wrong. Use `--jitter` (and
  `--oracle-provisional`) for robustness questions.
- **Slow-moving winners.** At low jitter a strong favourite wins most runs, so
  "who wins" shifts little between variants — read it through the CIs.
- **Draws** are unmodelled (fine for shogi, but note it before reading the tables
  as a claim that draws never happen).
- **Determinism** requires both the seed and a deterministic pairing engine; the
  matching in [`pairing.rs`](../crates/core/src/pairing.rs) /
  [`integer-blossom`](../crates/matching/src/lib.rs) is deterministic.
- **Rounds `N`** defaults from the base; the effective `N` is printed in the report
  header so a stale default can't silently change results.

## Possible extensions

- **Secondary pairing-health columns** — rematch rate, float-repeat rate,
  same-club pairings, score-group integrity — are already computed by the engine's
  rule ledger and could be surfaced as extra columns.
- **A small draw probability** would be a trivial knob in the result model.
