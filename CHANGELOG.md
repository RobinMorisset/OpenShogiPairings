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

### Fixed

- **A tournament created without a password of its own was writable by anyone**
  on a server with `OSP_ADMIN_PASSWORD` set: no password meant no gate, so a
  stranger could add players to it or delete it outright. Such a tournament now
  falls back to the admin token. Local and embedded servers, which have no admin
  password, are unchanged.
- **The API answered cross-origin requests from any website.** It now names the
  origins the app is actually served from, so a page on an unrelated site can no
  longer read the API's replies.
- **Discarding the draft of round 1 left registration closed for good**, with no
  draft and no round either — a state nothing but undo could get out of.
  Cancelling that first draft now reopens registration, exactly as cancelling
  round 1 itself already did.
- **Removing a player who never played could break the tournament**: their
  sit-outs stayed behind, pointing at a number nobody held any more, which could
  make the standings fail outright and an open round draft refuse to be
  confirmed. They are now removed from the rounds and the draft along with
  everything else.
- **A round with no game to play could not be finished**, and so blocked every
  later round: a round where every player is byed, absent or still on a long
  board has no board whose result could complete it.
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
  this version does not recognise, is now refused rather than read anyway. So is
  one whose rounds leave a player out altogether: a player needs exactly one
  board or one sit-out in every round they are in, and none at all quietly drops
  a round out of their record.
- Some filesystem failures were silently swallowed; they are now surfaced as
  errors in the log. In particular, an unreadable backup directory was reported
  as "no backups yet", and an unreadable data directory (an unmounted volume,
  say) as a server with no tournaments on it.
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
- **A cross-table cell's tooltip names the opponent with their rating** —
  `Doe Jane (1800)` — where it used to read `vs Doe Jane`. The "vs" said
  nothing the cell didn't already, and the rating answers what the hover is
  usually for: how strong was that opponent. An unrated opponent keeps the bare
  name.
- If `OSP_ADMIN_PASSWORD` is set, only published tournaments are visible to
  anyone without that password.
- **A long game's points disappeared from the standings for a whole round.** A
  long board decided early — most often because one player never turned up —
  scored its two points as soon as its first round finished, then lost them again
  the moment the next round was started, getting them back only once that round
  was complete. In the meantime the standings, and the public page, showed the
  wrong ranking. A long game's points now simply arrive once, when the round it
  finishes in is completed, like every other result of that round.
- **A long game nobody turned up for was still counted as a game played.** For
  tie-breaks a no-show is not a game: neither player is recorded as having faced
  the other. That held in the round the game finished in but not in the round it
  started, so the pair were counted as having met once — where the rule says
  either nought or, for a long game played out, twice. Every published tie-break
  that sums opponents (SOS, SODOS, Buchholz) was affected, as was the pairing of
  later rounds.
- **A long game that never got its second round is now refused by the grid
  export.** Flagging a board in the final round as a long game leaves it having
  taken one round rather than two, so what it is worth is genuinely undecided —
  and it was quietly being scored as nothing while still appearing in the exported
  grid. The export now says so and names the two ways out: start the next round,
  which the game carries into, or untick the box to score it as one ordinary game.
- **A player registered mid-tournament was scored as if they had not missed
  anything, which cost them points where half-point absences are on.** Someone
  added after a round has been played has now simply missed it, exactly like a
  player who was registered from the start and marked absent — the rounds before
  they arrived appear as absences and are worth what the tournament's
  "half point for an absence" setting says. They used to have no record of those
  rounds at all, so they scored nothing for them while an absentee scored half a
  point each, which pushed them below players they were level with and paired
  them against the bottom of the field for the rest of the event — the opposite of
  what that setting is for. With the setting off, nothing changes. One
  consequence to know about: the next round's draft now offers them ticked as
  absent, like anyone else who missed the previous round, so untick them.
- **The buttons that start a round and export the cross-table now ask the
  server whether they should be available, instead of working it out
  themselves.** Both had re-implemented the rule in the browser, and both had
  drifted from it: after a long game they stayed disabled for good, with the
  server willing all along. They now show the server's own reason, translated.
- **Refusing to make a round long now says why, and the checkbox stops
  pretending it worked.** The refusal borrowed the "can't re-pair a round with
  results" message, which described something else entirely; and because the
  refusal leaves nothing to redraw, the box kept the tick the referee had just
  made — showing the change as applied, under a banner saying it wasn't.
- **Making a cup round long is refused once any of its games has a result.** The
  flag covers the whole cup round at once, but the check only looked at the board
  clicked — so ticking an unplayed board retroactively doubled a result already
  recorded on another.
- **A qualifier cup's first round now goes long as one session.** The
  pre-qualified players' games in the open take the same length as the
  qualification boards, instead of staying short and freeing them a round ahead
  of the bracket they are about to join.
- **A player finishing a long game can no longer be marked absent.** They are
  playing, so the draft no longer offers them: marking them anyway gave them an
  absence *and* the game, which scored them twice and replaced the game with a
  `0-` in the cross-table export.
- **A long game is now a board of both rounds it is played over.** It used to
  live only in the round it started, which meant the round it was actually being
  played in did not know about it: that round could close with the game
  unfinished, the cross-table had to reach back a round to find the result, and
  the pairing had to scan every round to work out who was still busy. It now
  keeps one record per round — a placeholder in the round it began, and the live
  game in the round it is finished in, where the result is entered. The round it
  is played in stays open until that result is in, exactly like any other
  unfinished game. In the results cross-table it is drawn the way it was played:
  **one cell spanning the two rounds**, showing the result once, instead of a
  blank followed by a result that read like two separate games. The grid exported
  for the rating body keeps one column per round, as that format requires.
- **The American Grid export invented a result for an unfinished long game.** A
  long board carries its result into the *next* round's column, so one that had
  not been entered yet was written as a loss for **both** players — a double
  defeat, submitted to the rating body, for a game nobody had played. The export
  now refuses while any long game is unresolved, naming the round, and the
  "Export grid" button says so rather than failing on click.
- **A long game that finished early handed its players a third point.** A long
  board is one game played across two rounds, which is why its winner scores
  two points — but if it was decided inside its own round, or resolved by a
  no-show, its players were freed for the next round and could win there too,
  ending two rounds with three wins where the rest of the field could have at
  most two. The likeliest way to hit it was an opponent who never turned up:
  two free points, then a normal game. A long game now takes both of its rounds
  whichever round it finishes in; a referee who decides it really only took one
  unticks the box before the round advances, demoting it to an ordinary
  one-point board, exactly as before.

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
