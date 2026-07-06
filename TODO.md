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
- **Ranked standings.** The Results tab lists per-round results and a victory
  count, ordered by tournament number. Add real standings ordering (by score,
  then tie-breaks) and richer scoring (points, draws) on top of it.
- **No-shows.** A player who was paired for a round but did not show up (distinct
  from a game simply not yet recorded) should appear as `0#` in the results
  table. This needs a way to mark a board as a no-show (a new board-result state,
  giving the opponent the win); not handled yet. Byes (`0+`, win) and absences
  (`0-`, loss) are already handled.

## FESA rating list

- **Hardcoded, date-specific URL.** The list is fetched from a single hardcoded
  URL whose path encodes a publication date
  (`https://fesashogi.eu/old/ratinglists/2026-06-01.txt`, in
  [`crates/server/src/ratings.rs`](crates/server/src/ratings.rs)). FESA publishes
  a new dated list periodically, so this should become configurable and/or
  auto-discover the latest available list rather than being pinned to one date.

- **Detect the fixed-width column width instead of hardcoding it.** The parser
  ([`crates/core/src/fesa.rs`](crates/core/src/fesa.rs)) assumes the last-name
  column is exactly `LAST_NAME_WIDTH` (18) characters. If FESA widens that column
  — e.g. the day a player with a 19-character last name joins — every row would
  mis-split. Derive the column boundaries from the file itself (e.g. from the
  header row's `Name` / `Grades` positions, or by detecting the alignment) so the
  parser adapts automatically instead of silently breaking.
