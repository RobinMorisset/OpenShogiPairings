# Pairing explanations — design doc

## Goal

Give referees a way to understand *why* the engine produced a particular set of
pairings, in the vocabulary of the pairing rules they already know — not a wall
of edge weights.

The engine solves a **minimum-weight perfect matching** where the scalar edge
weight is a lexicographic scalarization of the rule ladder (see the module docs
in [`pairing.rs`](../crates/core/src/pairing.rs)). A global optimum has no local
"reason" for any single board; the honest answer to *"why is A paired with B?"*
is *"because that lets everyone else pair better than any alternative."* So the
design is built on three ideas:

1. **A per-board ledger, always on.** For each Swiss board, the exact per-rule
   penalty units it carries, surfaced as a quiet glyph + tooltip. Answers *"is
   this board a compromise, and on what?"* at a glance.
2. **A per-round report.** How many times each rule had to be relaxed across the
   whole round — which is, by construction, the *minimum* achievable for this
   field. Frames the compromises as forced, not chosen.
3. **A counterfactual probe.** The referee names an alternative they had in mind
   — *"why not pair A and B?"* (force) or *"why did you pair A and B?"* (forbid)
   — and the engine re-solves, showing the **chain of players whose board would
   change** and, per changed board, the **highest-priority rule that flipped**.

The counterfactual is the centrepiece; the ledger and report are the cheap
always-on context around it.

## Non-goals (V1)

- Explaining cup or referee-forced boards beyond "this was fixed by X" — they
  never went through the engine, so there is nothing to explain.
- Explaining colours (sente/gote is random and untracked) or ELO drift.
- A full "what-if" sandbox. The counterfactual answers one named alternative at
  a time, not arbitrary multi-edge edits.

---

## Core concepts that make this cheap

Two properties of the existing engine do the heavy lifting:

**The unit vector is already computed pre-scalarization.** `Rule::edge_units`
and `Rule::bye_units` return the penalty units for a single rule on a single
edge, *before* the priority multiplier. So for any pairing we can recover the
full `[units per rule]` vector by iterating `active_rules` — no need to invert
the scalar weight. "The binding rule on this board" is simply *the first rule in
priority order with `units > 0`.* A before/after comparison is a straight diff
of two unit vectors.

**The ladder is derived from a rules list.** `scale_ladder` takes a
priority-ordered `max_total` slice and derives strictly-separated multipliers
bottom-up. Appending a new lowest-priority rule automatically gets a multiplier
below every real rule — so it can only ever break ties among already-optimal
matchings, never distort the real optimum. This is what makes the
minimal-perturbation guarantee (below) a one-line addition rather than a
special-cased re-solve.

### The stability tier (minimal-perturbation re-solve)

When we force/forbid an edge and re-solve, ties in the remaining problem let the
matcher return an equal-cost matching that reshuffles more boards than necessary
— making the "affected players" list look scarier than the real consequence.
Ties are rare in mid-Swiss but real at the bottom of the ladder (`Fold` has no
rule beneath it) and common in ELO mode (rounded estimates + unrated players at
the shared prior are interchangeable, and `EloGap` is the last rule).

Fix: for the re-solve only, append one rule at the bottom of the ladder —

```rust
/// (Explanation only) Prefer keeping each player with their baseline partner.
/// Emits 1 unit for any edge/bye NOT in the baseline matching, so minimizing it
/// minimizes the number of boards that change from the baseline. Sits strictly
/// below every real rule (guaranteed by scale_ladder), so it only ever breaks
/// ties among already-optimal matchings.
Stability,
```

with `edge_units = 1` iff `(a, b)` is not a baseline edge, `bye_units = 1` iff
`player` is not the baseline bye, and `max_total_units = edges` (at most every
edge can differ). The re-solve then minimizes `(true rule cost, then boards
changed)` lexicographically — exactly the minimal cascade — using the ladder
machinery that already exists. `Ctx` gains one field: `baseline: Option<&HashSet<UnorderedPair>>`.

---

## Core (Rust) work

### 1. Extract a reusable pairing model

`pair_round_weighted` currently builds `Ctx`, the multiplier ladder, and the
cost matrix inline. Faithfulness *requires* that explanations use the identical
construction, so extract it rather than duplicate:

```rust
/// Everything needed to score and solve one round's Swiss pool, built once and
/// reusable for both pairing and explanation. Owns the derived maps; lends a Ctx.
pub(crate) struct PairingModel<'a> {
    free: Vec<Uuid>,          // free vertices, in matrix order
    need_phantom: bool,       // a bye vertex is appended
    rules: &'static [Rule],
    mult: Vec<i128>,
    // owned: scores, by_player, fold, elo, elo_rank, exempt_clubs, ...
}

impl<'a> PairingModel<'a> {
    /// Build from the same inputs pair_round_weighted takes (players, settings,
    /// completed_rounds, the free/present set). No forced/bye handling here —
    /// the caller pre-places those, exactly as today.
    pub(crate) fn build(...) -> Self;

    fn ctx(&self, baseline: Option<&HashSet<UnorderedPair>>) -> Ctx<'_>;

    /// Per-rule units for pairing a vs b (or the bye), in priority order.
    pub(crate) fn edge_units(&self, a: Uuid, b: Uuid) -> Vec<i128>;
    pub(crate) fn bye_units(&self, player: Uuid) -> Vec<i128>;

    /// Build the cost matrix (optionally with a Stability tier and/or a set of
    /// forbidden edges forced to a prohibitive weight) and solve it.
    pub(crate) fn solve(&self, opts: SolveOpts) -> Matching;
}
```

`pair_round_weighted` is rewritten to `PairingModel::build(...).solve(default)`
plus its existing forced/bye pre-placement and board assembly. Behaviour is
unchanged; the tests in `pairing.rs` are the regression guard.

`UnorderedPair` is a tiny `(Uuid, Uuid)` newtype normalized so `(a,b) == (b,a)`
(the tests already have an `unord` helper to lift).

### 2. Reconstruct a round's inputs

Given a confirmed round at index `i`, the pairing inputs are:

- `completed_rounds` = `rounds[..i]` (everything before it).
- `settings`, `players` = current tournament state.
- The Swiss free pool = round players **not** in a `Forced` or `Cup` board and
  **not** the bye's… actually the bye *is* part of the Swiss pool. Precisely:
  the union of both players of every `Swiss` board, plus the `bye`.
- `forced_boards` = the round's `Forced` boards; `forced_bye` = none (a forced
  bye shows up as `bye` but we treat the whole Swiss pool including the bye as
  the free set, matching how the round was actually solved).

Everything needed is already on the `Round` (`boards[].source`, `bye`,
`absent`). Add:

```rust
impl Tournament {
    /// The Swiss free set, forced boards, and completed-rounds slice that
    /// reproduce `rounds[i]`'s engine solve. None if i is out of range.
    fn reconstruct_pairing(&self, round_index: usize) -> Option<PairingInputs<'_>>;
}
```

### 3. The round explanation (ledger + report)

```rust
/// One rule's contribution on one board (or the bye).
pub struct RuleContribution {
    pub rule: RuleId,   // serde enum mirroring Rule, mode-aware
    pub units: i64,     // pre-multiplier penalty units
}

pub struct BoardLedger {
    pub player1: Uuid,
    pub player2: Uuid,           // == bye sentinel handling: see below
    pub contributions: Vec<RuleContribution>, // only rules with units > 0
    pub binding: Option<RuleId>, // highest-priority rule with units > 0
}

pub struct RuleTotal {
    pub rule: RuleId,
    pub boards: u32,    // how many boards this rule was relaxed on
    pub units: i64,     // total units across the round
}

pub struct RoundExplanation {
    pub round: u32,
    pub boards: Vec<BoardLedger>,   // Swiss boards only, aligned to display order
    pub bye: Option<BoardLedger>,   // the bye's own ledger, if any
    pub report: Vec<RuleTotal>,     // rules with nonzero totals, priority order
}

impl Tournament {
    pub fn explain_round(&self, round_number: u32) -> Result<RoundExplanation, TournamentError>;
}
```

`explain_round` builds the `PairingModel`, and for each Swiss board/bye asks for
its unit vector. `report` is the column-sum of the board ledgers.

### 4. The counterfactual (force — shipped)

Both directions ship (force in phase 2, forbid in phase 3). What shipped:

```rust
/// Why a probed player is out of the engine's hands.
pub enum ScopeReason { Forced, Cup, Absent }

/// One rule's net change: signed units, positive = the alternative is worse.
pub struct RuleDelta { pub rule: RuleId, pub units: i64 }

/// A vertex-disjoint alternating cycle in baseline Δ counterfactual: the ring of
/// players who reshuffle to honour the probe. The bye appears as the nil UUID.
pub struct AffectedCycle { pub players: Vec<Uuid> }

pub struct Counterfactual {
    pub scoped_out: Option<ScopeReason>, // A or B not an engine-paired Swiss player
    pub cost_delta: Vec<RuleDelta>,      // signed per-rule net change, priority order
    pub cycles: Vec<AffectedCycle>,      // the affected rings (structure/story)
    pub changed: Vec<BoardLedger>,       // the new boards (added edges), each a ledger
}

impl Tournament {
    pub fn explain_counterfactual(&self, round_number: u32, a: Uuid, b: Uuid)
        -> Result<Counterfactual, TournamentError>;
}
```

Two refinements from the original sketch above:

- **`changed` is the set of *new* boards** (the added M1 edges), each carrying
  its own ledger — not `before`/`after` pairs. The before-state is recoverable
  from the `cycles`, and the net rule movement lives in `cost_delta` (signed
  `M1 − M0` totals). Simpler payload, same information.
- **The stability tier is folded in arithmetically**, not as a `Rule::Stability`
  variant: `solve_stable` costs each edge `real_cost · (edges + 1) + (edge not in
  baseline)`. Since `real_cost` is already the correct lexicographic scalar and
  the stability term is `≤ edges < edges + 1`, this *is* the lexicographic order
  `(real rules, then boards changed)` — identical argmin to appending a
  lowest-priority rule to `scale_ladder`, but localized to the re-solve and with
  no new `RuleId` to thread through serialization.

Mechanics (shared `baseline_matching` + `diff_matchings`, one solve each):

1. Build the `PairingModel` for the round (§2). Baseline `M0` = the round's Swiss
   boards + `(bye, PHANTOM)` as normalized pairs (`PHANTOM` = nil UUID).
2. **Force**: pre-place `A–B` (drop both from the vertex set; the phantom stays
   if there was a bye), `solve_stable(rest, baseline = M0)`, add the edge back →
   `M1`. **Forbid**: keep the full vertex set, `solve_stable(all, baseline = M0,
   forbidden = {A–B})` → `M1`. Forbidding prices the edge above any matching that
   avoids it (`max·edges + 1`), so it's never chosen while an alternative exists.
3. `M0 Δ M1` decomposes into vertex-disjoint alternating cycles (`alternating_
   cycles`) — the affected rings; the probed edge sits on one.
4. `cost_delta` = per-rule `Σ(added units) − Σ(removed units)` (unchanged boards
   cancel), keeping only rules that moved.
5. `changed` = the added edges as ledgers (a board with no `player2` is a new
   bye).
6. Scope guard: if `A` or `B` isn't an engine-paired Swiss player (or the bye),
   return early with `scoped_out`. No feasibility guard — forcing a rematch is a
   valid, just costly matching whose price shows up as a `Rematch` delta.

Because the picker only offers this round's Swiss players, the UI never triggers
`scoped_out` itself; it's a safety net for direct API callers.

The bye is the phantom vertex throughout, so "player X now takes the bye" falls
out of the diff naturally and renders as "(bye)" on the frontend.

### 4a. Applying a force — `force_pairing` (phase 3)

The "force this pairing" action closes the loop from *"why not?"* to *"then do
it."* `Tournament::force_pairing(a, b)` re-pairs the **current** round with
`a–b` fixed, by reconstructing the draft the round came from (its absentees and
existing forced boards, plus the new forced pair) and running `confirm_round`
again. Reusing `confirm_round` means all the cup/forced/validation machinery and
board ordering come for free. It refuses (`RoundHasResults`) if the round is
completed or already has any recorded result, since re-pairing would discard it.

`POST /api/tournament/rounds/force-pairing` (mutating; takes a backup). The
frontend offers the action only after a *force* probe with a non-empty diff, on a
round that is current, in progress, and result-free (`canForce`).

Note: `force_pairing` re-solves through `confirm_round` (no stability tier), so
with the rare tie it may differ slightly from the previewed minimal cascade —
acceptable, since ties are rare and `confirm_round` is the single source of truth
for how rounds are built.

### 5. Rule identity for serialization

Add a `RuleId` serde enum mirroring `Rule` (`rematch`, `score_gap`,
`float_repeat`, `floater_selection`, `club`, `fold`, `bye_selection`,
`elo_gap`), plus a `Rule::id()` accessor. `Stability` is internal and never
serialized (it carries no explanatory meaning — it's a tiebreaker). The frontend
maps `RuleId` → localized label.

---

## Server API

All read-only (no mutation, no backup). Explanations are computed on demand from
current state.

```
GET  /api/tournament/rounds/{number}/explanation
       → 200 RoundExplanation
       → 404 if the round doesn't exist

POST /api/tournament/rounds/{number}/counterfactual
       body: { "mode": "force" | "forbid", "a": Uuid, "b": Uuid }
       → 200 Counterfactual
       → 400 if a == b or ids unknown
       → 404 if the round doesn't exist
```

Handlers follow the existing pattern in
[`crates/server/src/tournament.rs`](../crates/server/src/tournament.rs): take a
read lock (`state.store.read()`), call the core method, `Json`-serialize. No
`backup_after` — these don't mutate. Wire both routes in `routes()` next to the
existing `/rounds` routes.

`RoundExplanation` / `Counterfactual` derive `Serialize`; `Probe`'s wire form is
the flat `{mode, a, b}` body above (kept separate from the internal enum so the
JSON stays simple).

---

## Frontend

All three surfaces live on the **current (uncompleted) round's**
[`RoundView.svelte`](../frontend/src/lib/components/RoundView.svelte), which is
where a referee reviews fresh pairings and decides whether to override. (The
mechanism generalizes to any past round, but V1 scopes the UI to the live one.)

### Types (`types.ts`)

Mirror the serde structs: `RuleId`, `RuleContribution`, `BoardLedger`,
`RuleTotal`, `RoundExplanation`, `ChangedBoard`, `AffectedCycle`,
`Counterfactual`. Add a `ruleLabel(rule: RuleId): string` helper backed by i18n,
alongside the existing `pairingSource.ts` badge helper.

### 1. Always-on ledger glyph + tooltip

In the board row, between the two players (next to the existing `sourceEmoji`
slot), render a small warning glyph when that board's `BoardLedger` has a
**noteworthy** contribution. Fold is deliberately excluded from the trigger: a
score group almost never folds perfectly, so a fold deviation fires on most (and
possibly every) board and carries no signal — a glyph on all of them is just
noise. So the glyph keys on any rule *above* `Fold` (score gap, repeat float,
floater selection, club; and their ELO-mode equivalents). The tooltip lists
those in words, e.g.:

> Compromise on: **score gap** (floated across groups), **repeat float**

Fold still appears, but only in the full ledger on an expander, never as a
standalone reason to flag a board. Clean boards render nothing — the column
stays quiet, drawing the eye only to the boards that actually involved a
meaningful trade-off. The glyph is the visible anchor (tooltips alone can't tell
the referee *which* board to hover).

The explanation is fetched once per round load (`GET …/explanation`) and indexed
by board to align with `round.boards` display order (the core returns them in
the same order `confirm_round` stored).

### 2. Per-round report

A collapsible line above the board list:

> **Why these pairings?** 3 downfloats, 1 repeat float. [details ▾]

Expanded, it lists each `RuleTotal` with its count. No editorializing about
optimality — referees know the engine minimized these; the numbers speak for
themselves. Fold is reportable here (it's a total, not a per-board flag) but
listed last, consistent with its priority.

### 3. Counterfactual probe

A "Question a pairing" affordance opening a small panel:

- Two player pickers (`<select>`s of the round's Swiss players), or a
  click-two-players interaction on the board list.
- A toggle: *"Why not paired?"* (force) / *"Why paired?"* (forbid) — the latter
  pre-fills when the referee clicks an existing board.
- On submit, `POST …/counterfactual`, then render:
  - If `scoped_out`: "A's board was set by the referee/cup — the engine didn't
    choose it."
  - Otherwise the **affected chain** as a readable ring — "To pair **A–B**:
    **X** now plays **Y** → **Z** takes the bye" — with the net `cost_delta`
    summarized as "worse on *repeat float*, no better anywhere." Per-board flipped
    rules are surfaced for the top-priority changes and folded for the rest
    (§4). Forcing a rematch simply shows a `Rematch` flip ("they'd have to
    replay"), no special-casing.

Reuse `RoundView`'s `name()` for player rendering. Highlighting the affected
boards inline on the list (rather than only in the panel) makes the cascade
tangible.

### i18n

New keys under `roundView.explanation.*` in
[`en.json`](../frontend/src/lib/i18n/locales/en.json) /
[`fr.json`](../frontend/src/lib/i18n/locales/fr.json): one label per `RuleId`,
the report template, the glyph tooltip template, and the counterfactual panel
copy. Rule labels are the referee-facing vocabulary ("downfloat", "repeat
float", "club clash", "fold"), not the internal enum names.

---

## Testing

**Core**

- `explain_round` ledger equals `edge_units` for each board (round-trip against
  the same `PairingModel` the pairing used). *(shipped:
  `explain_ledger_matches_the_engine_units`)*
- A fold-only board produces no *noteworthy* contribution but does report a Fold
  total. *(shipped: `explain_flags_fold_deviation_as_binding`,
  `explain_clean_pairing_has_no_contributions`)*
- Force swaps a **minimal ring**: forcing a non-fold pairing changes exactly the
  forced board and its forced completion, a single 4-player cycle. *(shipped:
  `forcing_a_pairing_swaps_a_minimal_ring` — the stability tie-break keeps
  everything else fixed.)*
- Forcing the status quo changes nothing; forcing a worse pairing reports a
  positive `cost_delta`; forcing across a bye reassigns the sit-out (a changed
  board with no `player2`). *(shipped as three more tests.)*
- Forbid drops the engine's board and re-pairs without it (worse `cost_delta`);
  forbidding an unused pairing is a no-op. *(shipped.)*
- `force_pairing` re-pairs the same round with `a–b` as a forced board and
  refuses once a result is recorded (`RoundHasResults`). *(shipped in
  `tournament.rs`.)*

**Server**: the two endpoints return 200 with the expected shape; 404 for a
missing round; 400 for `a == b` or a non-`force` mode.

**Frontend** (verified in-browser against the American-Grid fixture): the glyph
appears only on boards relaxed above fold; the report line and expander render;
the probe panel returns a normal diff (cost summary + changed boards) and the
"already paired" no-change case.

---

## Phasing

1. **Ledger + report** — *shipped.* Core §1–3, `GET …/explanation`, frontend
   surfaces 1–2. The always-on layer.
2. **Counterfactual force** — *shipped.* Core §4 force path (arithmetic stability
   tie-break), `POST …/counterfactual`, frontend surface 3. The centrepiece.
3. **Forbid direction** and the "force this pairing" action — *shipped.* Forbid
   re-solves with the edge priced out; the apply action re-pairs the current
   round through `confirm_round` with the probed edge as a `Forced` board,
   closing the loop from *"why not?"* to *"then do it."*

---

## Open questions

- **Numbers vs. words in the ledger.** Show raw units ("score gap 4") or purely
  qualitative ("floated two groups")? Leaning qualitative for the tooltip, with
  units available on a details expander.
- **ELO-mode labels.** The ledger is mode-agnostic (it iterates `active_rules`),
  but `elo_gap` / `bye_selection` need their own referee-facing copy; the report
  framing ("minimum required") still holds.
- **Past rounds.** The core generalizes to any round; do we expose the UI on
  completed rounds too, or keep it to the live one? (V1: live only.)
- **Counterfactual verbosity.** The core returns the full changed set; the
  frontend folds all but the top-priority flips (§4). The exact fold threshold
  ("top rule only" vs. "top two tiers") wants tuning against real fields.
- **Large fields.** The counterfactual is one extra O(V³) blossom solve —
  identical cost to pairing, already run at confirm time — so no new performance
  concern for realistic fields.
