# TODO

Known limitations and future work, roughly ordered by area.

## Pairing

- **ILP/CP-SAT backend.** Pairing is now a real **minimum-weight perfect
  matching** (integer blossom in [`crates/core/src/matching.rs`](crates/core/src/matching.rs))
  over a rule-weighted graph ([`crates/core/src/pairing.rs`](crates/core/src/pairing.rs)):
  rematch/repeat-bye ≫ score gap² ≫ float-repeat ≫ floater-selection ≫ same-club ≫
  within-group fold, ordered by a scalar multiplier ladder whose tiers are derived
  from each rule's worst-case contribution (so the lexicographic separation is
  exact, not hand-tuned). An ILP/CP-SAT backend is still motivated by experimental
  formats (MacMahon-beyond, hard multi-round constraints) that need constraints a
  plain matching can't express, and by very large fields. Plan: `good_lp` + HiGHS
  first, then CP-SAT.
- ~~**Make the blossom solver generic over the weight type.**~~ Done:
  [`min_weight_perfect_matching`](crates/core/src/matching.rs) is now generic
  over a small internal `Weight` trait (`i32`/`i64`/`i128` implement it).
  [`pairing.rs`](crates/core/src/pairing.rs)'s `solve_matching` still builds the
  cost matrix in `i128` (so scoring itself can never overflow), then picks the
  narrowest of `i32`/`i64`/`i128` that comfortably holds the matrix's largest
  value before handing it to the solver — most tournaments' ladders fit in
  `i32`.
- **Extract the blossom solver into its own crate.** `matching.rs` is fully
  self-contained (no `use crate::…`, a single pure-function interface, its own
  brute-force oracle test) and has one consumer, so it's the cleanest extraction
  candidate in the workspace. Not worth the extra crate today, but pull it out
  (`crates/matching`, an original MIT-clean `blossom`/min-weight-matching crate)
  once a trigger fires: a second consumer (CLI, the ILP/CP-SAT backend wanting to
  A/B against blossom), a publish intent, or independent benchmarking/fuzzing
  without compiling osp-core. Best done together with the generic-weight change
  above, so the crate lands with a reusable interface rather than an `i128`-only
  one.
- **No-shows.** A player who was paired for a round but did not show up (distinct
  from a game simply not yet recorded) should appear as `0#` in the results
  table. This needs a way to mark a board as a no-show (a new board-result state,
  giving the opponent the win); not handled yet. Byes (`0+`, win) and absences
  (`0-`, loss) are already handled.
- **Airtight groups** Extra rule saying that for the first N rounds, players with different
  MM points should not play each other. Priority: just below rematch.
- **MM by rank** A new way to set thresholds, it requires updating the players list and fesa ratings parsing as well.
- **Refine Fold rule** to better match the normal swiss rules. In particular, I think there
  should be a separation between FoldUpper and FoldLower, with FoldUpper being higher priority.
  Also the penalty should be fancier than what it is currently to support the right order of trying permutations.  


## FESA rating list

- **Detect the fixed-width column width instead of hardcoding it.** The parser
  ([`crates/core/src/fesa.rs`](crates/core/src/fesa.rs)) assumes the last-name
  column is exactly `LAST_NAME_WIDTH` (18) characters. If FESA widens that column
  — e.g. the day a player with a 19-character last name joins — every row would
  mis-split. Derive the column boundaries from the file itself (e.g. from the
  header row's `Name` / `Grades` positions, or by detecting the alignment) so the
  parser adapts automatically instead of silently breaking.


## Frontend

- **Add a button to load a CSV of player names in the players tab**

- **Export settings as JSON from the Settings tab** A download button that
  serializes the current `TournamentSettings`, to feed the simulation CLI's
  `--configs` (see [`docs/simulation-cli.md`](docs/simulation-cli.md) §3.4).

## Simulations

See [`docs/simulation-cli.md`](docs/simulation-cli.md) for the design.

## Multi-referee setup

See [`docs/multi-referee-internet.md`](docs/multi-referee-internet.md) for the design.

## Other

- **Webhook for pushing results and pulling players** See https://github.com/ffrgo/pairgoth/blob/master/doc/reference.md#pairgoth-webhook-specification