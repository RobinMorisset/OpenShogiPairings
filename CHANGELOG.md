# Changelog

Notable changes to OpenShogiPairings. Format follows
[Keep a Changelog](https://keepachangelog.com/).

This project does NOT adhere to semantic versioning. Assume that even minor
version changes can change the save format and thus break compatibility; it
will be explicitly mentioned in the changelog for that version though.

## [Unreleased]

**Save files from earlier versions no longer load** (the save format version is
now 9): a board records what happened on it as a single value instead of three
separate flags — and, when it was forfeited, why each missing side missed it —
and tournaments can carry teams. A stale file is rejected with a clear message
rather than half-parsed; re-create the tournament.

### Changed

- **Points are no longer accumulated — they are derived**, as
  `MacMahon start + wins`, and a manual bonus/malus now lands *inside* the
  MacMahon start rather than beside it. Two consequences. The Results tab's
  "Wins + MacMahon points" is now a sum that adds up for every row, including
  an adjusted one: the MacMahon column previously showed the raw starting
  points while the total quietly included the adjustment, so an adjusted
  player's row visibly did not reconcile. And an adjustment now behaves as an
  ordinary MacMahon point *everywhere*, including the airtight-groups pairing
  rule — a +1 bonus puts a player in the MacMahon group their adjusted score
  puts them in, where before it moved their total but not their group.
  Structurally, there is no longer a points counter to keep in step with the
  wins counter, which is how the two came to disagree in the first place.

### Fixed

- **A drawn team match now counts as half a win** for each side, not none. It
  already scored half a *point* — the same half the Wins column was dropping,
  so the two columns described one match and disagreed about it, and the
  team tie-breaks summed from wins (SOSW, SODOSW, SOSOSW, CUSSW) never saw the
  draw at all. A draw still defeats nobody, so neither side enters the other's
  *defeated* list and the `…DOS` flavours are unchanged.
- **A half-point sit-out now counts as half a win**, not none. A player on a
  MacMahon start of 1 who lost a game and took a `0=` showed `Wins 0` beside
  `Points 1½` — two columns describing the same round and disagreeing about it,
  with the Points column's own tooltip ("Wins + MacMahon points") claiming a
  relation that did not hold. Wins are now counted in halves like points,
  following the EGF "number of wins" convention, and the tie-breaks summed from
  them (SOSW, SODOSW, SOSOSW, CUSSW, SOSW-1, SOSW-2) inherit the half instead of
  rounding it away. **This can change the final ranking** of a tournament with
  half-point sit-outs, since those tie-breaks now separate players they used to
  tie. Whole wins are unaffected, and the save format is unchanged.

### Added

- **Nationality protection** (Settings → *Nationality protection*), club
  protection's weaker sibling: avoid pairing two players of the same
  nationality, optionally only for the first N rounds and with nationalities
  (e.g. the host country) exempted. Off by default, configured independently of
  club protection, and one rule tier below it — so when only one of the two can
  be honoured, the club clash is the one avoided and the nationality clash is
  accepted, even at the cost of a worse fold. Nationalities are matched
  case-insensitively and a player with none set is never protected; in a team
  tournament it counts the compatriot *games* a match would create, exactly as
  the club rule does. It appears in the round explanation as its own rule.
- **A public read-only page for players and spectators** (see
  [`docs/public-access.md`](docs/public-access.md), phase 1). A tournament can
  be *published* from the new **Public page…** toolbar button — off by default,
  per tournament — which mints an unguessable link (`/t/{id}/public?k=…`) and
  shows it as a QR code, with a *Print QR code* button that lays out one sheet
  for the playing room. Anyone opening it sees the standings, the pairings of
  every round already started, and the cup bracket, updating live, with no
  password and no way to change anything. The round the referee is still
  preparing is *never* shown: it is a separate field on the tournament, so
  dropping it is the whole timing policy, while every result becomes public the
  instant it is recorded. Publishing again issues a fresh link and revokes the
  old one. Read-only is enforced by the server — the public routes are their
  own group with no mutating handler in it — not by hiding buttons, and the
  standings are the referee's own, so the wall display cannot disagree with
  their screen. A room of phones is cheap to serve: the payload is serialized
  once per change and pushed whole down the event stream, so readers never
  refetch. Not offered by the desktop app, whose server is only reachable from
  the laptop itself (that is phase 2). Referees are warned when publishing that
  point-adjustment reasons, written referee-to-referee, are now read by players.
- **The tournament picker no longer lists private tournaments to strangers.**
  On a server with `OSP_ADMIN_PASSWORD` set, a caller without the admin token
  sees only the published tournaments — and is told so, with a sign-in prompt,
  rather than shown a silently short list. A server deliberately run open (every
  local and desktop one) is unchanged.
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
- **Team tournaments, first half** (see
  [`docs/team-tournaments.md`](docs/team-tournaments.md)): a tournament can be
  set to team mode with a team size (2–9, default 3), players grouped into named
  teams with an explicit board order, and an unrated member given a "pairing
  ELO" used only for pairing-time computations and never exported. Finalizing
  validates the rosters loudly — every player in exactly one team, every team
  full, at least two teams — and numbers the teams by descending average rating.
  Team mode and the features it cannot support (the cup, long games, ELO
  pairing, grade-based MacMahon thresholds, the estimated-ELO tie-break) are
  rejected as a conflicting pair, naming both, rather than either being silently
  disabled.
- **Team pairing**: a team round pairs the *teams* — same Swiss/MacMahon rules,
  applied to team points, team averages and the teams already met — and expands
  each match into one ordinary board per position, board k against board k. A
  team bye is one full-point sit-out per member; an absent team is left out of
  the pairing as a whole. Everything above the boards is derived, never stored:
  the match a board belongs to, the match result (more board wins takes the
  point, level splits it), and each team's running score. Club protection now
  counts the same-club *games* a match would create, so a mixed-club team is
  handled without any notion of a "team club". Simulating a team tournament is
  refused explicitly — its metrics are all defined over individual standings.
- **Team standings**, ranked by the same criteria with team semantics: the SOS
  family sums opposing *teams'* scores and direct confrontation counts matches
  won. A new **board wins** tie-break counts games won — a team's total across
  every board, the established second criterion in team events after match
  points, and a player's own game count in an individual tournament. Per-player
  point adjustments are refused in team mode for now, since the ranking is by
  team and a per-player bonus would move nothing visible; team-level adjustments
  are still to come.
- **Team roster endpoints**, so a team tournament can be configured, rostered,
  finalized and played entirely over the API: create/rename/delete a team,
  add/remove a member, set or reset the board order, and set an unrated member's
  pairing ELO. See the route table in the README.
- **Team setup in the interface**: a "Team tournament" toggle and team size in
  Settings, and a Teams tab next to Players — create, rename and delete
  teams, move players in and out, reorder the boards (or reset that order by
  rating), and give an unrated member a pairing ELO. A player joining a team
  takes the board their rating calls for, so a roster built one player at a
  time is already in board order. Each card shows its size
  (`2/3`) and average rating, so an incomplete roster is visible before
  finalizing rather than only in the error afterwards. The panel goes read-only
  once registration is finalized, matching the frozen rosters.
- **Team results in the interface**: the round view groups a round's boards by
  the match they belong to, each under a header naming the two teams and the
  board wins so far, and the Standings tab becomes one table — each team,
  ranked by the configured criteria, followed by its players in board order.
  The team's name straddles the player columns; the matches won, the ranking
  criteria and the float markers belong to the team row, since those are team
  quantities. The round columns are shared: the team's cell names the team it
  met and how the match went, the player's names their own opponent. A round a team sat out is
  re-scored (`0+` / `0=` / `0−`) from the team's own cell, which writes the
  value to every member at once — a team sits out together, and its score for
  that round is read from entries that have to agree.
- **Justified absences.** A forfeited board now records *why* each missing side
  missed it, and the cross-table and American grid say so: `0-` for a player
  absent for a reason, `0#` for one who simply didn't turn up. It exists because
  a team plays whether or not every member appears — a player who falls ill
  still has a board, and stamping it with the unjustified `0#` put the wrong
  thing in the record. Marking part of a team absent before pairing now creates
  those boards already forfeited that way, instead of being refused. In an
  individual tournament an absent player never reaches a board, so the kind
  doesn't arise there and is rejected.
- **Customizing a team round**: the draft's forced pairings and forced byes name
  *teams* in a team tournament, since teams are what get paired — a forced match
  expands to its boards like any other. The "why this pairing?" probe and the
  "force this pairing" action follow, naming teams too. Marking players absent
  stays per player, because a member can be absent without their team being —
  but the list is grouped by team, with a box that marks a whole withdrawn one
  absent in a single step.
- **Team point adjustments**: a manual bonus or penalty, with its mandatory
  reason, applies to a *team* in a team tournament — the ranking is by team, so
  that is the level a delta can move. Unlike the roster controls it stays
  available once registration is finalized, which is when a referee actually
  awards one. Per-player adjustments remain refused in team mode.

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

- **Deleting a tournament no longer deletes its backups**, which made it the one
  action with no way back — and an easy one to take by accident. A final backup,
  labelled *deleted*, is now taken of the exact state it was deleted in (the
  automatic ones are only taken at round transitions, so the newest could be many
  edits old, and a tournament deleted before round 1 had none at all). Its
  backups directory is then kept for a month before being swept — set
  `OSP_BACKUP_RETENTION_DAYS` to change that, or to `0` for the old behaviour.
  Restoring one is by hand for now: import the file from the backups directory,
  whose path the Backups button names. A deleted tournament's password hash is
  kept alongside its backups, so a protected tournament stays protected on the
  way back.

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

### Fixed

- Marking a board as a draw after flagging it a no-show recorded a draw on a
  game nobody played, and fed it to the ELO estimate as a real ½ point. The
  draw button is now disabled on a forfeited board (clear the no-show first).
- The float markers in the standings cross-table were doubled: a player who
  played one point up showed `^^`, two points up `^^^^`. The gap is stored in
  half-points and the markers counted it raw. Only the display was wrong —
  the pairings themselves read the direction, never the count.

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
