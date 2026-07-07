# Experimental ELO-based (non-Swiss) pairing mode — design

Status: **V1 implemented** — the estimator ([`crates/core/src/elo.rs`](../crates/core/src/elo.rs)),
the ELO rule ladder in the pairing engine, the settings toggle + multiplier
(with the Swiss sections greyed out), and the Results-tab "Est. ELO" column
(shown only in ELO mode; `Standing.estimated_elo`) are in. Still open: the V2
items in §8. Tracks the TODO item
"Experimental ELO-based, non-swiss system". This document pins down the Bayesian
estimation math and how the mode plugs into the existing pairing engine
([`crates/core/src/pairing.rs`](../crates/core/src/pairing.rs)) and scoring
replay ([`crates/core/src/scoring.rs`](../crates/core/src/scoring.rs)).

Scope markers: **V1** = the first shippable version; **V2** = deferred, listed at
the end.

## 1. Overview

When this mode is active:

- MacMahon and the Swiss-specific options (thresholds, degressive removals,
  floater style, fold) are **disabled / greyed out** in the settings UI.
- OSP maintains a live **estimated ELO** for every player, updated after each
  round from the game results by Bayesian reasoning.
- The pairing rule ladder collapses to three tiers (see §6): the victories /
  floaters / fold / score-gap family is replaced by a single rule that
  **minimizes the square of the estimated-ELO difference** on each board, and the
  club rule is dropped.

Conceptually this is not really "non-Swiss" so much as a **continuous Swiss**:
the discrete integer score groups are replaced by the ELO continuum. Winners
drift up and get matched against other risers, losers drift down, and no-rematch
spreads the field — the same self-sorting behaviour as Swiss, but on a smooth
strength axis instead of integer buckets.

## 2. The estimation model

Each player has a latent strength `θᵢ` on the ELO scale. Game outcomes follow
the logistic (Bradley–Terry) model already used by the results-simulation TODO,
with `s = 400 / ln 10 ≈ 173.72`:

```
P(i beats j) = σ((θᵢ − θⱼ) / s),   σ(x) = 1 / (1 + e^−x)
```

which is identical to `1 / (1 + 10^((θⱼ − θᵢ) / 400))`.

The **estimated ELO is the MAP (maximum a-posteriori) estimate** of `θ` given all
completed games — the maximiser of the penalised log-likelihood:

```
L(θ) = Σ_games  Sᵢg · log σ(dg) + (1−Sᵢg) · log σ(−dg)     (game likelihood)
     + Σ_i  log prior(θᵢ)                                   (prior / anchor)
```

where for a game `g` of player `i` against `opp`, `dg = (θᵢ − θ_opp)/s` and
`Sᵢg ∈ {0, ½, 1}` is `i`'s score in that game (½ for a draw, see §4).

Two properties make this the right fit for OSP:

- **It is a pure replay.** `estimate_elos(players, settings, completed_rounds) ->
  Map<Uuid, f64>` is a pure function of the completed rounds, exactly like
  `compute_scores`. It is order-independent and recomputed from scratch on every
  edit/undo — unlike a sequential filter (Glicko rating-periods, TrueSkill),
  which would be order-dependent and awkward to replay after an earlier result is
  corrected.
- **The prior anchors the absolute scale.** Plain Bradley–Terry is only
  identifiable up to a global shift. The per-player prior (below) pins every `θᵢ`
  to its own seed, so there is no gauge freedom to fix. The prior is load-bearing,
  not just regularisation.

The log-likelihood is concave and every prior is a concave (Gaussian) log-density,
so the objective is smooth and strongly concave and the maximiser is **unique and
deterministic**.

### 2.1 Prior for rated players — FIDE K × a single multiplier

We express trust in the registration rating as a Gaussian prior
`θᵢ ~ Normal(μᵢ₀, σ₀ᵢ²)`, with `μᵢ₀` = the registration ELO. The one thing the
referee sets is a **single multiplier `m`** (default `1.0`, expected range ~1–4)
applied to FIDE's rating-dependent K table:

| Registration rating `μ₀` | FIDE K |
| --- | --- |
| `μ₀ ≥ 2000` | 20 |
| `1600 ≤ μ₀ < 2000` | 24 |
| `1200 ≤ μ₀ < 1600` | 28 |
| `800 ≤ μ₀ < 1200` | 32 |
| `400 ≤ μ₀ < 800` | 36 |
| `μ₀ < 400` | 40 |

(Bounds are inclusive-lower/exclusive-upper, matching the MacMahon threshold
convention in `settings.rs`.)

The per-player effective K is `Kᵢ = m · K_FIDE(μᵢ₀)`, and the prior width is
derived from it (derivation in Appendix A):

```
σ₀ᵢ = √(Kᵢ · 400 / ln 10) ≈ 13.2 · √Kᵢ
```

`Kᵢ` is *literally the first-game K-factor* for player `i`: on the very first
game, the MAP update reduces to a FIDE Elo update `Δθᵢ ≈ Kᵢ·(S − E)` (Appendix
A). So round 1 behaves like a FIDE Elo step with the player's FIDE K scaled by
`m`; later rounds automatically damp (§2.3). Stronger players get a smaller K and
so a tighter prior — matching the fact that established high ratings are more
reliable.

Reference values (`σ₀` in ELO points):

| multiplier `m` | K range (20–40 → …) | σ₀ range |
| --- | --- | --- |
| 0.5 | 10–20 | 42–59 |
| 1 | 20–40 | 59–83 |
| 2 | 40–80 | 83–118 |
| 4 | 80–160 | 118–167 |

**This is the answer to "do we pick how fast the estimate drifts?":** No. The
referee picks the single multiplier `m`; every player's per-game drift then falls
out of Bayes automatically and *auto-decelerates* (§2.3). There is no separate
"drift per round" knob.

### 2.2 Prior for unrated players — Gaussian `N(600, 350²)`

Unrated players have no seed rating. Rather than a hard box (which would force the
solver to handle constraints), give them a **Gaussian prior moment-matched to the
"unrated players are somewhere in `[1, 1200]`" belief**: mean `600`, standard
deviation `σ_u ≈ 350` (a uniform on `[1,1200]` has mean 600 and std
`1199/√12 ≈ 346`). This keeps the objective a plain smooth strongly-concave
maximisation and buys three things over the box:

- **No constraints / clamping** in the solver.
- **A well-defined estimate before any games** — the MAP is just the prior mean
  `600`, so there is no flat/indeterminate region to special-case.
- **A strong newcomer can rise past 1200** when results justify it (a newcomer who
  beats several 1600s *should* be estimated above 1200 — the box would have
  wrongly capped them).

`σ_u ≈ 350` is deliberately much wider than any rated player's `σ₀`, so unrated
estimates move fast — see §2.4 for exactly how fast, and the trade-off if it
proves too jumpy in practice.

### 2.3 Drift and auto-deceleration

After `n` roughly-even games (`E ≈ ½`, so per-game Fisher information near its
maximum `1/(4s²) ≈ 8.3·10⁻⁶`), the posterior variance shrinks as

```
σₙ² ≈ 1 / (1/σ₀² + n/(4s²)),     Kₙ = σₙ² / s
```

so the effective K falls as games accumulate. Concretely, with `σ₀ = 83` (a
K=40 player at `m=1`) variance roughly halves over ~17 even games; with `σ₀ = 59`
(a K=20 strong player) it takes ~35 — i.e. strong players barely move across a
tournament while newcomers move fast, with no hand-tuning. And because the
pairing *minimizes* the ELO gap, games are near-even, `E ≈ ½`, information is
near-maximal, and each result updates the estimate efficiently.

### 2.4 Per-game change is capped by K — just like Elo

A natural worry: does a big upset (an 1100 beating a 1600) move the estimate a
lot? For a **single game the Bayesian update is capped by the current effective
K, exactly like Elo.** The one-game posterior-mean shift is

```
Δθ = (S − E)/s / (1/σ² + E(1−E)/s²)   ≤   σ² / s = K_current
```

and since `|S − E| ≤ 1` the move can't exceed the current K. Worked example, with
the winner a *rated* 1100 player (`K_FIDE = 32`, `m = 1` → `σ₀ ≈ 75`) beating a
1600: `E = σ((1100−1600)/s) ≈ 0.053`, so `Δθ ≈ +30 → 1130` — essentially identical
to Elo's `K(S − E) = 32 · 0.947 ≈ +30`. One upset is genuinely weak evidence, and
a rated player's prior encodes their past games, so the model rightly resists it;
if the 1100 player really is stronger, they keep winning and the estimate climbs
game by game.

Two ways this is more than fixed-K Elo, both automatic:

- **The cap shrinks as a rating firms up.** `K_current = σ_current²/s`, and
  `σ_current` falls with games played (§2.3) — the "provisional rating settles"
  behaviour, for free.
- **The cap is proportional to how unsure we are.** This is the whole point of the
  wide unrated prior: `σ_u ≈ 350` gives a first-game cap of `σ_u²/s ≈ 705`, so an
  unrated player moves ~175 on an even game and rises toward that cap on a big
  upset (an unrated player beating a 1600 jumps ~700 in one game). A
  well-established rated player barely budges. Fixed-K Elo fakes this with
  rating-band K tables (which we reuse *only* to seed the prior width); here it
  falls out of the prior variance directly.

If upsets should move estimates more across the board, raise **m** (bigger K →
bigger cap). If the unrated jumps feel too large, shrink `σ_u` — a tighter, more
informative unrated prior trades responsiveness for stability (the same knob).

## 3. The solver

With every prior Gaussian the objective is smooth, unconstrained and strongly
concave, so its maximiser is unique. Use **coordinate ascent (Gauss–Seidel)** — no
external LP/QP dependency, in keeping with the hand-written blossom matcher (a
single dense Newton solve works too; coordinate ascent is just simpler and
allocation-free).

For each player `i`, holding the others fixed, take a 1-D Newton step on the
concave 1-D objective:

```
gradient g = Σ_games (Sᵢg − Eᵢg)/s   − (θᵢ − μᵢ₀)/σ₀ᵢ²
hessian  H = Σ_games −Eᵢg(1−Eᵢg)/s²  − 1/σ₀ᵢ²
θᵢ ← θᵢ − g/H
```

where `Eᵢg = σ((θᵢ − θ_opp)/s)` and `(μᵢ₀, σ₀ᵢ)` is the rated prior (§2.1) or the
unrated prior `(600, 350)` (§2.2) — every player now carries a prior term, so
there is no special-cased branch. Sweep all players until the largest change in a
sweep is below a tolerance (a few ELO-hundredths). Strong concavity guarantees
convergence to the unique global maximum, so the result is deterministic given a
fixed tolerance and max-iteration cap.

Seed: rated players at `μᵢ₀`, unrated at `600`. Games that don't inform strength
(byes; handicap games in V1 — see §4) are simply excluded from the sums.

Per-player posterior variance (`σ_current² = −1/H` at the optimum), if wanted
later (e.g. the results-simulation TODO or the caps of §2.3/§2.4), is available
for free.

## 4. Game-type handling

- **Draws** (`Board.drawn`): scored `S = ½` for each side — the standard
  Elo/Glicko pseudo-likelihood treatment. (A principled Davidson draw model would
  add another parameter; rejected for V1.)
- **Handicap games** (`Board.handicap`): **excluded from the likelihood in V1**
  (they still count as "played" for no-rematch). **V2:** incorporate them via a
  known handicap→ELO-adjustment mapping (to be provided) as an offset in the
  logistic, `P(giver wins) = σ((θ_giver − θ_recv − h)/s)`.
- **Byes:** not a game — contribute nothing to the likelihood. "No repeat bye"
  stays in the rematch tier; *who* takes the bye is decided by a dedicated tier
  (§6).

## 5. Data & code layout

- New module `crates/core/src/elo.rs`, sibling to `scoring.rs` / `standings.rs`:
  the FIDE-K table, the MAP solver, and `estimate_elos(...) -> Map<Uuid, f64>`
  (plus the raw `f64` and, optionally, variance).
- Estimated ELO surfaced as `Standing.estimated_elo` (rounded to `i32` to keep
  `Standing` `Eq`), computed in `compute_standings` and already in the envelope's
  `standings`; the Results tab shows it as an "Est. ELO" column, only in ELO mode.
- New settings fields (§7).

## 6. Integration with the pairing engine

When the mode is on, the `Rule::ORDER` ladder in `pairing.rs` becomes **three
tiers** (highest priority first):

1. **Rematch / repeat-bye** — unchanged (top tier). Two players never meet twice;
   no one takes the bye twice.
2. **ByeSelection** *(new — do not omit)* — decides *who* sits out. On the bye
   (phantom) edge only, `units = rank(p)` where players are ranked by estimated
   ELO ascending (`0` = weakest), so the **weakest eligible player takes the
   bye**. Real-game edges emit 0 for this rule. It must sit **above EloGap**:
   EloGap is indifferent to the bye (a bye has no gap), so without this tier the
   bye would fall on whoever happens to make the rest of the matching marginally
   cheaper, distorting the remaining pairings. Lexicographic priority means the
   bye is chosen first, then the rest is optimised. Worst-case total units:
   `free_count − 1`, emitted once (a single bye per round).
3. **EloGap** — replaces ScoreGap + FloatRepeat + FloaterSelection + Fold. On a
   real edge, `units = (round(eloᵢ) − round(eloⱼ))²`; on the bye edge, 0. This is
   structurally identical to today's `ScoreGap` rule (squared difference), just
   fed rounded estimated ELO instead of points, so it reuses the existing
   `max_gap`/`scale_ladder` machinery. `i128` weights have ample headroom
   (max gap ≈ 2400 → gap² ≈ 5.8·10⁶, × edges, well within range).

The multiplier ladder (`scale_ladder`) derives the tier separations from each
rule's worst-case units exactly as today, so lexicographic correctness is
preserved by construction.

**Club protection is intentionally dropped in this mode.** It would sit below
EloGap, whose real-valued squared-gap costs are essentially never tied, so a club
tie-break would almost never change a pairing — not worth the tier. The
`club_protection_*` settings are simply ignored when the ELO mode is on.

## 7. Settings shape

Add to `TournamentSettings` (additive, defaulted, so old saves still load):

- `elo_pairing_enabled: bool` — the mode switch. Mutually exclusive with
  MacMahon in the UI (greys out thresholds, removals, floater style, fold, and the
  club-protection controls — implemented by wrapping those sections in a disabled
  `<fieldset>`). Off by default.
- `elo_k_multiplier_percent: u32` — the single knob `m`, stored as an integer
  percent (`100` = ×1.0) so `TournamentSettings` stays `Eq` (an `f64` field would
  break the derive, and the tournament `Eq` that the undo-snapshot store relies
  on). Read as a float via `settings.elo_k_multiplier()`. Default `100`, expected
  100–400; the UI presents it as a decimal multiplier. Normalization clamps it to
  ≥ 1 (a zero-width prior would be degenerate).

The unrated prior `N(600, 350²)`, the FIDE K table, and `s` are constants in
`elo.rs`, not settings (change only with code).

## 8. V2 / deferred

- **Standings & ranking in this mode.** Left deliberately unspecified here — the
  standings code is about to change. **TODO(V2):** decide the ranking column
  (victories, or estimated ELO, or victories → estimated-ELO tiebreak) once that
  work lands.
- **`#games` reliability-weighted prior.** The FESA list already carries a
  `#games` column that the parser reads and discards
  (`let _games` in [`fesa.rs`](../crates/core/src/fesa.rs)). Persisting it would
  let `σ₀ᵢ` widen for provisional ratings — the Bayesian-correct version of
  "trust established ratings more." Smooths the unrated/low-games volatility.
- **Handicap → ELO mapping** in the likelihood (§4), using the table to be
  provided.

## Appendix A — from the multiplier to `σ₀`, and the drift

For one game, prior `θᵢ ~ N(μ₀, σ₀²)`, the MAP update is one Newton step.
With `E = σ((θᵢ − θ_opp)/s)` the log-likelihood gradient in `θᵢ` is
`g = (S − E)/s` and its curvature is `h = E(1−E)/s²`. The posterior precision is
`1/σ₀² + h`; for a tight-ish prior `h ≪ 1/σ₀²`, so the mean shift is

```
Δθᵢ ≈ σ₀² · g = (σ₀² / s)(S − E).
```

Matching this to a FIDE Elo update `Δθᵢ = K(S − E)` gives the effective first-game
K-factor `K = σ₀² / s`, i.e.

```
σ₀ = √(K · s) = √(K · 400 / ln 10) ≈ 13.2 · √K.
```

As games accumulate the `h` terms add up and shrink the posterior variance
(`σₙ² ≈ 1/(1/σ₀² + Σ h)`), so the effective K decays — the auto-deceleration of
§2.3. Setting `K = m · K_FIDE(μ₀)` per player yields the per-player `σ₀ᵢ` used in
the solver.
