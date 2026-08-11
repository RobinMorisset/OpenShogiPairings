# Changelog

Notable changes to OpenShogiPairings. Format follows
[Keep a Changelog](https://keepachangelog.com/).

This project does NOT adhere to semantic versioning. Assume that even minor
version changes can change the save format and thus break compatibility; it
will be explicitly mentioned in the changelog for that version though.

## [Unreleased]

Save files from earlier versions load, **except** one saved mid-tournament with
the hybrid cup enabled: the cup now records which format it was seeded under, and
that field has no default. Re-create such a tournament rather than loading it.

### Added

- **The tournament's city, country, dates and time control** (Settings →
  *Place, dates and time control*), reported in the header of the American Grid
  export the way the
  [FESA tournament-system guide](https://fesashogi.eu/tournament-system-user-guide/)
  asks: `[name, city, country, first to last]`, followed by the tournament's
  last day on its own line and by `[Time control: …]`. Each part is written only
  when it was entered, so an export from a tournament that sets none of them is
  unchanged. A live preview under the fields shows the header as it will be
  sent. The dates are entered as a pair (a one-day event repeats the day) and
  validated — an impossible date or a range ending before it starts is rejected
  rather than exported.

- **Hybrid cup: a qualification-round format** (Settings → Hybrid cup), used by
  the German Championship. The top half of the bracket is pre-qualified and
  plays an ordinary Swiss game in round 1, while the next players play a
  qualification round among themselves; its winners complete the bracket, which
  then runs from round 2. It takes half as many eligible players again as the
  bracket holds — 12/24/48/96 for a bracket of 8/16/32/64 — and one more round.
  The pre-qualified are never paired with each other in that round — a new
  pairing rule that exists only there. The existing behaviour is the "direct
  bracket" option, unchanged and still the default.
- `osp-sim --cup-format direct|qualifier`, so simulations can run either cup
  format (see [`docs/simulation-cli.md`](docs/simulation-cli.md)).
- **Configurable data and backup directories.** `OSP_DATA_DIR` (tournaments)
  and the new `OSP_BACKUP_DIR` (automatic backups) now apply to the **desktop
  app** as well as the standalone server, which previously hardcoded both under
  the per-user data directory — so the files can live on a synced folder, a
  second drive, or beside a portable install. Unset keeps exactly the old
  locations. Both are logged at startup.
- **The Backups button says where the backups are kept** — the absolute
  directory, in its tooltip and at the top of the panel (selectable there, to
  paste into a file manager). A rotating store of recovery copies is only as
  useful as the referee's ability to find the files; it also makes the new
  `OSP_BACKUP_DIR` visibly take effect. `GET /backups` grew the directory
  alongside the list to carry it.

### Changed

- **The desktop app keeps a log file**, where it previously logged only in
  development builds — so a packaged app had no way to report that it could not
  save. It lives beside the app's other data (`~/Library/Logs/…` on macOS,
  `%APPDATA%\org.openshogipairings.desktop\logs\` on Windows) as a single file
  the app itself caps at 40 KB, and it stays near-empty: the release build logs
  only warnings and errors, plus the startup lines saying which data and backup
  directories are in use. Nothing is sent anywhere — it is a file to look at (or
  attach to a bug report) when something has gone wrong.
- **A filesystem failure the server used to ignore now says so.** Rotating an
  old backup, deleting a tournament's files, caching the FESA rating list and
  creating the data directory all discarded their errors, so a full disk or a
  read-only directory produced silence and a job quietly not done. Each is still
  best-effort — none of them interrupts what the referee is doing — but each now
  leaves a line in the log. A file that was already absent when deleting it
  stays silent, since that is the outcome that was wanted.
- **The Results tab shows the round in progress**, instead of waiting for it to
  be completed. Its column (marked `R3*`) appears as soon as the round is
  paired, with each board's cell filling in the moment its result is recorded;
  a game still being played shows `5?` rather than a win or a loss. The wins,
  points and tie-break columns — and the ranking itself — still count completed
  rounds only, so the table re-sorts in one step at the end of the round rather
  than shuffling under the referee mid-round.
- **A cross-table cell's tooltip names the opponent with their rating** —
  `Doe Jane (1800)` — where it used to read `vs Doe Jane`. The "vs" said
  nothing the cell didn't already, and the rating answers what the hover is
  usually for: how strong was that opponent. An unrated opponent keeps the bare
  name.

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
