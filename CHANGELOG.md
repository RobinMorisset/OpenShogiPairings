# Changelog

Notable changes to OpenShogiPairings. Format follows
[Keep a Changelog](https://keepachangelog.com/).

This project does NOT adhere to semantic versioning. Assume that even minor
version changes can change the save format and thus break compatibility; it
will be explicitly mentioned in the changelog for that version though.

## [Unreleased]

The save format changed again, but a **tournament that has not started yet can
still be opened from a v1.1.0, v1.2.0 or v1.3.0 save**, with its players and all
of its settings carried over. A save whose tournament had rounds played, or a
round in preparation, says so plainly when you try to open it: finish that event
with the version that has it. Saves from v1.0.0 cannot be opened at all.

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
- A **licence check** in the Players tab: give it a CSV list of the players
  whose federation licence is up to date, and it names the registered players of
  a chosen nationality who are missing from it — the ones who forgot to pay
  their yearly fee — flagging near-miss spellings, since these files are typed
  by hand. It only reports; nothing is registered, edited or removed.
- **An application icon of its own** — a shogi piece bearing a pairing list —
  in place of the generic Tauri placeholder, and a favicon for the browser
  client, which had none at all.
- The server status line now says **which build the server is**, not just its
  version number, so a bug report can name it exactly.

### Fixed

- **A tournament created without a password of its own was writable by anyone**
  on a server with `OSP_ADMIN_PASSWORD` set: no password meant no gate, so a
  stranger could add players to it or delete it outright. Such a tournament now
  falls back to the admin token. Local and embedded servers, which have no admin
  password, are unchanged.
- **The API answered cross-origin requests from any website.** It now names the
  origins the app is actually served from, so a page on an unrelated site can no
  longer read the API's replies.
- **A long game was scored wrong in three ways.** Its two points arrived when its
  first round finished, vanished when the next one started, and returned only
  once that round completed — so the standings and the public page ranked the
  field wrongly in between. One decided early, most often by a no-show, also
  freed both players for the next round, letting a winner end two rounds with
  three wins. And one nobody turned up for still counted as a game played,
  skewing every tie-break that sums opponents (SOS, SODOS, Buchholz) and the
  pairing of later rounds. A long game now takes both of its rounds however it
  ends, and scores once, when the round it finishes in is complete.
- A player registered mid-tournament was not affected by the "half point for
  an absence" setting.
- The American Grid export could be used even when some games had not been
  completed, and sometimes emitted a wrong grid in that case.
- It was possible to declare a sennichite, or to set a handicap, in a game
  that did not occur because of a no-show.
- **Discarding the draft of round 1 left registration closed for good**, with no
  draft and no round either — a state nothing but undo could get out of.
  Cancelling that first draft now reopens registration, exactly as cancelling
  round 1 itself already did.
- **Removing a player who never played could break the tournament**: their
  sit-outs stayed behind, pointing at a number nobody held any more, which could
  make the standings fail outright and an open round draft refuse to be
  confirmed. They are now removed from the rounds and the draft along with
  everything else.
- **Loading a CSV roster twice registered every player twice**, leaving a list of
  homonyms to delete one by one. A file now registers only the players it names
  who are not registered yet, matched on name ignoring case and accents; the ones
  skipped are named above the player list.
- **A round with no game to play could not be finished**, and so blocked every
  later round: a round where every player is byed, absent or still on a long
  board has no board whose result could complete it.
- **A round finishing a long game could not be re-paired**, so none of that
  round's other pairings could be changed by hand; and after a long game, the
  buttons that start the next round and export the cross-table stayed disabled
  for good. A carried long game is now the one result that survives a re-pairing,
  handicap included; everything else still refuses.
- **Refusing to make a round long now says why**, instead of borrowing the
  "can't re-pair a round with results" message, and the checkbox no longer keeps
  the tick the referee just made under a banner saying it was refused. In a cup
  the refusal now covers the whole round: ticking an unplayed board used to
  retroactively double a result already recorded on another.
- **A player finishing a long game can no longer be marked absent.** Marking them
  gave them an absence *and* the game, which scored them twice and replaced the
  game with a `0-` in the cross-table export.
- Half points from a `0=` influenced Points and tie-breaks like SOS, SODOS, but
  they were not included in Wins, SOSW, SODOSW, etc.
- Manual points adjustments now interact with airtight groups, since they
  are MacMahon points like any other.
- The float markers in the standings table were doubled: a player who
  played one point up showed `^^`, two points up `^^^^`.
- **An internal server error used to hang up instead of answering**, which a
  client cannot tell from the network failing — and every later edit then came
  back as "another referee changed the tournament first" until the page was
  reloaded. The error is now answered and named, and the app reloads the
  tournament rather than assuming the edit was rejected.
- **A damaged save file could bring the server down**, taking every other
  tournament with it, or quietly produce standings for a tournament nobody
  played. Saves are now checked when they are loaded, exactly as imported ones
  are, and a file that fails appears in the picker with the reason it could not
  be opened, untouched. A file with no format version at all, or carrying a key
  this version does not recognise, is now refused rather than read anyway.
- Some filesystem failures were silently swallowed; they are now surfaced as
  errors in the log. In particular, an unreadable backup directory was reported
  as "no backups yet", and an unreadable data directory (an unmounted volume,
  say) as a server with no tournaments on it.
- The desktop app now respects `OSP_DATA_DIR` instead of putting its data in a
  hardcoded location.
- **Switching away from the window and back cleared a pairing explanation** — the
  two players you had picked, and the answer you were reading. An explanation now
  survives anything that leaves the pairing alone, and is cleared only when the
  round really is paired differently.
- **A CSV import quietly replaced values it could not read.** An ELO written
  `2 100` or `2,100` — routine spreadsheet output — counted as no ELO at all, and
  the player was then given the FESA list's rating instead, with nothing to show
  for it. The import now refuses and names the rows, and does the same for an
  unreadable grade, a row that doesn't line up with the header, a quoted cell
  that is never closed (which used to swallow every row below it, so a roster of
  forty imported as one player), and a name that matches two players in the FESA
  list.
- **Rows the FESA rating list parser could not read simply disappeared**, so a
  change in the file's format would have shown the referee a list with players
  missing — and no way to notice. Such a list is now refused with the reason.
- **Turning off team mode or MacMahon starting points silently erased every
  hand-entered pairing ELO**, and turning them back on did not restore the
  values. The change is now refused until those are cleared deliberately.
- **A server that cannot write to disk now says so** in a banner. Edits used to
  be accepted and reported as saved while living only in memory, so a full disk
  cost the whole tournament at the next restart with no warning anywhere.
- Exporting the settings to a file reported nothing when the write failed.

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
  round rather than shuffling under the referee mid-round, with a note above it
  naming the rounds not counted yet.
- **A reorganized Settings tab**, with useless sections hidden, settings grouped
  logically, and better handling of very wide windows.
- **Much better handling of narrow windows (e.g. phones)**, especially in the
  Standings tab which was barely readable before.
- **Tidying of the UI in general**: better tooltips, the Tab key now correctly
  moves focus to the next field in the Settings tab, useless and redundant
  elements are gone, columns that were misaligned are now properly aligned,
  etc.
- **Better explanations of pairings**, with much more detail. And if the
  explanation would be inaccurate (because the referee corrected an earlier
  round's result, edited a player, or changed a setting since that round was
  paired), it avoids giving wrong information.
- **Players are typed, not scrolled for**: the round draft's forced pairings and
  forced bye, and the "Why these pairings?" panel, take part of a name or a
  tournament number in an autocomplete field, and accept only someone the round
  can actually pair.
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
