# Long boards, second design

Status: **proposed**. Supersedes the model described in
[`two-round-boards.md`](two-round-boards.md), which stays accurate as a record of
what shipped in 1.3/1.4.

## Why change

The shipped model keeps a long game on a single board that lives in its
*starting* round N, and reconstructs everything else by looking across rounds:

- `american_grid.rs` renders column N+1 by reaching back into `rounds[i - 1]`
  (`long_aware_cell`).
- `confirm_round` finds the players to exclude by scanning **every** round for
  `long_pending()` (`busy_long`, `tournament.rs:1222`).
- `prepare_round` carries a second, ad-hoc guard (`UnresolvedLongGame`,
  `tournament.rs:979`) next to the general `PreviousRoundNotComplete` one, purely
  because round N+1's completion cannot see the carried game.

Every one of those is a cross-round lookup, and each has produced a defect:

1. **A pending long board exports as a loss for both players.**
   `long_aware_cell` calls `game_cell` unconditionally in column N+1; with the
   board still `Pending`, `result_marker` returns `-` for both sides. That is a
   double loss submitted to a rating body for a game nobody finished.
2. **Back-to-back long boards drop a result.** The "am I long *this* round?"
   branch is tested before the "did I carry one?" branch, so a game decided
   within round N-1 whose players are flagged long again in round N never renders
   in any column.
3. **The `i - 1` lookup indexes the *completed* slice.** Completed rounds are a
   prefix today, but clearing a result in an earlier round un-completes it and
   punches a hole in that list, after which `rounds[i - 1]` silently reads the
   wrong round.
4. **Round N+1 can complete while the game is outstanding**, which is what makes
   (1) reachable: the guard only blocks preparing N+2.
5. **The players have no record at all in round N+1.** Sit-outs are built from
   `forced_byes`, `swiss_bye`, `cup_byes` and `draft.absent`
   (`tournament.rs:1400-1444`); `busy_long` players are filtered out of the
   pairing pool and never given one. They are simply absent from the data.

The root cause is that one board is asked to belong to two rounds. This design
gives the game one record per round it occupies, and makes every question local
to a single round.

## The model

`Board` gains a kind. The natural spelling puts the outcome *inside* it, so that
the illegal state cannot be written down at all:

```rust
enum GameRecord {
    /// An ordinary one-round game.
    Short(Outcome),
    /// A long game in its starting round, not yet carried. May be decided —
    /// finishing early is the demote path.
    LongStart(Outcome),
    /// The inert record left in the starting round once the game was carried
    /// forward. Has no outcome field: it cannot disagree with anything.
    LongCarried,
    /// The live record of a carried long game, in the round it finishes in.
    LongEnd(Outcome),
}
```

If putting `Outcome` inside the enum is too invasive for one change, the fallback
is a `kind: GameKind` field beside the existing `outcome`, with
`LongCarried ⇒ outcome == Pending` enforced in `validate_loaded` and a
`debug_assert!` at every mutation site. The sum type is preferred: it is the same
move `round.rs` already made with `Outcome`/`Forfeit` to make the old illegal
combinations unrepresentable.

### A long game always consumes two rounds

This is the rule the shipped model gets wrong, and it drives everything below.

A long game occupies rounds N and N+1 **whichever round it actually finishes
in**. Its players are not paired in round N+1 even if the game ended inside round
N, or was resolved by a forfeit. A referee who decides it was really a one-round
game unticks the box *before the round advances*, demoting it to an ordinary
one-point board; that is the deliberate escape hatch, and the only one.

Two reasons it has to work this way:

- **Points must not outrun rounds.** A long board is worth two points
  (`scoring.rs:252, 306, 326` apply `reps = 2` unconditionally). If its players
  are freed to play round N+1 as well, they can score three points across two
  rounds while the rest of the field can score at most two. That is a wrong final
  standing, not a cosmetic oddity.
- **A long cup round must move at one speed.** A bracket round spans two
  tournament rounds as a unit. Letting a quarterfinal pair that finished early
  slip a Swiss game into the gap round desynchronises the bracket from the field.

The shipped behaviour is the opposite, and is documented as a deliberate decision
in `two-round-boards.md:513-518` ("it is no longer `long_pending` and its players
are *not* excluded from round N+1; it scores its `long` weight"). The most likely
way to hit it in practice is a **no-show on a long board**: the present player
takes two free points *and* a normal game the following round.

### Lifecycle

1. The referee flags a board long in the current round N. It becomes
   `LongStart(Pending)`.
2. The game may be decided at any point — inside round N, or during N+1 — or
   resolved by forfeit. Being decided early does **not** cancel the carry.
3. When round N+1 is confirmed, `confirm_round` injects a `LongEnd` board into
   round N+1 with the same pair and **moves the outcome onto it**, flipping the
   round-N record to `LongCarried`. If the game was still pending, the outcome
   moved is simply `Pending`.
4. From then on the round N+1 `LongEnd` board is the only record that can hold
   the result.

The carry is therefore an *outcome-moving* operation, and it is symmetric:
cancelling round N+1 moves the outcome back onto the round-N record as it
reverts to `LongStart`. Exactly one record is authoritative at every instant, in
both directions, which is what makes the two records incapable of disagreeing.

The one-time transition in step 3 is the only mutation of a confirmed round, and
it preserves the pair, so round N's frozen `RoundExplanation` — whose
`BoardLedger` is keyed by `player1`/`player2`, not by index
(`pairing/explain.rs:41`) — remains true: the ledger explains why A was paired
with B in round N, which is still what happened.

`long_pending()` disappears as a concept. Nothing needs to distinguish a decided
long board from a pending one for the purpose of who plays next round, and it was
that distinction that produced the points bug.

### Completion

`complete_for_round` becomes, with no reference to any other round:

```rust
matches!(kind, LongStart(_) | LongCarried) || is_decided()
```

`LongStart` and `LongCarried` never hold their round open — the rest of the field
plays on, which is the whole feature. `LongEnd` is an ordinary board, so round
N+1 stays open until the game is decided. That is the semantics we want, obtained
**locally**, with no cross-round recompute and therefore no risk of the
never-completing deadlock that a neighbour-aware `is_complete` would introduce.

Consequences:

- `UnresolvedLongGame` and its guard (`tournament.rs:979-987`) are deleted.
  `PreviousRoundNotComplete` now covers the case: N+2 cannot be prepared because
  N+1 is not complete while its `LongEnd` board is pending.
- A long game still cannot straddle three rounds, for the same reason, without
  anyone having to say so.

### Rendering

All three surfaces become single-round lookups.

- **American grid.** `long_aware_cell` is deleted. `LongCarried` renders `0-`
  through the existing sit-out/no-game path; `LongStart` renders `0-` while
  pending and its real result once decided (which is the case where the
  tournament ends at round N — the game shows as decided, correctly);
  `LongEnd` and `Short` render normally. Defects (1), (2) and (3) all disappear:
  there is no cross-round read and no branch ordering.
- **Standings cross-table.** Same rule. A `LongStart` in the last, ongoing round
  shows as an ordinary game. A `LongCarried` in column N followed by a `LongEnd`
  in column N+1 is what the UI keys on to draw one cell straddling both columns;
  absent the `LongEnd`, the `LongStart` stands alone.
- **Round view.** The "carried games" section that records into the *previous*
  round goes away: the game is a board of the round being displayed.

### Pairing, explanations, counterfactuals

`PairingSource` gains a variant:

```rust
enum PairingSource { Swiss, Forced, Cup { stage }, Carried { from: u32 } }
```

This is load-bearing and cheap. Engine-paired boards are already selected with
`matches!(b.source, PairingSource::Swiss)` (`tournament.rs:1377, 1590, 1665`),
and `explain_pairing`'s contract is explicitly "engine-paired boards only —
forced/cup boards aren't engine decisions and carry no explanation". A `Carried`
board is exactly that: chosen by the engine in round N, not in round N+1. So it
is excluded from `swiss_boards`, gets no `BoardLedger`, and contributes to no
`RuleTotal`, by the same mechanism that already handles forced and cup boards.
No new special case.

Counterfactuals follow for free. `scope_reason` (`tournament.rs:1864`) matches
exhaustively on `PairingSource`, so adding the variant is a compile error at
every site that must handle it, and `ScopeReason` gains a matching variant so the
UI can say *"still playing a long game from round N"* rather than falling into
the `Absent` bucket. This is the point raised as a risk; it costs one enum arm.

Pairing exclusion narrows but does not vanish. At `prepare_round` time for round
N+1 the `LongEnd` board does not exist yet, so the pool must still exclude those
players. The rule becomes "players with a `LongStart` in the **immediately
previous** round, decided or not" — a single-round check with no outcome
inspection, replacing both the all-rounds `long_pending()` scan at
`tournament.rs:1222` and the two matching validations in the
forced-board/forced-bye loops.

## Issues this design must handle

These are the places where the change is not free. Each is a real interaction,
found by walking the call sites rather than by inspection of the model.

### 1. `force_pairing` would destroy a carried game — must be handled

`force_pairing` (`tournament.rs:1740-1798`) pops the current round and
re-confirms it from a *reconstructed* draft consisting of `absent`,
`forced_boards` and `forced_byes`. A `Carried` board is none of those, so
re-confirming round N+1 would drop it — losing the result and stranding round N's
record in `LongCarried` with nothing pointing at it.

The fix belongs in the same place that already loses information here: this path
also silently discards `long` flags today. `confirm_round_inner` should re-derive
carried boards from the previous round's `LongStart(Pending)` records rather than
from the draft, so that any re-confirmation of round N+1 re-injects them
identically. That also keeps the draft free of derived state.

### 2. `cancel_last_round` must move the outcome back

Cancelling round N+1 flips round N's record from `LongCarried` back to
`LongStart` and **copies the `LongEnd` board's outcome onto it** before deleting
the round. This is not a special case to be decided: it is the exact inverse of
the carry in step 3, and it is what keeps "exactly one record holds the outcome"
true through a cancel. A decided-and-carried game is the normal case, not an
exceptional one, so discarding the result (or refusing the cancel) would both be
wrong.

The invariant to assert on both sides: after a carry, the round-N record holds no
outcome; after a cancel, the round N+1 record is gone and the round-N record
holds whatever the game had reached.

### 3. The result must be enterable on exactly one record

`toggle_board_winner`, `set_board_drawn` and `set_board_no_show` accept any round
number. Attempting any of them on a `LongCarried` record must be a loud error
naming the round the live record lives in — this is what makes the two records
incapable of disagreeing. With the outcome inside the enum, `LongCarried` has no
field to write and the error is a `match` arm; with the fallback flag layout, it
is an explicit guard plus a `validate_loaded` invariant.

### 4. Handicaps travel with the game

`Board::handicap` is part of the game, not of the round. A handicap set on the
round-N board before the carry must move to the `LongEnd` record in step 3,
otherwise the grid's `(-2p)` suffix renders on a `0-` placeholder and vanishes
from the column that shows the result. `set_board_handicap` must also refuse a
`LongCarried` record, as in (3).

### 5. `opponents` multiplicity — verify, do not assume

Scoring records a long-board opponent **twice** in `opponents`
(`scoring.rs:245-249`), which is how the 2× weighting reaches SOS-family
tie-breaks. With one record per round, the accounting has to be restated
explicitly: `LongCarried` has no outcome and must contribute nothing, and
`LongEnd` must contribute whatever multiplicity the rule intends. This is the
one place where a quiet off-by-one would change published tie-breaks, so it wants
a test that pins the SOS of a long game's opponent against the same game played
short.

The same care applies to `elo.rs`, where a long win must keep moving the estimate
exactly like a single normal win (`two-round-boards.md:557`), i.e. `LongCarried`
contributes no game and `LongEnd` contributes one.

### 6. Cup: the schedule keeps working, but only because the record stays

`cup_schedule` (`cup.rs:210-225`) derives the bracket→tournament-round mapping by
asking, for each tournament round t, whether any Cup board there is flagged long,
and advancing by 2 if so. This survives **only** because the round-N record
remains in round N as `LongCarried`. Had the board been physically moved out of
round t, the mapping would collapse back to the identity and the whole bracket
would shift, and `replay_to`/`play_round` would look for the quarterfinal results
in the round they are no longer in. The predicate must therefore be "is this a
long-start-or-carried cup board", not "does it have an outcome".

Note also that `cup.rs` reads `completed` nowhere (the only two occurrences are a
test helper), and every caller passes `&self.rounds` unfiltered, so the
completion change cannot perturb the bracket.

Two existing cup facts to preserve: a bracket round is long as a unit (all cup
boards of the round flip together, `tournament.rs:2086-2092`), and that coupling
currently has a bug — the "not already decided" guard tests the clicked board
only and then writes `long` to every cup board including decided ones,
retroactively doubling a recorded result. Fix that in the same work.

### 7. Team mode: no interaction, by construction

`team_mode_conflict()` rejects long games in team mode, and that rejection is what
makes "a completed team round has no undecided board" true, which several replay
paths depend on. Nothing in this design changes that; it should stay rejected.

### 8. New load-time invariants (fail loud)

`validate_loaded` currently checks almost nothing. This design adds cheap,
checkable invariants that turn a corrupt save into a named error instead of a
panic or a silent mis-score:

- every `LongEnd` in round r has a matching `LongCarried` in round r-1 with the
  same pair, and vice versa;
- a `LongCarried` record carries no outcome;
- no `LongStart` exists in any round before the last, decided or not — once a
  further round exists, every long game must have been carried;
- neither player of a `LongEnd` board appears on any other board or sit-out of
  that round (this is the points-per-round invariant, made checkable);
- `LongEnd` never appears in round 1.

### 9. Save format

`TOURNAMENT_FORMAT_VERSION` bumps. No migration is written, per the project's
stated policy; older files reject loudly. Worth doing in the same release as the
`format_version`-default fix, so that a file *lacking* the field is also rejected
rather than being read as current.

## What this does not fix

- The referee can still leave a long game unresolved when the tournament simply
  ends at round N+1: nothing forces a result if N+2 is never prepared. Under this
  design the export is at least honest (`0-` twice, no fabricated loss), but if a
  stronger guarantee is wanted, it belongs in a "finish the tournament" check, not
  here.
- The `american_grid` design choice — that the game is reported in the column
  where it *became known* (N+1), with N showing a non-game — is unchanged. This
  design only makes the code match it.

## Suggested sequencing

1. Introduce the kind enum with `LongCarried` inert, and make `complete_for_round`
   local. Delete `UnresolvedLongGame`.
2. Add `PairingSource::Carried` and the `ScopeReason` arm; let the compiler find
   the sites.
3. Move the carry into `confirm_round_inner`, derived from the previous round
   (fixes `force_pairing` at the same time).
4. Delete `long_aware_cell`; render from the kind.
5. Add the `validate_loaded` invariants and the load-time rejections.
6. Tests, none of which exists today:
   - a long game decided **inside round N**, asserting its players are absent
     from round N+1 and that they hold 2 points after two rounds, not 3 (the
     scoring bug above);
   - the same via a **no-show** on a long board, which is the likeliest trigger;
   - a pending carried game rendered across an N+1 grid column (the double-loss
     case);
   - back-to-back long boards;
   - a cancelled N+1, asserting the outcome landed back on the round-N record;
   - a long cup round with its results withheld. The existing
     `a_long_cup_round_consumes_two_tournament_rounds_and_the_bracket_resumes_after`
     builds its round-1 boards already decided, so no test currently exercises a
     *pending* long cup board at all.
