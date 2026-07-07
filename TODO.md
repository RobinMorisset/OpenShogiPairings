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
- **Experimental ELO-based, non-swiss system** When that mode is active, all MacMahon and swiss option should
  be disabled/greyed out. In it, OSP keeps track of an estimated ELO for each player throughout the tournament based on
  Bayesian reasoning; and the constraints related to victories, floaters, fold, etc.. (everything but rematch and club)
  get replaced by a constraint minimizing the square of ELO differences per game (sitting between rematch and club).


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

## Simulations

- **Add an API to get random game results** This API should not be surfaced in the UI, it is
  to be used in tests, and for doing simulations of various pairing methods. For each undecided match
  in the current round, it should pick a winner based on the respective ELOs, using the following formula:
  Chance of victory for player with elo A playing against player with elo B: 1 / (1 + 10^((B - A) / 400))
- **Add an API getting statistical results from a tournament** the one I'm interested in is specifically the distribution of
  ELO differences between players of games, ideally taking not the ELO at the start of the tournament for each player, but allowing an updated player -> ELO mapping to be provided by the client for this request only.

## Other

- **Alternative scoring rules** See from PairGoth reference doc: SOSMM1, SOSMM2, CUSSM, the various non-mac-mahon versions.
  https://github.com/ffrgo/pairgoth/blob/master/doc/reference.md
  Which one is used and in which order should be configurable (and only those should appear in the results tab.

- **Allow storing default properties** which apply to all new started tournament.
  I'm not sure it is worthwhile, just a savec tournament with no players and just the properties set is equivalent.

- **Add authentication to the distributed instance** So that only referees with the right password can access it.

- **Automatic backups** This is just a save of the tournament file, but done on the server side after every step transition
  of the round state machine (coarser than the undo stack). Should be parameterized how many are kept (rotating), default to 10 for now.
  There should be a way to load a backup tournament.

- **Webhook for pushing results and pulling players** See https://github.com/ffrgo/pairgoth/blob/master/doc/reference.md#pairgoth-webhook-specification

- **Add some pairing explanation mechanism**