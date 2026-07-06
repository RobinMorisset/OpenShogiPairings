# TODO

Known limitations and future work, roughly ordered by area.

## Pairing

- **ILP/CP-SAT backend.** Pairing is now a real **minimum-weight perfect
  matching** (integer blossom in [`crates/core/src/matching.rs`](crates/core/src/matching.rs))
  over a rule-weighted graph ([`crates/core/src/pairing.rs`](crates/core/src/pairing.rs)):
  rematch/repeat-bye ≫ score gap² ≫ float-repeat ≫ same-club ≫ within-group fold,
  ordered by a scalar multiplier ladder. Two limits motivate an ILP/CP-SAT
  backend: (a) the multiplier ladder only approximates strict lexicographic
  priority and its gaps are sized for realistic events (≲128 players, ≲20 rounds);
  (b) experimental formats (MacMahon-beyond, hard multi-round constraints) need
  true lexicographic tiers and constraints a plain matching can't express. Plan:
  `good_lp` + HiGHS first, then CP-SAT.
- **Tune / make rules configurable.** The rule weights and the float-decay base
  are compile-time constants; expose them (and let referees toggle/reorder rules)
  once the model settles. Colour balance is intentionally absent (sente/gote is
  random per game in shogi).
- **Ranked standings.** The Results tab lists per-round results, a victory count
  and a total-points column (victories + MacMahon starting points), ordered by
  tournament number. Add real standings *ordering* (by points, then tie-breaks)
  on top of it.
- **No-shows.** A player who was paired for a round but did not show up (distinct
  from a game simply not yet recorded) should appear as `0#` in the results
  table. This needs a way to mark a board as a no-show (a new board-result state,
  giving the opponent the win); not handled yet. Byes (`0+`, win) and absences
  (`0-`, loss) are already handled.
- **End-of-tournament ELO update.** Boards now record the *actual* result plus a
  `drawn` flag and an optional `handicap` (with a frozen giver), precisely so an
  ELO recompute can consume them — but no such computation exists yet. The
  standings/pairing already use the *effective* winner (`Board::effective_winner`:
  the handicap giver always scores, whoever actually won); ELO should instead use
  the actual result and the draw/handicap detail. In the Results tab a handicap
  game shows the actual sign with the handicap in parentheses (e.g. `3=−(−4p)`
  giving, `1=+(+4p)` receiving) while the victories column counts the effective
  winner.

- **Making club protection optional** as a tournament option. It should also be possible
  to have it only active for the first N rounds of the tournament (also settable from the
  tournament settings tab).

- **Add proper selection of floaters**: classic swiss vs median swiss as an option 

## FESA rating list

- **Detect the fixed-width column width instead of hardcoding it.** The parser
  ([`crates/core/src/fesa.rs`](crates/core/src/fesa.rs)) assumes the last-name
  column is exactly `LAST_NAME_WIDTH` (18) characters. If FESA widens that column
  — e.g. the day a player with a 19-character last name joins — every row would
  mis-split. Derive the column boundaries from the file itself (e.g. from the
  header row's `Name` / `Grades` positions, or by detecting the alignment) so the
  parser adapts automatically instead of silently breaking.

## UI

- **Markers for ascending/descending floaters** When a player plays against a player
  with more (respectively less points), they should get a ^ (respectively v) at the end
  of their result cell for that match. In the exceptional case of a game with a difference
  in points greater than 1, it can be expressed by emitting multiple v or ^ in a row.
- **Correct focus in the player registration form** When adding a player to the tournament,
  focus is currently lost. It should go back to the last name field.
  
## Simulations

- **Add an API and a button to cancel the last round** This should return to the state right after the last round completion
  (right after player registration if the last round is the first one)
  and make it easy to replay a round many times in simulation. It can have a button rather than just an API because it can also be useful to referees in rare situations
- **Add an API to get random game results** This API should not be surfaced in the UI, it is
  to be used in tests, and for doing simulations of various pairing methods. For each undecided match
  in the current round, it should pick a winner based on the respective ELOs, using the following formula:
  Chance of victory for player with elo A playing against player with elo B: 1 / (1 + 10^((B - A) / 400))
- **Add an API getting statistical results from a tournament** the one I'm interested in is specifically the distribution of
  ELO differences between players of games, ideally taking not the ELO at the start of the tournament for each player, but allowing an updated player -> ELO mapping to be provided by the client for this request only.