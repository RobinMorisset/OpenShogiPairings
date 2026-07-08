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