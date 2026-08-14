# Changelog

Notable changes to OpenShogiPairings. Format follows
[Keep a Changelog](https://keepachangelog.com/).

This project does NOT adhere to semantic versioning. Assume that even minor
version changes can change the save format and thus break compatibility; it
will be explicitly mentioned in the changelog for that version though.

## [Unreleased]

The save format changed again, but a **tournament that has not started yet can
still be opened from v1.1.0, v1.2.0 or v1.3.0** — those three share one save
format, and its players and all of its settings are carried over, the file being
saved in the new format from then on. A save from any of them whose tournament
had rounds played, or a round in preparation, says so plainly when you try to
open it: finish that event with the version that has it. Saves from v1.0.0
cannot be opened at all.

### Added

- **Team tournaments** are now supported, following the same format as the
  one that precedes every WOSC.
- **Hybrid cups of the German type**, with a number of pre-qualified players
  facing (usually) foreigners in the first round, while the rest of the
  eligible players play a qualifier round, with the actual cup starting in the
  second round.
- An optional **public read-only page for players and spectators**, only
  usable in server + client mode (not from the desktop app), and only
  if the referee chooses to enable it from the new "Public page" toolbar
  button.
- Alternatively, the referee can choose to **export the public page** as a set
  of plain HTML pages to be put on any website they may have. Unlike the
  previous feature, this one works even from the desktop app.
- **Printable per-player result sheets**, accessible from a button in each
  round tab.
- Optional **nationality protection**, a weaker variant of club protection.
- **An application icon of its own** — a shogi piece bearing a pairing list —
  in place of the generic Tauri placeholder, and a matching favicon for the
  browser client, which had none at all.

### Fixed

- Half points from a `0=` influenced Points and tie-breaks like SOS, SODOS, but
  they were not included in Wins, SOSW, SODOSW, etc.
- "Why these pairings?" now warns if unreliable; this can occur if a referee
  changed the results of an earlier round, or the settings of the tournament,
  or did a manual adjustment to MacMahon points.
- Manual points adjustments now interact with airtight groups, since they
  are MacMahon points like any other.
- It was possible to declare a sennichite in a game that did not occur because
  of a no-show.
- The float markers in the standings table were doubled: a player who
  played one point up showed `^^`, two points up `^^^^`.
- **A tournament created without a password of its own was writable by
  anyone** on a server with `OSP_ADMIN_PASSWORD` set, once it was published:
  publishing lists its id to unauthenticated callers, and having no password
  meant having no gate, so a stranger could add players to it, unpublish it or
  delete it outright. Such a tournament now falls back to the admin token.
  Local and embedded servers, which have no admin password, are unchanged, and
  the public reader page still needs only its capability key.
- **The API answered cross-origin requests from any website.** It now names the
  origins the app is actually served from — the desktop webview and the dev
  server — so a page on an unrelated site can no longer read the API's replies.
  This matters most on a server with no admin password, where every route is
  open by design and `osp-server` listens on the well-known `127.0.0.1:3000`.
- **Removing a player who never played could break the tournament.** Someone
  who says they will miss the first round and then never comes should be
  removable, and mostly was — but their sit-outs stayed behind, pointing at a
  number nobody held any more. If they were the last player registered, the
  standings then failed outright; and an open round draft kept them too, so
  confirming it failed the same way. They are now removed from the rounds and
  the draft along with everything else, leaving no row in the standings and no
  line in the american grid.
- **A round with no game to play could not be finished.** A round where every
  player is byed, absent, or still on a long board carried over from the
  previous round has no board at all, and was nonetheless stamped as being in
  progress. Since a round only finishes when a board result is recorded, and
  there was no board to record, it stayed unfinished and no later round could
  be started.
- **A damaged save of a cup tournament brought the server down instead of being
  refused.** The bracket is rebuilt from the frozen seeding on every read, and
  that rebuild trusted the bracket size and the seed list it was given — so a
  file whose two no longer agreed (a truncated write, a hand edit) crashed the
  moment anything looked at that tournament, taking the others with it. Such a
  file is now checked when it is loaded, exactly as an imported one is, and
  appears in the picker with the reason it could not be opened; the file itself
  is left untouched.
- **The rest of a damaged save could still bring the server down, or quietly
  report the wrong tournament.** The same check now also refuses a file whose
  players share a registration id or a tournament number, whose players have no
  tournament number although registration is finalized, whose rounds are not
  numbered in order, or whose boards and sit-outs name somebody the file does
  not contain — the first three of which crashed the server on the first look at
  that tournament, and the rest of which produced standings for a tournament
  nobody played. A key this version does not recognise anywhere in a save is now
  an error too, rather than being skipped over: a renamed `outcome` used to load
  as a board with no result, silently erasing it.
- A file with no format version at all was assumed to be in the current format
  and read anyway. It is now refused, since that field is the only thing that
  says what shape the rest of the file is in.
- A backup directory that could not be read was reported as "no backups yet",
  and a data directory that could not be read (an unmounted volume, say) as a
  server with no tournaments on it. Both now say what actually happened.
- **The public page and its exported copy showed different things once a cup was
  drawn but round 1 had not started.** The bracket is frozen when registration is
  finalized, and in the gap before the first round the live page showed the
  bracket and no entrant list, while the exported pages showed the entrant list
  and no bracket — so a player looking at their phone in the room and one
  looking at the club's website saw two different tournaments, neither of them
  complete. Both now show both, and which sections a public page has is decided
  in one place for the two of them.
- The simulator's `--cup-final` reconstructed twice the bracket it should from a
  hybrid cup of the German type. Walking backward from the final, the round the
  pre-qualified players spend in the open looked exactly like a bracket round, so
  their opponents were taken for cup players. A game a player *lost* is no longer
  read as one they won their way through, which gives that round away — unless
  every pre-qualified player happens to have won it, which no reading of the
  results alone can tell from a bracket of twice the size.
- **A team could end up playing a match one board short.** Removing a player
  who had not played took them out of their team as well, and a roster one
  member short simply played that many fewer boards — so a three-board match
  could be decided 2–0 over two boards and reported as an ordinary result. In a
  team tournament the team is what gets paired, so removals now work at that
  level: after registration a player can no longer be removed on their own, and
  a team that has never played a match can be removed instead — before
  registration closes its players go back to the unassigned pool, after it they
  leave with the team, which is how a no-show team is dropped.
- Some filesystem failures were silently swallowed; they are now surfaced
  as errors in the log.
- The desktop app now respects `OSP_DATA_DIR` instead of putting its data in a
  hardcoded location.

### Changed

- **Better backups**: the directory is configurable by `OSP_BACKUP_DIR`, shown
  in a tooltip, and backups are preserved for `OSP_BACKUP_RETENTION_DAYS`
  (default: 30) even after a tournament is deleted, and can be restored from the
  tournament picker UI.
- **The Standings tab shows the round in progress**, instead of waiting for it
  to be completed. Its column (marked `R3*`) appears as soon as the round is
  paired, with each board's cell filling in the moment its result is recorded;
  a game still being played shows `5?` rather than a win or a loss. The wins,
  points and tie-break columns — and the ranking itself — still count
  completed rounds only, so the table re-sorts in one step at the end of the
  round rather than shuffling under the referee mid-round. A note above the
  table names every round that isn't in the ranking yet: normally just the one
  being played, but also an earlier one whose result a referee cleared to
  correct it.
- **A reorganized Settings tab**, with useless sections hidden, settings grouped
  logically, and better handling of very wide windows.
- **Much better handling of narrow windows (e.g. phones)**, especially in the
  Standings tab which was barely readable before.
- **Tidying of the UI in general**: better tooltips, the Tab key now correctly
  moves focus to the next field in the Settings tab, useless and redundant
  elements are gone, columns that were misaligned are now properly aligned,
  etc.
- **A cross-table cell's tooltip names the opponent with their rating** —
  `Doe Jane (1800)` — where it used to read `vs Doe Jane`. The "vs" said
  nothing the cell didn't already, and the rating answers what the hover is
  usually for: how strong was that opponent. An unrated opponent keeps the bare
  name.
- If `OSP_ADMIN_PASSWORD` is set, only published tournaments are visible to
  anyone without that password.

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
