# Tournament simulation & pairing-quality analysis — design

Status: **Not started** — design only. Fleshes out the **Simulations** section of
[`TODO.md`](../TODO.md) (the two endpoint lines) into a full tool.

Goal: answer, with numbers, two questions a referee has when tuning pairing
settings:

1. **Fewer boring games?** Does a settings change reduce *mismatched* games —
   boards where the two players are far apart in strength, so the result is a
   foregone conclusion?
2. **Same likely winner?** Does it meaningfully change *who* tends to win the
   tournament, or how faithfully the final ranking reflects real strength?

The vehicle is a Monte-Carlo simulator: replay a tournament many times under a
probabilistic result model, once per settings variant, and compare the resulting
distributions. Two prerequisites already exist — loading a tournament from an
American Grid ([`import_american_grid`](../crates/core/src/grid_import.rs), for
historical baselines) and cancelling rounds ([`cancel_last_round`](../crates/core/src/tournament.rs))
— so this design covers the rest.

Scope markers: **V1** = first shippable slice; **V2** = deferred.

---

## 0. Architecture — where the work runs

The statistics tool wants to run the pairing→result→pairing loop *thousands* of
times. Driving that over HTTP against the single in-memory tournament
([`TournamentStore.current`](../crates/server/src/state.rs)) would be serial,
slow, and would clobber whatever tournament that server holds — and would buy
nothing, because the HTTP handlers only wrap core functions the loop can call
directly.

The decision (see §7): **put all the logic in `osp_core` as pure functions** and
have the CLI **link `osp_core` directly**, running its loop in-process — parallel
across runs, seeded, touching no live server state. The same core functions
(`prepare_round` → `confirm_round` → result → `complete_current_round`) that the
referee's actions ultimately call are reused verbatim, so the simulation has full
fidelity with the real engine while staying fast and isolated. **No REST
endpoints are added** — see §2 for why, including the two `TODO.md` lines that
proposed them.

```
osp_core::sim   (pure: outcome model, elo-diff stats, single-run driver)
   └── crates/sim   → CLI: K×N loop in parallel, seeded, comparative report
```

---

## 1. Phase 1 — Core building blocks (`osp_core`)

All three are pure functions with no I/O, unit-tested in isolation. They live in a
new `crates/core/src/sim.rs` (outcome model + drivers) reusing the existing ELO
scale constant from [`elo.rs`](../crates/core/src/elo.rs).

### 1.1 The result model — probabilistic auto-fill

Model (from `TODO.md`, the standard logistic/Bradley–Terry expected score):

```
P(A beats B) = 1 / (1 + 10^((elo_B − elo_A) / 400))
```

This is the same law `elo.rs` already fits; we now *sample* from it. For each
**undecided** board in a round: draw `u ~ Uniform(0,1)`, set the winner to
player 1 iff `u < P(p1 beats p2)`, and write it through the existing
[`toggle_board_winner`](../crates/core/src/tournament.rs) path so standings stay
consistent.

- **Strength source** (see §7). Each player's ground-truth strength for a run is:
  - if the client supplied an **override** for them, that value **exactly** — an
    override is a known truth and is never jittered;
  - otherwise a draw from the player's own **prior**,
    `strength_i ~ N(rating_i, (jitter · σ₀_i)²)`, where `σ₀_i` is the prior
    standard deviation [`elo.rs`'s `prior()`](../crates/core/src/elo.rs) already
    computes (rating gives the mean, `600 / 350` for an unrated player). The
    `jitter` multiplier scales that width: `0` pins truth to the registration
    rating, `1` samples from the very prior the estimator assumes (automatically
    tighter for strong/established players, wider for provisional/unrated ones),
    `>1` stress-tests worse-than-assumed ratings.
  - The draw happens once per player per run, so each run is a slightly different
    "world" and victory probabilities marginalise over them. This reuses the
    existing rating-dependent widths instead of a second, flat noise model.
- **Byes** are not boards and not games — skipped, and excluded from every game
  statistic. (A bye already scores as a win in standings.)
- **Handicaps**: the simulator pairs even games only, so boards never carry a
  handicap. If one somehow does, fall back to `effective_winner` semantics; not a
  V1 concern.
- **Draws**: shogi draws (sennichite) are rare. V1 models none (a pure win/loss
  Bernoulli). A small fixed draw probability is a trivial **V2** knob.
- **Determinism**: the function takes an `&mut impl Rng` (e.g. `rand_chacha`
  `ChaCha8Rng`). The caller seeds it, so a run is byte-for-byte reproducible.

> **Engine-determinism risk to verify.** Reproducibility also requires the
> *pairing* to be deterministic given identical inputs. The matching in
> [`integer-blossom`](../crates/matching/src/lib.rs)/[`pairing.rs`](../crates/core/src/pairing.rs)
> should be, but any incidental `HashMap` iteration order feeding a tie-break
> would inject non-determinism. Audit this when implementing; sort where needed.

### 1.2 The ELO-difference distribution

A pure function over a `Tournament` (+ optional strength override map) returning
the per-game strength gaps:

```
for each completed board (excluding byes):
    diff = | strength(p1) − strength(p2) |
```

Why the optional map: as a tournament progresses a player's *effective* strength
drifts from their registration rating (the point of the `TODO.md` "updated
player→ELO mapping" note). Passing the [`estimate_elos`](../crates/core/src/elo.rs)
output, or the simulator's ground-truth map, measures mismatch against a more
realistic strength than the frozen seed rating.

The function returns the **raw list** of diffs (cheap, composable); summarising
into histograms/quantiles happens one level up (§3) so the same primitive serves
both a single-tournament endpoint and a multi-run aggregation.

### 1.3 The single-run driver

```
fn simulate_run(base: &Tournament, settings: &TournamentSettings,
                strength: &StrengthMap, rounds: u32, rng: &mut impl Rng)
    -> RunOutcome
```

Clone `base` reset to *registration-finalized, zero rounds* (the reset the
American-grid + `cancel_last_round` steps already enable), apply `settings`, then
loop `rounds` times: `prepare_round` → `confirm_round` → auto-fill the new round's
boards (§1.1) → `complete_current_round`. `RunOutcome` collects what the report
needs: the final [`standings()`](../crates/core/src/tournament.rs) order, every
board's ELO diff (§1.2), and enough pairing facts for the secondary metrics
(§4) — rematches avoided/forced, floats, etc.

Cloning the base per run (rather than cancelling rounds over REST between runs)
is what makes the loop embarrassingly parallel.

---

## 2. Why no REST endpoints (the two `TODO.md` lines)

`TODO.md` proposed both building blocks as server endpoints ("Add an API to get
random game results", "Add an API getting statistical results"). With the loop
in-process, they earn nothing here:

- **Auto-fill** — the `TODO.md` line itself scopes it to "tests, and for doing
  simulations," explicitly "not surfaced in the UI." Both consumers are now
  in-process (core unit tests + the CLI). An endpoint would have **no caller
  left**, and — since it writes real results — would be a footgun on the
  [remote-mode](multi-referee-internet.md) server. Dropped entirely.
- **ELO-diff** — as a *simulation* input it is just a core function the CLI calls.
  The endpoint would only matter for a different, product-side feature: showing
  the mismatch spread of a referee's **live** tournament in the app UI. That is a
  real feature, but it needs a UI to consume it (none exists today) and is
  independent of this tool. **Deferred** to whenever that stats panel is designed;
  the §1.2 core function is its natural backend when it is.

No HTTP layer adds fidelity — the endpoints would only re-wrap the same core
functions the in-process loop already calls. So this project ships as **core +
CLI**, nothing on the server.

---

## 3. Phase 3 — The CLI (`crates/sim`)

A new binary crate linking `osp_core`. It is the actual analysis tool.

### 3.1 Inputs

```
osp-sim \
  --base CdF2026.osp            # or --grid AmericanGrid.txt, or --results results_WOSC.txt
  --configs base.json a.json b.json   # each: a full TournamentSettings JSON (see §3.4)
  --runs 1000                   # K
  --rounds 7                    # N (default: the base's configured/observed count)
  --seed 42                     # master seed → per-run seeds (seed + run index)
  --strength elos.json          # optional override map (treated as known truth)
  --strength-fesa-after DATE    # true strength = first FESA list after this tournament date
  --strength-fesa-list URL|PATH # true strength = a specific FESA list (matched by name)
  --jitter 0                    # multiplier on each player's prior σ₀ (0 = truth == rating)
  --threshold 400               # |diff| cut for P(|diff|>T); repeatable; default 400
  --out report/                 # JSON + human table + per-config CSV histograms
```

**Base sources.** `--base` loads an `.osp` save; `--grid` an American Grid; and
`--results` a **FESA post-tournament result table** (the artefact tournaments
usually publish instead of the raw grid — see
[`fesa_results`](../crates/core/src/fesa_results.rs)). `--results` is special: it
reconstructs the rounds *and*, from each row's pre-ELO + points gained (or the
assigned rating for a pre-unrated player), supplies every player's true strength
directly — so it needs no `--strength*` flag and covers 100% of players (no
name-matching gaps). It reuses the American-grid round-rebuild machinery; because
that model holds one bye per round, extra `0+` walkover wins in a round are
demoted to absences, a tail-only rounding in the *observed* standings that leaves
the real games and the simulation untouched.

**True strength from the post-tournament FESA list.** When you only have a grid
(not a result table), the first FESA rating list *after* the event also reflects
its results. `--strength-fesa-after 2025-12-01` resolves the first
list published after that date (FESA publishes on 1 Jan and 1 Jun), fetches
`…/ratinglists/YYYY-MM-DD.txt`, and matches players by name; `--strength-fesa-list`
points at a specific list (URL or local file) instead. Matched players become
known-truth overrides (un-jittered); unmatched players (unrated, or a name the
list spells differently) keep their grid rating. Note the grid has no date field,
so the tournament date is supplied on the command line. These flags and
`--strength` are mutually exclusive, and none of them changes the ratings the
engine *pairs* on — those always come from the grid (§0-style: pairing uses the
tournament's own ratings; strength only drives outcomes and the mismatch metric).

Each `--configs` file is a complete [`TournamentSettings`](../crates/core/src/settings.rs)
JSON object (not a patch) — the same shape the server's
`PUT /api/tournament/settings` already accepts and `normalized()` cleans up. To
make producing one painless, the Settings tab in the UI gains an **"Export
settings"** button that downloads the current `TournamentSettings` as JSON, so a
referee tweaks the knobs in the app they already know and drops the file straight
into `--configs` (see §3.4).

### 3.2 Execution

For each config × each run index `i`: seed a fresh RNG with `seed + i`, call
`simulate_run` (§1.3). Runs are independent → parallelise with `rayon`
(`into_par_iter`), collect `RunOutcome`s, then aggregate into the metrics of §4.
K=1000, N=7, ~40 players completes in well under a second per config on a laptop.

### 3.3 Output

- **Human table** to stdout: one row per config, columns for the headline metrics
  (§4) with Monte-Carlo confidence intervals, so an A-vs-B gap is visibly
  distinguishable from sampling noise.
- **`report.json`**: the full aggregates, for scripting / plotting.
- **`elo-diff-<config>.csv`**: the pooled diff histogram per config, for a quick
  external plot.

### 3.4 Producing config files (UI export)

A small frontend addition, so referees never hand-write JSON: the Settings tab
gains an **"Export settings"** button that serializes the current
`TournamentSettings` to a `.json` download. Configuring a variant is then "adjust
the knobs in the UI → export → pass to `--configs`." Round-trips cleanly because
the CLI feeds the file through the same `TournamentSettings` deserialize +
`normalized()` the server uses. (A matching "Import settings" is a natural
companion but not required by this tool.)

---

## 4. Metrics & statistics (the substance)

Aggregated across the K runs of a config:

### 4.1 Mismatch ("boring games") — the first question

- Pooled ELO-diff distribution: **mean & median |diff|**, the 90th/95th
  percentiles, and **`P(|diff| > T)`** for each referee-chosen `T` (`--threshold`,
  repeatable; **default 400**, ≈ a 90% win chance for the favourite). The last is
  the headline "fraction of games that were foregone conclusions."
- Reported as a histogram (CSV) plus those summary scalars.

### 4.2 Who wins & how fair — the second question

- **Victory probability** per player: fraction of runs finishing rank 1
  (standings order already applies the configured tie-breaks, so rank 1 is
  well-defined). Also **top-3 / podium** rates, since the format has a cup podium.
  Each with a **Wilson confidence interval** (`SE ≈ √(p(1−p)/K)`), so "config A
  makes player X 8%±2% more likely to win" is stated honestly.
- **Ranking fidelity**: **Spearman rank-correlation** between the final standings
  order and the *true-strength* order, averaged over runs — a single number for
  "does this format sort players by real strength?" Plus a **"strongest player
  won"** rate.

### 4.3 Interestingness vs ranking fidelity — measure, don't assume

4.1 and 4.2 need not move together, but the reason is subtler than "close games
are uninformative" — they are not. What a game reveals depends on what you do with
the result:

- For the **strength estimates**, an even game (`p ≈ 0.5`) is the *most*
  informative: the Fisher information about the gap is `p(1−p)`, maximal at 0.5.
  The estimator uses exactly this — each game adds `e(1−e)` into its curvature
  ([`elo.rs:257`](../crates/core/src/elo.rs)) — which is why the ELO-pairing mode
  pairs closest-in-strength.
- The **score standings** (metric 4.2 as first defined) rank by accumulated wins,
  *not* by that estimate. Win-counts recover a **global** order only from
  *decisive* results; a card of near-coin-flips gives noisy scores, and pairing
  only near-equals also means far-apart players never meet — so no global order
  emerges. The information is in the results, but the score statistic discards it.

So whether minimizing mismatch (4.1) costs ranking fidelity (4.2) is **contingent**
— on the ranking statistic and the pairing structure — not a law. The tool should
therefore report ranking fidelity **both** ways: against the score standings *and*
against the [`estimate_elos`](../crates/core/src/elo.rs) posterior. If pushing
mismatch toward zero hurts the score-based Spearman but not the estimate-based
one, that is the real, measurable finding — and it turns on exactly the
information an even game carries. Putting all of this in one table lets the referee
*measure* whether a setting traded one for the other, instead of assuming it.

### 4.4 Secondary pairing-health metrics (V2)

Rematch rate, float-repeat rate, same-club pairings, score-group integrity — all
already computed by the engine's rule ledger
([`RoundExplanation`](../crates/server/src/tournament.rs)); surface them as extra
columns once the core loop exists.

---

## 5. Risks, caveats & things easy to miss

- **Self-consistency caveat.** If ground-truth strength *equals* the ratings the
  engine pairs on, the simulation measures the format under a *known, correct*
  world — it cannot reveal how the format copes when ratings are wrong. That is
  precisely what the optional strength-override map and the `--jitter` knob are
  for; the doc should steer users toward using them for robustness questions.
- **Victory-probability interpretation.** With zero jitter, upsets come only from
  the logistic model's inherent noise; a strong favourite will win most runs.
  That is realistic, but means "who wins" moves slowly between configs — small
  differences need the CIs of §4.2 to interpret, hence Monte-Carlo error bars are
  not optional polish.
- **Byes** must be excluded from every game statistic (they are wins, not games).
- **Draws** are unmodelled in V1 — fine for shogi, but note it in the report so a
  reader doesn't mistake it for a claim that draws never happen.
- **Determinism** depends on both the RNG seed *and* a deterministic pairing
  engine (§1.1 risk note).
- **Rounds count `N`.** Default it from the base, but a stale default silently
  changes results; make the effective `N` explicit in the report header.

---

## 6. Suggested cut & sequencing

All of the following are **V1**:

1. **Core (§1)** — the outcome model, ELO-diff function, and single-run driver,
   with unit tests. Nothing else is possible without this.
2. **CLI (§3–4)** — multi-config comparison with the ELO-diff, victory-probability
   and ranking-fidelity metrics; the settings-export button (§3.4); and the
   historical **"observed"** column (§3.3), since the base can already load a
   completed grid and the referee wants the model checked against real events.
3. **Secondary pairing-health metrics (§4.4)** — cheap extra columns once the loop
   exists; the one genuinely deferrable piece, so **V2**.

Recommended order: **1 → 2 → 3**. (No server work — see §2.)

---

## 7. Settled decisions

- **Logic in `osp_core`; CLI runs the loop in-process; no REST endpoints.** Fast,
  isolated from live server state, and reuses the real engine code paths (§0). The
  two `TODO.md` endpoint proposals are dropped/deferred (§2).
- **True strength** = an explicit override (known, un-jittered) else a per-run
  draw from the player's own `elo.rs` prior, `N(rating, (jitter·σ₀)²)`, so the
  noise is rating-dependent and coherent with the estimator (§1.1).
- **One CLI invocation compares multiple settings configs** and emits a
  side-by-side report with Monte-Carlo confidence intervals (§3–4).
- **Outcome law** is the logistic `1/(1+10^((B−A)/400))` from `TODO.md` — the same
  model `elo.rs` fits.
- **Auto-fill endpoint is gated off** on hosted/remote instances (§2).
- **Winner = the single rank-1 finisher**, using the tournament's existing
  tie-break chain to resolve the order — no special fractional-win handling for
  points-ties (§4.2).
- **`--configs` takes full `TournamentSettings` JSON**, not patches; the UI grows
  an "Export settings" button to produce them (§3.4).
- **`P(|diff| > T)` thresholds are referee-set** (`--threshold`, repeatable),
  defaulting to 400 ≈ a 90% favourite (§4.1).
- **Historical "observed" column is V1** — validating the model against real grids
  is a first-class goal, not a nice-to-have (§3.3, §6).
