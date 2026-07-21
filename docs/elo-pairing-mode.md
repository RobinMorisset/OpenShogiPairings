# Experimental ELO-based (non-Swiss) pairing mode — design

Status: **V1 implemented** — the estimator ([`crates/core/src/elo.rs`](../crates/core/src/elo.rs)),
the ELO rule ladder in the pairing engine, the settings toggle + K multiplier +
provisional-rating multiplier (with the Swiss sections greyed out), the Results-tab
"Est. ELO" column, estimated ELO as a selectable ranking criterion
(`Tiebreak::EstElo`), and FESA-style handicap-game rating (§4) are in. Still open:
the V2 items in §8. Tracks the TODO item
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

The log-likelihood is concave and every prior is a **log-concave** density
(Gaussian by default, or the optional Laplace of §2.5 — both log-concave), so the
objective is concave and the maximiser is **unique and deterministic**. With the
default Gaussian prior it is additionally smooth and *strongly* concave; the
Laplace variant keeps concavity but is only piecewise-smooth, which §2.5 handles
with a small Huber core.

### 2.1 Prior for rated players — FESA K × a single multiplier

We express trust in the registration rating as a Gaussian prior
`θᵢ ~ Normal(μᵢ₀, σ₀ᵢ²)`, with `μᵢ₀` = the registration ELO. The one thing the
referee sets is a **single multiplier `m`** (default `1.0`, expected range ~1–4)
applied to FESA's rating-dependent development coefficient K (section 1 of the
[FESA ELO system](https://fesashogi.eu/elo-system/); its thresholds sit on grade
boundaries):

| Registration rating `μ₀` | FESA K |
| --- | --- |
| `μ₀ ≥ 2240` | 16 |
| `1920 ≤ μ₀ < 2240` | 20 |
| `1560 ≤ μ₀ < 1920` | 24 |
| `1280 ≤ μ₀ < 1560` | 28 |
| `1040 ≤ μ₀ < 1280` | 32 |
| `720 ≤ μ₀ < 1040` | 36 |
| `μ₀ < 720` | 40 |

(Bounds are inclusive-lower/exclusive-upper, matching the MacMahon threshold
convention in `settings.rs`.)

The per-player effective K is `Kᵢ = m · K_FESA(μᵢ₀)`, and the prior width is
derived from it (derivation in Appendix A):

```
σ₀ᵢ = √(Kᵢ · 400 / ln 10) ≈ 13.2 · √Kᵢ
```

`Kᵢ` is *literally the first-game K-factor* for player `i`: on the very first
game, the MAP update reduces to an Elo update `Δθᵢ ≈ Kᵢ·(S − E)` (Appendix A). So
round 1 behaves like a FESA Elo step with the player's K scaled by `m`; later
rounds automatically damp (§2.3). Stronger players get a smaller K and so a
tighter prior — matching the fact that established high ratings are more reliable.

Reference values (`σ₀` in ELO points):

| multiplier `m` | K range (16–40 → …) | σ₀ range |
| --- | --- | --- |
| 0.5 | 8–20 | 37–59 |
| 1 | 16–40 | 53–83 |
| 2 | 32–80 | 75–118 |
| 4 | 64–160 | 106–167 |

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

Both ends of this prior are **referee-tunable** (defaults reproduce the above): the
estimator's `unrated_prior_center` is the mean, and `unrated_k` sets the width via
the *same* `σ₀ = √(K · s)` law a rated player's K obeys — so the referee tunes one
familiar quantity rather than a raw standard deviation (a rated K is ~16–40; the
unrated default `705` gives `σ ≈ 350`). The center and K are stored as integers so
the settings stay `Eq`; `elo_unrated_k()` clamps K ≥ 1. The estimator is unaffected
by the overall multiplier `m` and the provisional multiplier for unrated players —
an unrated player has no registration rating to drift from or be provisional about,
so `unrated_k` is itself the only width knob. The simulator's oracle mirrors
these as `--oracle-unrated-center` / `--oracle-unrated-k` (its own truth-model
knobs, distinct from a variant's settings), so the true world's newcomer spread can
be studied independently of what a variant *believes* about newcomers.

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
the winner a *rated* 1100 player (`K_FESA = 32`, `m = 1` → `σ₀ ≈ 75`) beating a
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

### 2.5 Optional fatter-tailed / asymmetric prior — `EloPriorShape`

The Gaussian prior's restoring force is `(θ − μ₀)/σ₀²`, i.e. **linear and
unbounded**: it fights back ever harder the further the estimate drifts, so a run
of surprising results is capped near `K` per game (§2.4) and a genuinely
mis-seeded player is corrected only slowly. A referee can instead select
`EloPriorShape::Laplace`, a Huber-smoothed **asymmetric Laplace** prior of the
*same width* but with a **constant** restoring force `1/b` — exponential (hence
fatter) tails. A sustained streak against much-stronger opponents then moves the
estimate much further before the prior reins it in, at the cost of a *dead zone*:
below `≈ s/b` net surprise-wins the estimate does not move at all (the constant
force isn't yet overcome), and past it the estimate tracks the evidence.

**Asymmetry is a separate axis from the tail shape** — the per-category
upward-looseness knobs below apply to the Gaussian and the Laplace alike, so a
referee can pick fatter tails, an upward tilt, both, or neither.

- **Width mapping.** The Laplace scale is variance-matched to the Gaussian,
  `b_down = σ₀/√2`, so the existing knobs (`m`, provisional multiplier,
  `unrated_k`) keep their meaning — they still set `σ₀`, which sets `b_down`.
- **Asymmetry, per player category — and it works for *both* shapes.** The upward
  arm is widened by a ratio `r ≥ 1`: for the Laplace `b_up = r · b_down`, and for
  the Gaussian `σ_up = r · σ₀` (a **two-piece normal** — std `σ₀` below the mean,
  `r · σ₀` above). `r > 1` makes an *upward* revision (the common case — an
  under-rated improver) clear on less evidence than a downward one, while the
  downward arm stays as tight as before. `r = 1` is symmetric and, for the
  Gaussian, collapses back to the plain `N(μ₀, σ₀²)` exactly. `r` is set
  **separately for the three prior categories** — the estimator fields
  `upward_looseness_established`, `_provisional`, `_unrated` — because a global tilt
  is rarely what you want: a reliable FESA rating deserves little upward bias,
  whereas a newcomer who beats the field is far more likely genuinely strong than
  lucky, so the unrated (and, to a lesser degree, provisional) categories are where
  the asymmetry earns its keep. Each player reads the `r` of the same category that
  set its width (§2.1/§2.2). All three default to `1`.
- **Still log-concave.** For the Laplace, `−|d|` is concave, so the objective stays
  concave and the MAP unique; the only wrinkle is the kink at `d = 0`, where the
  Gaussian's finite curvature (`−1/σ₀²`) that keeps the Newton step well-defined is
  absent, so we round it off with a two-piece linear Huber core of half-width
  `HUBER_DELTA` (a couple of ELO points) **anchored at the origin** (`g(0) = 0`):
  this restores a strictly negative curvature near the kink *and* keeps the penalty
  minimised exactly at `μ₀`. The asymmetric **Gaussian** needs none of this — the
  two-piece normal is already `C¹` at the mode (both arms have zero slope there)
  and strictly concave, so plain Newton works unchanged. Either way the mode stays
  at `μ₀`, so a player with no games sits precisely at their registration rating
  regardless of `r`.

**`EloPriorShape::Flat` — the improper prior (`turnering.py` performance rating).**
A third shape drops the prior entirely: its contribution to the gradient and
Hessian is `(0, 0)`, so a player carrying it is estimated by the **likelihood
alone** — the maximum-likelihood performance rating over their games. This
reproduces the FESA rating program's treatment of unrated newcomers (the
`performance2` Newton solve), where a strong veteran arriving without an ELO is
rated straight off the field they beat, with no regularisation toward the unrated
centre. It is meant for the `prior_shape_unrated` slot; the upward-looseness
knobs don't apply (there's no arm to widen). Because a flat prior can't bound an
all-win or all-loss likelihood, those scorelines follow `turnering.py`'s guards,
applied in `estimate_elos`:

- **all games lost** → floored to `FLAT_ALL_LOSS_FLOOR = 1` (the bottom of the
  assumed `[1, 1200]` unrated range, matching turnering's new-rule floor);
- **all games won** → a pseudo-*draw* against the strongest opponent is added to
  the likelihood (turnering's `[best_elo] + elo_results`), giving the otherwise-
  monotone objective a finite maximum above the field;
- **no games** → the player's Hessian is `0`, so the sweep leaves them at their
  seed (the unrated centre) rather than dividing by zero.

A mixed scoreline needs no guard — the likelihood alone is strictly concave in
that player's `θ`. Note the flat prior supplies *no* curvature, so it relies on
the rated field's proper priors to pin the overall scale; on a normal tournament
(some rated players present) that is always satisfied.

Default is `Gaussian` with every `r = 1` — behaviour-neutral; nothing changes
until a referee raises a looseness knob or switches shape.

## 3. The solver

The objective is unconstrained and concave (strongly and smoothly so with the
Gaussian prior — even the asymmetric two-piece variant, which stays `C¹` and
strictly concave; still concave, with a piecewise-smooth Huber core, under the
optional Laplace prior of §2.5), so its maximiser is unique. Use **coordinate
ascent (Gauss–Seidel)** — no external LP/QP dependency, in keeping with the
hand-written blossom matcher (a single dense Newton solve works too; coordinate
ascent is just simpler and allocation-free).

For each player `i`, holding the others fixed, take a 1-D Newton step on the
concave 1-D objective:

```
gradient g = Σ_games (Sᵢg − Eᵢg)/s   − (θᵢ − μᵢ₀)/σ₀ᵢ²
hessian  H = Σ_games −Eᵢg(1−Eᵢg)/s²  − 1/σ₀ᵢ²
θᵢ ← θᵢ − g/H
```

where `Eᵢg = σ((θᵢ − θ_opp)/s)` and `(μᵢ₀, σ₀ᵢ)` is the rated prior (§2.1) or the
unrated prior `(600, 350)` (§2.2) — every player now carries a prior term, so
there is no special-cased branch. The `−(θᵢ − μᵢ₀)/σ₀ᵢ²` / `−1/σ₀ᵢ²` terms above are
the Gaussian prior's contribution; under the optional Laplace prior (§2.5) they are
replaced by that prior's constant restoring force and its Huber-core curvature,
computed by `PriorPenalty::grad_hess` — the rest of the sweep is identical. Sweep all players until the largest change in a
sweep is below a tolerance (a few ELO-hundredths). Strong concavity guarantees
convergence to the unique global maximum, so the result is deterministic given a
fixed tolerance and max-iteration cap.

Seed: rated players at `μᵢ₀`, unrated at `600`. Games that don't inform strength
(byes) are simply excluded from the sums. Handicap games *are* included, via the
per-side handicap offset described in §4.

Per-player posterior variance (`σ_current² = −1/H` at the optimum), if wanted
later (e.g. the results-simulation TODO or the caps of §2.3/§2.4), is available
for free.

## 4. Game-type handling

- **Draws** (`Board.drawn`): scored `S = ½` for each side — the standard
  Elo/Glicko pseudo-likelihood treatment. (A principled Davidson draw model would
  add another parameter; rejected for V1.)
- **Handicap games** (`Board.handicap`): rated using the **actual** result (who
  really won — not the standings' effective winner, which is always the giver) via
  the [FESA treatment](https://fesashogi.eu/elo-system/) (sections 7–8). The
  giver's registration rating is turned into a fractional **grade** number by
  interpolating the grades' lower-bound ratings (`GRADE_LB`), the handicap's
  grade value (Sente 0.2 … 6-piece 8.0) is subtracted, and converting back gives
  a rating-point drop `h`. That enters the logistic as a fixed offset:
  `P(giver wins) = σ((θ_giver − θ_recv − h)/s)` — `−h` on the giver, `+h` on the
  receiver. Using the *fixed* registration rating (like FESA) keeps `h` constant,
  so the likelihood stays log-concave and the solver is unchanged. Because the
  handicap shrinks the effective gap, a favourite who gives odds and still wins
  gains more than from an even win (and is punished more for losing).
- **Byes:** not a game — contribute nothing to the likelihood. "No repeat bye"
  stays in the rematch tier; *who* takes the bye is decided by a dedicated tier
  (§6).

## 5. Data & code layout

- New module `crates/core/src/elo.rs`, sibling to `scoring.rs` / `standings.rs`:
  the FESA-K table, the MAP solver, and `estimate_elos(...) -> Map<Uuid, f64>`
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
tie-break would almost never change a pairing — not worth the tier. Club protection
is a Swiss-only knob (`pairing.club_protection`, present only on the
`PairingMode::Swiss` variant) and is structurally absent from the `Elo` variant.

### 6a. Estimate-based MacMahon (`pairing.macmahon.source = from_estimate`)

An orthogonal way to hybridize — this one touches **scoring**, not the pairing
rule list, so it composes with plain Swiss pairing (it's moot under pure ELO,
which ignores MacMahon). When MacMahon's source is `from_estimate`, `compute_scores`
([`crates/core/src/scoring.rs`](../crates/core/src/scoring.rs)) awards each
player's MacMahon starting points from the **live ELO estimate** instead of their
registration rating: it calls `estimate_elos(...)` once and, for each player,
passes `round(estimate)` in place of `Player.rating` to
`macmahon_points_at(...)`. Only ELO-based thresholds are affected — grade
thresholds still read `Player.grade` — and because the estimate is a pure replay
recomputed every round, a player's MacMahon points can rise or fall between
rounds as their estimated strength moves (the score/standings architecture
already recomputes MacMahon each round via `rounds_played`, so this needed no new
plumbing).

`TournamentSettings::macmahon_from_estimate_active()` gates it: the toggle *and*
at least one ELO threshold to compare against (with only grade thresholds, or
none, the estimate would change nothing, so the estimator call is skipped, and
the UI greys the checkbox out). This is deliberately kept separate from
`elo_estimate_needed()` (pure ELO pairing) — the latter still gates only the *pairing* model's
ELO context, so plain Swiss + estimate-based MacMahon doesn't pay for the pairing
ELO context it wouldn't use. The `Tiebreak::EstElo` ranking criterion becomes
valid here too (a live estimate is maintained), so `normalized()` keeps it
whenever either `elo_estimate_needed()` **or** `macmahon_from_estimate_active()`.

Note this uses the same `k_multiplier` / `provisional_multiplier` estimator knobs
as pure-ELO pairing (the `EloEstimator` under `macmahon.source = from_estimate`
carries them); their inputs live in the pairing-mode section of the settings UI,
so under plain Swiss the estimator runs with the stored defaults.

## 7. Settings shape

> **Post-refactor note.** This section originally added flat `elo_*` /
> `elo_pairing_enabled` / `macmahon_from_estimated_elo` fields directly to
> `TournamentSettings`. The settings have since been refactored into the sum type
> [`PairingMode`](../crates/core/src/settings.rs): the pairing model is now the
> tagged `pairing` union (not a boolean), and the estimator knobs moved onto an
> [`EloEstimator`](../crates/core/src/settings.rs) struct carried by whichever model
> maintains a live estimate. The accessor methods below (`settings.elo_k_multiplier()`
> etc.) are unchanged — they read whichever estimator is in play — so only the field
> names differ. The mapping:
>
> | original flat name | current location |
> | --- | --- |
> | `elo_pairing_enabled: bool` | `pairing` = `{ "kind": "elo", "estimator": … }` (vs the `"swiss"` variant) |
> | `macmahon_from_estimated_elo: bool` | `pairing.macmahon.source` = `{ "kind": "from_estimate", "estimator": … }` (vs `{ "kind": "static" }`) |
> | `club_protection_*` | `pairing.club_protection` (Swiss variant only) |
> | `elo_k_multiplier_percent` | `estimator.k_multiplier` |
> | `elo_provisional_multiplier_percent` | `estimator.provisional_multiplier` |
> | `elo_unrated_prior_center` | `estimator.unrated_prior_center` |
> | `elo_unrated_k` | `estimator.unrated_k` |
> | `elo_prior_shape_{established,provisional,unrated}` | `estimator.prior_shape_{…}` |
> | `elo_upward_looseness_{…}_percent` | `estimator.upward_looseness_{…}` |

`pairing` is additive/defaulted (old saves load as plain Swiss). It is either
`{ "kind": "swiss", floater_style, airtight_groups, club_protection, macmahon }`
or `{ "kind": "elo", estimator }`:

- **Pure-ELO pairing** — the `PairingMode::Elo` variant. Mutually exclusive with
  MacMahon *by construction*: the Swiss-only knobs (thresholds, removals, floater
  style, fold, club protection) don't exist on this variant, and the UI greys out
  those sections. Selected by making `pairing` the `elo` variant rather than by a
  boolean toggle.
- **Estimate-based MacMahon** — `pairing.macmahon.source` =
  `{ "kind": "from_estimate", "estimator": … }` awards MacMahon from the live
  estimate rather than the registration rating (see §6a); `{ "kind": "static" }` is
  the default. Independent of the pairing model (composes with plain Swiss). Inert
  unless there's an ELO threshold (`macmahon_from_estimate_active()`); the UI greys
  the checkbox out until then.

The estimator knobs are the fields of `EloEstimator` (the same struct on the `elo`
pairing variant and on the `from_estimate` MacMahon source, so they mean the same
in either):

- `k_multiplier: u32` — the single knob `m`, stored as an integer
  percent (`100` = ×1.0) so `TournamentSettings` stays `Eq` (an `f64` field would
  break the derive, and the tournament `Eq` that the undo-snapshot store relies
  on). Read as a float via `settings.elo_k_multiplier()`. Default `100`, expected
  100–400; the UI presents it as a decimal multiplier. Normalization clamps it to
  ≥ 1 (a zero-width prior would be degenerate).
- `provisional_multiplier: u32` — an **extra** K multiplier for a
  *provisionally*-rated player (default `200` = ×2.0, clamped ≥ 100). A rated
  player is provisional when they aren't in the FESA list (`Player.fesa_games`
  is `None` — rating typed by hand) or their FESA game count is below
  `PROVISIONAL_GAMES_THRESHOLD` (18). It stacks on `m`
  (`K = m · K_FESA · provisional`), widening the prior so a shaky seed rating
  drifts faster. Unrated players are unaffected (they already get the wide
  `N(600, 350²)` prior). `Player.fesa_games` is captured at registration when the
  rating is picked from the FESA autocomplete (the parser now keeps the `#games`
  column). It's kept even if the referee then edits the *rating* by hand — the
  FESA list is often stale and referees routinely bump a known player's rating —
  and cleared only when the *last name* is edited (a possibly different entry) or
  the rating is removed entirely.
- `unrated_prior_center: u32` — the mean of the unrated prior (default `600`;
  see §2.2). Read as a float via `settings.elo_unrated_prior_center()`.
- `unrated_k: u32` — the K setting the unrated prior's width via `σ₀ = √(K·s)`
  (default `705 ≈ σ 350`; see §2.2). Read via `settings.elo_unrated_k()`, which
  clamps K ≥ 1; normalization stores the clamped value. Unlike `m` and the
  provisional multiplier, this is the *only* width knob for an unrated player.
- `prior_shape_{established,provisional,unrated}: EloPriorShape` — the prior
  shape **per player category**: `Gaussian` (default), the fatter-tailed,
  optionally asymmetric `Laplace`, or the improper `Flat` (the `turnering.py`
  performance rating — no prior; see §2.5). A plain enum (`#[serde(rename_all =
  "snake_case")]`), default-`Gaussian` for all three, so old saves and untouched
  tournaments keep the historical behaviour exactly. Splitting the shape per
  category lets a fat tail be confined to the one population whose true strength is
  genuinely heavy-tailed — **unrated** players, where most newcomers are weak but a
  few are strong veterans arriving without an ELO — while established and
  provisional players, well-anchored by their history, stay on the thin-tailed
  Gaussian (a fat tail there just loosens the anchor and adds noise). The Settings
  UI exposes a single selector bound to the *unrated* shape; established and
  provisional are written `Gaussian`.
- `upward_looseness_{established,provisional,unrated}: u32` — the
  asymmetry ratio `r` for the Laplace prior, **one per player category**, integer
  percent (`100` = ×1.0 = symmetric, the default for all three). Read via
  `settings.elo_upward_looseness_{established,provisional,unrated}()`;
  normalization clamps each ≥ 100 (an upward revision is never harder than a
  downward one). Each player reads the knob for the category that set its prior
  width. Applies to **both** prior shapes (the Gaussian becomes a two-piece
  normal, the Laplace widens its upward scale); the UI shows the three inputs
  whenever a live estimate is maintained.

There are ten estimator knobs (`k_multiplier`, `provisional_multiplier`,
`unrated_prior_center`, `unrated_k`, the three `prior_shape_*`, and the three
`upward_looseness_*`), but the Settings UI deliberately exposes only **two** derived
controls whenever a live estimate is maintained (pure ELO pairing, or estimate-based
MacMahon (§6a) with an ELO threshold): *Estimate* — unrated players only
(`k_multiplier = 0`, the default, pinning rated players to their registration
rating) or all players (`= 100`); and *Unrated prior* — the flat performance rating
(`prior_shape_unrated = flat`, the default) or a tuned asymmetric Laplace
(`laplace` with `unrated_prior_center = 700`, `unrated_k = 260`,
`upward_looseness_unrated = 300`). The remaining knobs (the provisional
multiplier, the per-category shapes and loosenesses) keep their defaults through
the UI and are reachable only via the settings JSON / the simulator CLI, where the
full estimator is still exercised for research. The FESA K table, `s`, and the
reliability threshold (18 games) remain constants in `elo.rs`, not settings;
`UNRATED_PRIOR_MEAN` / `UNRATED_PRIOR_DEFAULT_K` there are only the *defaults* for
the two settings above (and the fallback where no settings are in hand).

## Ranking

Estimated ELO is a selectable ranking criterion: `Tiebreak::EstElo`
(`Standing.tiebreak` returns `estimated_elo.max(0) as u32`), so the referee can
place it in the ranking order via the Settings tab like any other metric. When
it isn't in the ranking order, ELO mode still shows a dedicated informational
"Est. ELO" column; once added as a ranking criterion it appears as that
tie-break column instead (no duplicate).

## 8. V2 / deferred

- **Default ranking order in this mode.** EstElo is *available* as a ranking
  criterion but the default order is still the classic Swiss one; a mode-aware
  default (e.g. seed the order with EstElo when ELO mode is enabled) is left for
  when the standings work settles.

## Appendix A — from the multiplier to `σ₀`, and the drift

For one game, prior `θᵢ ~ N(μ₀, σ₀²)`, the MAP update is one Newton step.
With `E = σ((θᵢ − θ_opp)/s)` the log-likelihood gradient in `θᵢ` is
`g = (S − E)/s` and its curvature is `h = E(1−E)/s²`. The posterior precision is
`1/σ₀² + h`; for a tight-ish prior `h ≪ 1/σ₀²`, so the mean shift is

```
Δθᵢ ≈ σ₀² · g = (σ₀² / s)(S − E).
```

Matching this to an Elo update `Δθᵢ = K(S − E)` gives the effective first-game
K-factor `K = σ₀² / s`, i.e.

```
σ₀ = √(K · s) = √(K · 400 / ln 10) ≈ 13.2 · √K.
```

As games accumulate the `h` terms add up and shrink the posterior variance
(`σₙ² ≈ 1/(1/σ₀² + Σ h)`), so the effective K decays — the auto-deceleration of
§2.3. Setting `K = m · K_FESA(μ₀)` per player yields the per-player `σ₀ᵢ` used in
the solver.
