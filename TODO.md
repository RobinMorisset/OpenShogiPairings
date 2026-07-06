# TODO

Known limitations and future work, roughly ordered by area.

## Pairing

- **Replace the naïve pairing with real weighted matching.** The current pairing
  ([`crates/core/src/pairing.rs`](crates/core/src/pairing.rs)) is the most naïve
  mode: all edges weight 1, so it just pairs players consecutively with a bye for
  the odd one out. Implement the intended **minimum-weight perfect matching**
  (Blossom algorithm) with a real weight function (score difference, rematch
  avoidance, colour balance, float rules…), then an ILP/CP-SAT backend for
  experimental formats. `pair_round`'s signature is meant to stay stable while
  its internals are swapped.
- **Standings.** Populate the Results tab from the recorded board results.

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
