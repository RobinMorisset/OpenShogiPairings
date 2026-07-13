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

There is also a **mixed mode** (`mixed_elo_pairing_enabled`), a middle ground
between Swiss and this (pure) ELO mode: it keeps MacMahon and the Swiss
score-group rules (score gap, float repeat, club, airtight groups, bye group)
but replaces just `Rule::Fold` and `Rule::FloaterSelection` with `Rule::EloGap`,
so within- and across-group ordering follows the live estimate instead of a
static registration rating — unlike pure ELO mode, it stays fully compatible
with MacMahon points. See §6a.

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

Both ends of this prior are **referee-tunable** (defaults reproduce the above):
`elo_unrated_prior_center` is the mean, and `elo_unrated_k` sets the width via the
*same* `σ₀ = √(K · s)` law a rated player's K obeys — so the referee tunes one
familiar quantity rather than a raw standard deviation (a rated K is ~16–40; the
unrated default `705` gives `σ ≈ 350`). The center and K are stored as integers so
the settings stay `Eq`; `elo_unrated_k()` clamps K ≥ 1. The estimator is unaffected
by the overall multiplier `m` and the provisional multiplier for unrated players —
an unrated player has no registration rating to drift from or be provisional about,
so `elo_unrated_k` is itself the only width knob. The simulator's oracle mirrors
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

- **Width mapping.** The Laplace scale is variance-matched to the Gaussian,
  `b_down = σ₀/√2`, so the existing knobs (`m`, provisional multiplier,
  `elo_unrated_k`) keep their meaning — they still set `σ₀`, which sets `b_down`.
- **Asymmetry, per player category.** The upward arm is widened to
  `b_up = r · b_down`, where `r ≥ 1`. `r > 1` makes an *upward* revision (the
  common case — an under-rated improver) clear on less evidence than a downward
  one, while the downward arm stays as tight as before. `r = 1` is symmetric. `r`
  is set **separately for the three prior categories** — `elo_upward_looseness_
  established`, `_provisional`, `_unrated` — because a global tilt is rarely what
  you want: a reliable FESA rating deserves little upward bias, whereas a newcomer
  who beats the field is far more likely genuinely strong than lucky, so the
  unrated (and, to a lesser degree, provisional) categories are where the
  asymmetry earns its keep. Each player reads the `r` of the same category that
  set its width (§2.1/§2.2). All three default to `1`, so the prior stays neutral
  about direction until a referee opts in.
- **Still log-concave.** `−|d|` is concave, so the objective stays concave and the
  MAP unique. The only wrinkle is the kink at `d = 0`, where the Gaussian's finite
  curvature (`−1/σ₀²`) that keeps the Newton step well-defined is absent. We round
  it off with a two-piece linear Huber core of half-width `HUBER_DELTA` (a couple
  of ELO points) **anchored at the origin** (`g(0) = 0`): this restores a strictly
  negative curvature near the kink *and* keeps the penalty minimised exactly at
  `μ₀`, so a player with no games still sits precisely at their registration
  rating regardless of `r`.

Default is `Gaussian`, `r = 1` — behaviour-neutral; nothing changes until a
referee switches shape.

## 3. The solver

The objective is unconstrained and concave (strongly and smoothly so with the
default Gaussian prior; still concave, with a piecewise-smooth Huber core, under
the optional Laplace prior of §2.5), so its maximiser is unique. Use **coordinate
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
(byes; handicap games in V1 — see §4) are simply excluded from the sums.

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
tie-break would almost never change a pairing — not worth the tier. The
`club_protection_*` settings are simply ignored when the ELO mode is on.

### 6a. Mixed mode: `EloGap` as a drop-in replacement for `Fold` + `FloaterSelection`

Mixed mode (`mixed_elo_pairing_enabled`) uses a different rule list, still built
entirely from existing `Rule` variants — no new rule was needed:

```
Rematch, ByeGroup, AirtightGroups, ScoreGap, FloatRepeat, Club, EloGap
```

This is exactly the Swiss list with `FloaterSelection` and `Fold` removed and
`EloGap` appended at the bottom (lowest priority — it's the finest-grained
tiebreak, same role `Fold` played). Everything above it — MacMahon-derived
score groups (`ScoreGap`, `AirtightGroups`, `ByeGroup`), no-repeat-float
(`FloatRepeat`), and club protection — is untouched, so a group is still formed
exactly as in Swiss; only *how players are ordered inside (and across) that
group* changes, from a static fold-by-registration-rating to a live,
result-reactive ELO estimate. Concretely: `EloGap`'s edge cost
`(round(eloᵢ) − round(eloⱼ))²` fires on every edge regardless of score group
(unlike `Fold`, which is zero across groups, and `FloaterSelection`, which is
zero within one) — but since `ScoreGap` sits above it in priority, cross-group
pairings are still only chosen when a float is unavoidable, same as Swiss;
`EloGap` merely decides *which* players end up on each side of that float and
how they're matched within a group.

`PairingModel::build` computes the live ELO estimate (`elo`/`elo_rank`/
`max_elo_gap`) whenever `TournamentSettings::elo_estimate_needed()` is true —
i.e. either ELO mode — rather than gating on `elo_pairing_enabled` alone, so
mixed mode gets the same estimator ([`crates/core/src/elo.rs`](../crates/core/src/elo.rs))
feeding both the pairing rule and (optionally) the `Tiebreak::EstElo` ranking
criterion. `elo_rank` (used only by pure ELO mode's `ByeSelection`) is simply
unused in mixed mode, which keeps `ByeGroup` (the MacMahon-aware bye rule)
instead.

### 6b. Estimate-based MacMahon (`macmahon_from_estimated_elo`)

A third, orthogonal way to hybridize — this one touches **scoring**, not the
pairing rule list, so it composes with *any* pairing mode (plain Swiss or mixed
ELO; it's moot under pure ELO, which ignores MacMahon). When
`macmahon_from_estimated_elo` is on, `compute_scores`
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
`elo_estimate_needed()` (§6a) — the latter still gates only the *pairing* model's
ELO context, so plain Swiss + estimate-based MacMahon doesn't pay for the pairing
ELO context it wouldn't use. The `Tiebreak::EstElo` ranking criterion becomes
valid here too (a live estimate is maintained), so `normalized()` keeps it
whenever either `elo_estimate_needed()` **or** `macmahon_from_estimate_active()`.

Note this uses the same `elo_k_multiplier_percent` / `elo_provisional_multiplier_percent`
knobs as the pairing modes; their inputs live in the pairing-mode section of the
settings UI, so under plain Swiss the estimator runs with the stored defaults.

## 7. Settings shape

Add to `TournamentSettings` (additive, defaulted, so old saves still load):

- `elo_pairing_enabled: bool` — the pure-ELO mode switch. Mutually exclusive with
  MacMahon in the UI (greys out thresholds, removals, floater style, fold, and the
  club-protection controls — implemented by wrapping those sections in a disabled
  `<fieldset>`). Off by default.
- `mixed_elo_pairing_enabled: bool` — the mixed-mode switch (see §6a). Mutually
  exclusive with `elo_pairing_enabled` (`normalized()` clears this one if both are
  set, so pure ELO wins). Unlike pure ELO mode, it only greys out the
  floater-selection section — MacMahon, degressive removals, airtight groups and
  club protection stay active. Off by default.
- `macmahon_from_estimated_elo: bool` — award MacMahon from the live estimate
  rather than the registration rating (see §6b). Independent of the pairing-mode
  switches (composes with Swiss or mixed ELO). Inert unless there's an ELO
  threshold (`macmahon_from_estimate_active()`); the UI greys the checkbox out
  until then. Off by default.
- `elo_k_multiplier_percent: u32` — the single knob `m`, stored as an integer
  percent (`100` = ×1.0) so `TournamentSettings` stays `Eq` (an `f64` field would
  break the derive, and the tournament `Eq` that the undo-snapshot store relies
  on). Read as a float via `settings.elo_k_multiplier()`. Default `100`, expected
  100–400; the UI presents it as a decimal multiplier. Normalization clamps it to
  ≥ 1 (a zero-width prior would be degenerate).
- `elo_provisional_multiplier_percent: u32` — an **extra** K multiplier for a
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
- `elo_unrated_prior_center: u32` — the mean of the unrated prior (default `600`;
  see §2.2). Read as a float via `settings.elo_unrated_prior_center()`.
- `elo_unrated_k: u32` — the K setting the unrated prior's width via `σ₀ = √(K·s)`
  (default `705 ≈ σ 350`; see §2.2). Read via `settings.elo_unrated_k()`, which
  clamps K ≥ 1; normalization stores the clamped value. Unlike `m` and the
  provisional multiplier, this is the *only* width knob for an unrated player.
- `elo_prior_shape: EloPriorShape` — `Gaussian` (default) or the fatter-tailed,
  optionally asymmetric `Laplace` (see §2.5). A plain enum (`#[serde(rename_all =
  "snake_case")]`), default-`Gaussian`, so old saves and untouched tournaments keep
  the historical behaviour exactly.
- `elo_upward_looseness_{established,provisional,unrated}_percent: u32` — the
  asymmetry ratio `r` for the Laplace prior, **one per player category**, integer
  percent (`100` = ×1.0 = symmetric, the default for all three). Read via
  `settings.elo_upward_looseness_{established,provisional,unrated}()`;
  normalization clamps each ≥ 100 (an upward revision is never harder than a
  downward one). Each player reads the knob for the category that set its prior
  width. Inert under the Gaussian prior; the UI only shows the three inputs when
  the shape is Laplace.

The eight `elo_*` estimate knobs (`elo_k_multiplier_percent`,
`elo_provisional_multiplier_percent`, `elo_unrated_prior_center`, `elo_unrated_k`,
`elo_prior_shape`, and the three `elo_upward_looseness_*_percent`)
surface in the Settings UI whenever a live estimate is actually maintained — either
ELO pairing mode, or estimate-based MacMahon (§6b) with an ELO threshold. The FESA
K table, `s`, and the reliability threshold (18 games) remain constants in
`elo.rs`, not settings; `UNRATED_PRIOR_MEAN` / `UNRATED_PRIOR_DEFAULT_K` there are
only the *defaults* for the two settings above (and the fallback where no settings
are in hand).

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
