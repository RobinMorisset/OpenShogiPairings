# Changelog

Notable changes to OpenShogiPairings. Format follows
[Keep a Changelog](https://keepachangelog.com/).

This project does NOT adhere to semantic versioning. Assume that even minor
version changes can change the save format and thus break compatibility; it
will be explicitly mentioned in the changelog for that version though.

## [Unreleased]

**Save files from earlier versions no longer load** (the save format version is
now 8): a board records what happened on it as a single value instead of three
separate flags — and, when it was forfeited, why each missing side missed it —
and tournaments can carry teams. A stale file is rejected with a clear message
rather than half-parsed; re-create the tournament.

### Added

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
  Settings, and a Teams panel on the Players tab — create, rename and delete
  teams, move players in and out, reorder the boards (or reset that order by
  rating), and give an unrated member a pairing ELO. Each card shows its size
  (`2/3`) and average rating, so an incomplete roster is visible before
  finalizing rather than only in the error afterwards. The panel goes read-only
  once registration is finalized, matching the frozen rosters.
- **Team results in the interface**: the round view groups a round's boards by
  the match they belong to, each under a header naming the two teams and the
  board wins so far, and the Standings tab gains a team table above the
  per-player one — ranked by the configured criteria, with each team row
  expanding to its players in board order and the games each of them won.
- **Justified absences.** A forfeited board now records *why* each missing side
  missed it, and the cross-table and American grid say so: `0-` for a player
  absent for a reason, `0#` for one who simply didn't turn up. It exists because
  a team plays whether or not every member appears — a player who falls ill
  still has a board, and stamping it with the unjustified `0#` put the wrong
  thing in the record. Marking part of a team absent before pairing now creates
  those boards already forfeited that way, instead of being refused. In an
  individual tournament an absent player never reaches a board, so the kind
  doesn't arise there and is rejected.

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

### Fixed

- Marking a board as a draw after flagging it a no-show recorded a draw on a
  game nobody played, and fed it to the ELO estimate as a real ½ point. The
  draw button is now disabled on a forfeited board (clear the no-show first).

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
