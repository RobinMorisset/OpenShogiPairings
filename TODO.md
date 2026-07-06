# TODO

Known limitations and future work, roughly ordered by area.

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
