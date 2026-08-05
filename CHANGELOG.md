# Changelog

Notable changes to OpenShogiPairings. Format follows
[Keep a Changelog](https://keepachangelog.com/).

This project does NOT adhere to semantic versioning. Assume that even minor
version changes can change the save format and thus break compatibility; it
will be explicitly mentioned in the changelog for that version though.

## [1.3.0] - 2026-08-05

Backwards compatible with v1.1.0 and v1.2.0

### Added

- **Translations** of the UI in German, Japanese, Polish, Slovak, Russian,
  Belarusian and Ukrainian, bringing it to nine languages alongside English
  and French.

### Fixed

- Some error messages were untranslated and always in English.
- Some buttons in the UI remained active (and would do nothing) when they
  should have been greyed out.
- Loading an invalid tournament save file created an empty tournament.
- Loading an invalid set of settings was silently ignored, it is now clearly
  rejected.

### Changed

- Some performance optimizations to the core matching algorithm; pairing one
  round of a 1000-player tournament is now reliably between 20ms and a few
  hundred milliseconds, depending on which pairing rules are in use.

## [1.2.0] - 2026-07-18

Backwards compatible with v1.1.0

### Added

- **Customizable player categories**, such as Women or U18, with a way to
  highlight these players in the Standings tab.
- **Import of tournament settings.** The Settings tab already had an export
  button; it now has a matching import button too.

### Fixed

- Throughout the app, buttons would often dim for a fraction of a second
  whenever another button was clicked.
- In the Players tab, editing a cell shifted the column widths, making it hard
  to reliably click another cell.
- In the Round tab, in alphabetical mode, the column headers were misaligned
  with their contents.
- In the Round tab, clicking a player to mark them a winner would in some cases
  make the "Why these pairings?" section disappear for a split second, causing
  the layout to flicker.
- On Windows, the select elements in the hidden panel at the bottom of the
  Round tab kept a white background even in dark mode.
- In the Settings tab, the tie-breaker picker overflowed its column.
- In print mode, various unnecessary interface elements are now hidden.
- In print mode, everything is now black-and-white (checkmarks and medal emojis
  were previously left in color).

## [1.1.0] - 2026-07-17

NOT backwards compatible with v1.0.0.

### Added

- **Support for `0=`** throughout: absences can now be worth 1/2 point. This is
  off by default, but can be turned on by an option in the settings.
- **Fine-grained absence tweaks.** Referees can switch between `0+`, `0=` and
  `0-` on individual rounds and players.
- **Smarter ELO estimation.** For the ELO estimate (used by both the
  experimental pure-ELO pairing mode and estimate-based MacMahon, off by
  default), unrated players can be given a Laplace or flat prior, instead of
  the usual Gaussian. Yet more options and parameters are available from the
  command line, but are hidden in the UI.
- **MacMahon from estimated ELO.** The ideal option when you have very strong
  unrated players in your tournament — it grants them MM points as they
  demonstrate their strength over the tournament. Off by default.
- **Two-round "long games"**: flag a board to span two rounds (double time
  control); its players sit out the intervening round and the winner scores two
  points. Works in the Swiss and cup phases.
- **Cup bracket view**: a Cup tab showing the direct-elimination bracket and
  podium.
- **Alphabetical player lookup** in the round view.
- **Tooling for large-scale studies.** The simulator can now be more easily
  used to evaluate the effects of specific pairing policies across thousands of
  runs over thousands of tournaments, with careful statistical evaluations of
  the effect on several metrics; see `scripts/mm_study.py` for details.

### Changed

- **Tournament file format bumped to v5.** Files saved by v1.0.0 no longer load
  and are rejected with a clear version error rather than mis-parsed — the
  settings object was restructured (pairing is now a Swiss/ELO sum type; club
  protection and the Wiel handicap rule fold into their own enums).
- **Cup games can now count as floaters.** If some cup players have more MM
  points than their opponents, they will now correctly count as descending
  floaters, and their opponents as ascending floaters.
- **Many optimizations to the core pairing system**, netting roughly 4x in
  performance. Imperceptible in normal use (pairing 100 players was roughly
  4.5ms even before this), but very useful in simulation mode when doing 1000
  runs of each of the 3000+ tournaments on the FESA website.

### Fixed

- Pairing a round with thousands of players could trigger a silent integer
  overflow.
- When several referees changed the same tournament on the same server
  concurrently, their edits could race.
- The simulator applied the wrong amount of jitter to players that started a
  tournament as unrated and finished it as provisional.

### Internal

- **Refactorings to improve type safety.** TournamentId, Wins and HalfPoints
  instead of u32 everywhere.
- **Refactorings to improve type safety (2).** Settings object has been
  restructured to make illegal combinations of options unrepresentable.
- **CI workflow** to catch test regressions and similar issues.

## [1.0.0] - 2026-07-13

Initial release
