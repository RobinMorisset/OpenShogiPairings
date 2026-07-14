# Two-round boards ("long games")

## The rule

Some tournaments give the top boards **double the time control**, so those games
last **two rounds** while the rest of the field plays two ordinary rounds. To
compensate the players who played a single game over two rounds, **the winner of
a long game scores two points** (the loser scores none).

We add this as a tournament-level option. When it is on, every round's pairings
grow an extra column with a **checkbox per board** letting the referee declare
that a given board is a long game — rather than trying to encode an overall
"top N boards are always long" rule. In a hybrid (cup) tournament, ticking the
box on one cup board ticks it on all cup boards of that round, for sanity.

This is a deliberately high-blast-radius feature. This document is the design; it
records the decisions already taken (see [Decisions](#decisions-locked)) and the
per-subsystem plan.

## Terminology

- **Long board** — a board flagged to last two rounds. Its winner scores 2 points.
- **Starting round (N)** — the round in which the long board is paired and its
  game begins.
- **Second round (N+1)** — the next tournament round, during which the field
  plays on while the long game finishes. The long board's two players sit out the
  ordinary pairing of round N+1.
- **Pending long board** — a long board with no recorded result yet. This is the
  state that drives the round-N+1 pairing exclusion and the `0-` placeholder.

## Decisions (locked)

These were settled up front because they change the shape of the implementation:

1. **Scoring model — "two points; two games for tie-breaks, one for ELO."** A
   decided long board gives the winner **+2 points** and **+2 to the victory
   count** (so `points = macmahon + 2·victories` still holds). For the
   **opponent-based tie-breaks** it counts as **two games against the same
   opponent** — the opponent is recorded **twice** in the `opponents`/`defeated`
   lists, so SOS / SODOS / SOSOS / Buchholz all weight that opponent double
   (this is the fix for "a player on many long games would otherwise get an
   unfairly low SOS"). It still feeds the end-of-tournament **ELO computation as a
   single game** (ELO reads the boards, not these lists). For **display**, the
   Results-tab tooltip collapses the duplicate to **one opponent entry with the
   contribution doubled**.
2. **FESA / American-grid export.** The long game shows as **`0-` in the starting
   round's column** and the **actual result in the second round's column** (the
   round in which the result becomes known). This is what "padding the standings
   with `0-`" refers to.
3. **Round gating — exactly two rounds.** A long board spans rounds N and N+1
   only. Its two players are excluded from round N+1's pairing; **round N+2 cannot
   be prepared until the long result is entered.**
4. **Toggle lifecycle.** The long checkbox is live on the **current (in-progress)
   round's boards**. It can be flipped **off** even after the board has a result
   — that is how the referee demotes a long game that actually finished in one
   round, or that resolved by forfeit, back to an ordinary one-point game (so
   those cases need no special scoring rule). Once the round advances, longness is
   frozen. Cup boards toggle together.
5. **Scope of the first commit — Swiss only.** Cup (direct-elimination) support is
   explicitly **deferred to a second commit** (see [Cup interaction](#cup-interaction-deferred-to-a-second-commit)).
6. **Print.** Long boards are **marked on the printed pairing sheet** (a static
   "★ 2R" glyph), so the physical sheet shows which boards run long.

## Data model

### `TournamentSettings` — the enabling flag

`crates/core/src/settings.rs`

Add an additive, defaulted `bool`:

```rust
/// Whether the referee may flag individual boards as "long games" that last
/// two rounds and score two points for the winner (double time control on the
/// top boards). Off by default. When on, the round view shows a per-board
/// checkbox. See `docs/two-round-boards.md`.
#[serde(default)]
pub long_boards_enabled: bool,
```

Wire it through `Default`, and — importantly — through `normalized()`: **if the
flag is turned off, there is nothing to strip** (existing long flags on already
paired boards stay meaningful), so `normalized()` needs no change beyond leaving
the field alone. It is a pure "show the column / accept the toggle" gate.

Regenerate `frontend/src/lib/generated/TournamentSettings.ts` (ts-rs).

### `Board` — the per-board flag

`crates/core/src/round.rs`

Longness is **orthogonal** to `PairingSource` (a long board can be Swiss, Forced,
or Cup), so it is a separate boolean, not a new `PairingSource` variant:

```rust
/// This board is a "long game": double time control, lasts two rounds, and its
/// winner scores two points instead of one. Off by default and omitted from
/// JSON when false. See `docs/two-round-boards.md`.
#[serde(default, skip_serializing_if = "is_false")]
pub long: bool,
```

Add small helpers next to `is_decided`:

```rust
impl Board {
    /// A long board whose game hasn't finished yet — the state that makes the
    /// two players sit out the next round and shows the `0-` placeholder.
    pub fn long_pending(&self) -> bool {
        self.long && !self.is_decided()
    }
}
```

`Board::pending` gains no new argument — it defaults `long: false`; the flag is
set afterwards through the new `set_board_long` mutation (below). Every existing
`Board { .. }` struct literal in tests and in `force_pairing` / `update_draft`
picks up `long: false` via `..Board::pending(...)`, so the churn is limited to the
literal in `round.rs` itself.

Regenerate `frontend/src/lib/generated/Board.ts`.

### No new stored round state

Following the same philosophy as the cup (derive, don't store), we do **not**
add a "carried players" list to `Round`. The set of players busy on a pending
long board is **derived** by scanning `self.rounds` for `board.long_pending()`.
This keeps undo, replay, and result-editing correct for free.

`TOURNAMENT_FORMAT_VERSION` does **not** need bumping: every change is an
additive, defaulted field, exactly like the existing settings and board fields,
so old saves load unchanged (no long boards).

## Round lifecycle

`crates/core/src/tournament.rs`

### 1. Completion ignores pending long boards

Today a round is complete iff **every** board `is_decided()`
(`toggle_board_winner` / `set_board_no_show` recompute
`round.completed = round.boards.iter().all(|b| b.is_decided())`).

Redefine "complete" to treat a pending long board as not blocking:

```rust
// A long board may still be running while the rest of the round is done;
// it must not hold the round open (that is the whole point of the feature).
round.completed = round.boards.iter().all(|b| b.is_decided() || b.long);
```

Extract this into a `Round::is_complete()` (or a free helper) and use it in
**both** mutation sites plus anywhere else that recomputes it. Note the server's
`round_completed()` reads the stored `completed` flag, so it needs no change once
the flag itself is computed correctly.

Consequence: a round can be `completed` while it still contains an undecided
board. Audit every `all(is_decided)` / `is_decided` assumption:

- `prepare_round` gating (`last.completed`) — fine, reads the flag.
- `force_pairing` refuses a round that `round.boards.iter().any(is_decided())` —
  fine; a long board being *un*decided doesn't trip it, and re-pairing a round
  with an unfinished long game is legitimately forbidden by the results check.

### 2. Excluding the busy players from round N+1

In `confirm_round`, alongside the existing `cup_players` set, build a
`busy_long` set from the previous rounds:

```rust
let busy_long: HashSet<Uuid> = self
    .rounds
    .iter()
    .flat_map(|r| &r.boards)
    .filter(|b| b.long_pending())
    .flat_map(|b| [b.player1, b.player2])
    .collect();
```

Subtract `busy_long` from `present` the same way cup players are subtracted, so
the Swiss engine never re-pairs them, and reject any forced board / forced bye
that references a busy player (mirror the existing cup-player validation errors,
with a new `TournamentError::LongGameInProgress`-style variant, or reuse
`InvalidDraft` with a clear message).

Parity is naturally preserved: each long board removes exactly **two** players,
so an even field stays even; a cup that couples several long boards removes an
even count too.

`prepare_round`'s `default_absent` must **not** fold busy-long players into the
absent set — they are not absent, they are mid-game. Leave them out; they are
handled purely by the `confirm_round` exclusion.

### 3. Gating round N+2

A long board spans exactly two rounds, so before preparing the round **after**
the second round, its result must be in. Add to `prepare_round`:

```rust
// A long game started in round R excludes its players from round R+1; it must
// be resolved before R+2 is prepared, so it can never straddle three rounds.
if let Some(stale) = self.rounds.iter().find(|r| {
    r.number + 1 < next_round_number && r.boards.iter().any(|b| b.long_pending())
}) {
    return Err(TournamentError::UnresolvedLongGame { round: stale.number });
}
```

(`next_round_number = self.rounds.len() as u32 + 1`.) This is the concrete
enforcement of decision #3.

### 4. The new `set_board_long` mutation

```rust
/// Flag (or unflag) a board as a long game (two rounds, two points). Allowed
/// only on the current (last) round (decision #4); unflagging is allowed even
/// after a result, so the referee can demote a game that finished in one round
/// or resolved by forfeit. For a cup board, the flag is applied to *every* cup
/// board of that round, so a whole cup round is long or not as a unit.
pub fn set_board_long(
    &mut self,
    round_number: u32,
    board_index: usize,
    long: bool,
) -> Result<&Round, TournamentError> { ... }
```

Guards:
- The round must be the **current** round (`self.rounds.last()`), else
  `TournamentError::NotCurrentRound` (new) — longness is frozen once the round
  advances.
- **Flagging on** (`long = true`) requires the board undecided (`!is_decided()`,
  else `RoundHasResults`) — you don't retroactively make a finished game long.
  **Flagging off** (`long = false`) is always allowed on the current round, so a
  finished-early / forfeited long game can be demoted (decision #4).
- Gate behind `self.settings.long_boards_enabled` (reject otherwise, defence in
  depth; the UI already hides the column).
- **Cup coupling** (second commit): if the target board's `source` is
  `Cup { .. }`, set `long` on all boards in the round whose source is
  `Cup { .. }`; otherwise set it on just the one board. Until the cup phase lands,
  a long toggle on a cup board can simply be rejected.

Returns the whole round (the UI needs the coupled cup boards refreshed).

## Scoring

`crates/core/src/scoring.rs`

Everything keys off the effective winner as today. A long game is modelled as
**two games against the same opponent** for scoring and tie-breaks (but one game
for ELO). Concretely:

In the **opponents-recording** loop (the one that runs before results are
applied), push each player into the other's `opponents` list **twice** for a long
board:

```rust
let reps = if board.long { 2 } else { 1 };
for _ in 0..reps {
    by_id.get_mut(&a).unwrap().opponents.push(b);
    by_id.get_mut(&b).unwrap().opponents.push(a);
}
```

In the **apply results** loop, double the points, victories, and defeated entries:

```rust
// A long board (double time control) is worth two points and counts as two
// games against the same opponent for the point/victory totals and the
// opponent-based tie-breaks. It stays a single game for ELO (which reads the
// boards directly, below/elsewhere).
let reps = if board.long { 2 } else { 1 };
if let Some(s) = by_id.get_mut(&winner) {
    s.points += 2 * reps;      // half-point units: 4 for long, 2 normal
    s.victories += reps;       // 2 for long, 1 normal
    for _ in 0..reps { s.defeated.push(loser); }
}
```

Notes:

- `points += 4`, `victories += 2`, and two `defeated` entries keep every relation
  consistent (`points == macmahon + 2·victories`, `victories == defeated.len()`).
- Because `opponents` and `defeated` each hold the opponent **twice**,
  `standings.rs` needs **no change**: SOSM/SOSW (sum over `opponents`), SODOS (sum
  over `defeated`), SOSOS (sum of opponents' SOS), and the **Buchholz cuts** (drop
  the lowest per-entry) all weight the long opponent double automatically — a long
  game behaves exactly like two ordinary games versus that opponent. Direct
  confrontation likewise counts it as two head-to-head wins, consistent with the
  doubled victory count. (Duplicate opponent entries are not unprecedented — an
  actual rematch already produces them.)
- **ELO stays one game.** `estimate_elos` (`crates/core/src/elo.rs`) iterates the
  decided boards once, so it already treats a long board as a single result —
  confirm it does not read `long` and does not double-weight.
- **Pending long board:** while the game is undecided the board's `result` is
  `None`, so the results loop `continue`s and awards nothing — the two players
  show their prior totals with no points from this game. That is the "padded"
  standings state. The opponents are still recorded (the earlier loop pushes them
  for any non-no-show board), so the game is visibly "in progress" but unscored.
- **CUSS attribution.** Cumulative tie-breaks sum running totals over completed
  rounds. Because the board lives in the starting round N, once the result is
  entered the two points attribute to round N's running total (round N is
  `completed`). This is invisible unless CUSSM/CUSSW is a selected tie-break, and
  is internally consistent; call it out in the doc/tests. (Attributing CUSS to
  N+1 would require moving the board, which we explicitly avoid.)

## Standings and tie-breaks

`crates/core/src/standings.rs` needs **no structural change** — it consumes
`compute_scores` output. The doubled points/victories and the duplicated
`opponents`/`defeated` entries flow through `points`, `victories`, `sosm/sosw`,
`sodosm/sodosw`, `sososm/sososw`, `sosm1/2`, `sosw1/2`, `cussm/cussw`, and `dc`
automatically — a long game is simply "two games versus that opponent" everywhere
the tie-breaks look. Update the module doc comment (which states "a bye as a win")
to mention that a long win scores two points / two victories / a doubled
tie-break weight.

The **Results tab** (frontend) shows aggregate points + tie-breaks + a per-round
breakdown, and a per-cell **tooltip** built from `standing.opponents` /
`standing.defeated` (`sumTerms` / `droppedTerms` in
`frontend/src/lib/components/ResultsView.svelte`). Because those lists now hold a
long opponent **twice**, the tooltip must **collapse the duplicate to one row with
the contribution doubled** — e.g. show `Smith (4)` once, not `Smith (2) + Smith
(2)`:

- For the plain-sum metrics (`sumTerms`), group the ids by opponent and render one
  term per opponent whose value is `count × score` (so it still adds up to the
  server's number).
- For the **Buchholz-cut** metrics (`droppedTerms`), the cut operates per entry
  and one of a long opponent's two entries can legitimately be the dropped-lowest
  while the other is kept — so collapsing to a single row is only unambiguous when
  both entries fall on the same side of the cut. Recommend: collapse when both are
  kept or both dropped; otherwise fall back to showing the two entries. This is a
  display nicety only — the numeric total is already correct from the server.

A player mid-long-game shows their opponent recorded but no points yet — no
special UI needed, though a small "long game in progress" hint is a nice-to-have.

## American Grid export

`crates/core/src/american_grid.rs`

Today `round_cell` is computed one round at a time and, for a player with no
board that round, returns `0-` (or `0=`). We need the long board — which lives in
the **starting** round N — to render:

- **`0-` in round N's column** (placeholder; the game is unfinished *as of that
  round*), and
- **the decisive result in round N+1's column** (`<opp-rank><+/-/=>`), because
  that is the round in which the result is known.

Since `round_cell` currently sees only one `Round`, refactor `row_for` to pass
the round **index** and the full `&[&Round]` slice, so the cell function can:

1. If the player is on a **long** board in round `rounds[i]` → emit `0-`
   (regardless of the board's result — its result belongs to the next column).
2. Else if the player is on a **long** board in round `rounds[i-1]` → emit that
   board's `<opponent><marker><handicap?>` here (this is the second-round column
   carrying the first-round game's result).
3. Else the existing logic.

Edge cases:
- If round N is the **last** completed round and the long game is still pending,
  the result has nowhere to land yet — round N shows `0-` and there is no N+1
  column. Fine; it appears once N+1 exists.
- Opponent rank references still resolve through the final standings `rank_of`,
  unchanged.

### Import / round-trip

`crates/core/src/grid_import.rs` and `crates/core/src/fesa_results.rs`
parse grids back in. Reconstructing a long board from a `0-`-in-N /
result-in-N+1 pattern is genuinely ambiguous (a `0-` in N already means "absent"
in a normal grid, and the pairing lives across two round columns). **Recommend
scoping the reverse round-trip out of the first version:** importing such a grid
will read the second-round result as an ordinary game and the `0-` as an absence,
which does not perfectly reconstruct a long board. Flag this in the importer docs.
Only invest in exact reimport if these tournaments are actually re-imported (the
common case is export-only, for federation ELO submission). This is the main
**known lossy** area — see [Risks](#risks).

## Cup interaction (deferred to a second commit)

> **Scope note (decision #5):** everything below is **out of the first commit**.
> The first commit ships Swiss-only long boards; a long toggle on a cup board is
> rejected until this phase lands. This section is the plan for that second pass.

`crates/core/src/cup.rs`

A hybrid tournament's cup currently assumes a **1:1 mapping** between cup rounds
and tournament rounds: `is_cup_round(r) = r <= cup_rounds()`, and
`matches_for_round(rounds, r)` derives the bracket state by replaying tournament
rounds `1..r`. Making a cup round **long** breaks that 1:1 assumption: a single
bracket round now consumes **two** tournament rounds.

Concretely, when the cup boards of tournament round N are long:

- Round N holds the cup matches (long, pending). Cup players are excluded from
  round N's Swiss pool as usual.
- Round N+1 must **not** advance the bracket (the round-N cup games aren't
  decided). Cup players are all busy (pending long boards), so they are excluded
  from round N+1 by the same `busy_long` mechanism — round N+1 is an ordinary
  Swiss round for **everyone else**.
- Round N+2 resumes the cup: it should present the *next* bracket round, fed by
  round N's now-decided results.

The core difficulty is `matches_for_round`'s round arithmetic. Options:

- **(Recommended) Derive the cup-round→tournament-round offset from long flags.**
  Teach the cup to skip the extra tournament round a long cup round consumed:
  when replaying, a cup round that was long occupies two tournament-round slots.
  `is_cup_round` / `matches_for_round` compute the tournament round for each
  bracket round by walking the rounds and counting long cup rounds as two. This
  keeps the "derive from results" philosophy but makes the mapping data-driven
  rather than `r <= cup_rounds()`.
- **(Alternative) Store the mapping.** Record, on the `Cup`, which tournament
  round each bracket round occupies. Simpler arithmetic, but adds stored state
  that must survive undo/cancel — against the current derive-don't-store grain.

Because the cup replay (`replay_to`, `play_round`, `decide`) reads boards by
`round.number`, and the boards physically live in their starting round, the
replay largely keeps working as long as the **frontier→tournament-round** mapping
is corrected. `podium()` and `draft_cup_players()` ride on the same mapping.

**Cup coupling of the toggle** (decision #4) is handled in `set_board_long` (all
cup boards of the round flip together), so the referee can never create a
half-long cup round — which is exactly the invariant the cup arithmetic relies
on.

Given the depth here, a reasonable **phasing** is to ship long boards for the
**Swiss** case first (fully useful on its own, and the case the user actually
experienced), then add cup support in a second pass with the mapping rework and
its own test matrix. Flag this split to the user.

## Server API

`crates/server/src/tournament.rs`

- **New route + handler** for the long toggle, mirroring `set_board_no_show`:
  `POST /rounds/{round_number}/boards/{board_index}/long` with a `{ "long": bool }`
  body, calling `Tournament::set_board_long`. Register it in `scope()` in the
  protected group. Take the automatic "round N completed" backup on the same
  completed-transition logic already used by `set_board_result` /
  `set_board_no_show` if toggling long changes completion (it can: flagging the
  last-undecided board long completes the round).
- `TournamentView` (the response) already serializes rounds and thus each board's
  new `long` field automatically. `effective_winners` is unaffected (longness
  doesn't change *who* won, only the point weight, which is computed in scoring).
- **New `TournamentError` variants** (`NotCurrentRound`, `UnresolvedLongGame`,
  `LongGameInProgress`/reuse `InvalidDraft`) need mapping to HTTP status in the
  server's error module (`crates/server/src/error.rs`) — follow how the existing
  `TournamentError`s map (validation → 4xx).

Also check the **Tauri** command surface (`frontend/src-tauri/src/lib.rs`) if it
mirrors the server routes for the desktop build — add the equivalent command.

## Frontend

### Settings toggle

`frontend/src/lib/components/TournamentSettingsView.svelte` — add a checkbox
bound to `long_boards_enabled`, next to `cup_enabled` / `club_protection_enabled`
(follow the exact `sCup` / `cupEnabled` pattern at lines ~130/203). Add i18n
strings in `frontend/src/lib/i18n/`.

### Round view — the checkbox column

`frontend/src/lib/components/RoundView.svelte`

- New prop `longEnabled: boolean` and `onSetLong(boardIndex, long)`.
- When `longEnabled`, add a `Long` column header and, per game row, a checkbox
  bound to `board.long`. It is enabled only on the **current** round; ticking it
  **on** additionally requires the board undecided, but **un**ticking a decided
  long board stays enabled (decision #4 — demoting an early-finished/forfeited
  long game). Reuse the `busy` disabling pattern for the in-flight state.
- The checkbox for a cup board should visually reflect that the whole cup round
  toggles together (e.g. tick all cup rows when any is ticked — the server
  already enforces it; the optimistic UI should mirror it).
- **Print (decision #6):** hide the interactive checkbox in print (`print-hide`,
  like the draw/no-show columns) but render a static **"★ 2R"** marker on long
  boards, so the printed pairing sheet shows which boards run long. (Reuse the
  `src-col` glyph slot, or add a small print-only span on the board row.)

### Carried long boards in the second round's view

When round N+1 is the current round, its `round.boards` do **not** contain the
long board (it lives in round N). Surface the pending long boards from round N as
a **read-only "carried games" section** at the top of the round N+1 view (styled
like the cup rows), with the winner buttons wired to
`toggle_board_winner(N, idx, ...)` so the referee records the long result from the
round they are actually running. The parent component (`App.svelte`) supplies the
previous round's pending long boards.

This also gives the referee the natural place to see "these two are still on their
long game" while pairing/scoring round N+1.

### Wiring

`frontend/src/App.svelte` (and `frontend/src/lib/api.ts`) — add the `setLong`
API call and thread `longEnabled` + the carried-boards list into `RoundView`.

## Undo / backups

`Tournament::undo` and the backup machinery operate on whole-tournament snapshots,
so they need no special handling — flagging/unflagging long, and recording the
long result, are ordinary mutations captured by the existing undo stack. The one
thing to verify: the "round N completed" backup transition (server
`set_board_result` / `set_board_no_show`) should also fire from `set_board_long`
when flagging the last-undecided board long tips the round into `completed`.

## Edge cases

- **Long game finishes early (within round N)** or **resolves by forfeit /
  no-show.** No special scoring rule (decision #4): the board is decided, so it is
  no longer `long_pending` and its players are *not* excluded from round N+1; it
  scores its `long` weight (two points). If the referee decides it should *not*
  count as a long game after all (it really only took one round, or a forfeit
  shouldn't be worth two), they simply **untick the box** on the current round,
  demoting it to an ordinary one-point board. This is why the toggle-off guard
  stays open after a result. The no-show scoring path in `compute_scores` must
  therefore also honour `board.long` (a forfeited long board is two points unless
  demoted).
- **Un-ticking long** is allowed on the current round even after a result (the
  demote path above). Once the round advances, the flag is frozen.
- **Forcing / re-pairing.** `force_pairing` refuses a round with any decided
  board; a long board is undecided, so a round can still be re-paired while a long
  board is pending — but re-pairing regenerates boards and would drop the `long`
  flag. Since longness is set *after* confirm and only on the current round,
  re-pairing (which rebuilds the current round) legitimately clears it; the
  referee re-ticks. Document this; optionally preserve long flags across a
  re-pair for boards whose players are unchanged (nice-to-have).
- **Odd field with a long board.** Removing two players keeps parity; the bye
  logic is untouched.
- **Half-point absences interaction.** Orthogonal — a long board's players aren't
  in `round.absent`.

## Testing plan

Rust (unit, alongside the existing dense test suites in each module):

- `scoring.rs`: a decided long board → winner +2 points / +2 victories / **two**
  `opponents` + **two** `defeated` entries; loser 0. Pending long board → no
  points, opponent recorded (twice). Long win by forfeit (no-show) honours the
  weight; unticking demotes it to one point. CUSS attribution to round N.
- `standings.rs`: SOS/SODOS/SOSOS/Buchholz all count the long opponent **twice**;
  wins-only tie-breaks see the doubled victory count; the relations
  `points == macmahon + 2·victories` and `victories == defeated.len()` hold.
- `tournament.rs`: round N completes with a pending long board; round N+1 pairing
  excludes the two players (and rejects forcing them); `set_board_long` guards
  (current round only, undecided only, gated by the setting); cup coupling flips
  all cup boards; `prepare_round` refuses round N+2 while the long game is
  unresolved; entering the result later unblocks it.
- `cup.rs`: a long cup round consumes two tournament rounds; the bracket resumes
  correctly in N+2; podium/`draft_cup_players` follow the shifted mapping.
- `american_grid.rs`: `0-` in the starting column, result in the next column;
  handicap suffix still renders on the second-round cell; last-round-pending case.
- `elo.rs`: a long win moves the estimate exactly like a single normal win.

Frontend: the checkbox column shows only when enabled and is disabled per the
rules; carried-games section renders and records into the previous round.

End-to-end (`/verify`): enable the setting, pair a round, flag the top board long,
finish the other games (round completes), pair the next round (long players out),
enter the long result, confirm the winner shows two points and the cross-table
shows `0-` then the result.

## Blast-radius file checklist

Core:
- `crates/core/src/settings.rs` — `long_boards_enabled` flag (+ default).
- `crates/core/src/round.rs` — `Board.long`, `long_pending`, `Round::is_complete`.
- `crates/core/src/tournament.rs` — completion rule, `confirm_round` exclusion,
  `prepare_round` N+2 gating, `set_board_long`, new error variants.
- `crates/core/src/scoring.rs` — weighted win points/victories.
- `crates/core/src/standings.rs` — doc only (behaviour flows through).
- `crates/core/src/cup.rs` — cup-round↔tournament-round mapping (phase 2).
- `crates/core/src/american_grid.rs` — `row_for`/`round_cell` cross-round render.
- `crates/core/src/grid_import.rs`, `fesa_results.rs` — note lossy reverse
  round-trip (or handle, if required).
- `crates/core/src/elo.rs` — confirm single-game weighting (likely no change).

Server:
- `crates/server/src/tournament.rs` — `set_board_long` route/handler; backup on
  completion transition.
- `crates/server/src/error.rs` — map new error variants.

Frontend:
- `frontend/src/lib/generated/{TournamentSettings,Board}.ts` — regenerate.
- `frontend/src/lib/components/TournamentSettingsView.svelte` — settings toggle.
- `frontend/src/lib/components/RoundView.svelte` — checkbox column + carried-games
  section + print marker.
- `frontend/src/App.svelte`, `frontend/src/lib/api.ts` — wiring + API call.
- `frontend/src/lib/i18n/` — new strings.
- `frontend/src-tauri/src/lib.rs` — Tauri command parity, if applicable.

## Risks

- **Cup mapping** is the highest-risk change and is why cup support is deferred to
  a second commit: the 1:1 cup-round↔tournament-round assumption is baked into
  `cup.rs`.
- **American-grid reverse import** is lossy for long boards; scoped out of the
  first version unless re-import is a real workflow.
- **`completed` no longer implies "all boards decided"** — this is the assumption
  most likely to surprise future code; centralizing it in `Round::is_complete`
  and documenting it mitigates that.
- **Duplicate opponent entries in the tooltip** — the backend correctness (count
  twice) is easy; the display "show once, doubled" is the finicky part, especially
  for the Buchholz cuts (see [Standings and tie-breaks](#standings-and-tie-breaks)).

## Resolved decisions

All the questions raised during design are now settled and folded into
[Decisions](#decisions-locked): a long game finishing early or by forfeit needs no
special rule (the referee unticks to demote); long boards are marked "★ 2R" on the
printed sheet; and cup support is deferred to a second commit. No open questions
remain — the design is ready to implement (Swiss phase first).
