# Long boards, second design

Status: **implemented**. Supersedes the model described in
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

**Implemented** in `crates/core/src/round.rs` as the sum type above — the fallback
of a `kind` flag beside the existing `outcome` was not needed. It is the same move
`round.rs` already made with `Outcome`/`Forfeit` to make the old illegal
combinations unrepresentable.

Two details settled during implementation:

- **Adjacently tagged on the wire** (`#[serde(tag = "kind", content = "outcome")]`),
  giving `{"kind":"short","outcome":{…}}` and a bare `{"kind":"long_carried"}`.
  Internal tagging cannot work here: `Outcome` is *itself* tagged `kind`, so
  flattening it in would emit that key twice.
- **`Board::outcome()` is an accessor**, returning `Pending` for `LongCarried`,
  and `set_outcome` panics on it — reaching that is a bug in core, since every
  API-facing caller must refuse the board first. The frontend's `outcomeOf` in
  `boardOutcome.ts` mirrors the same rule, so the ~50 call sites that only ever
  *read* an outcome were unaffected by the change.

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
  through the existing sit-out/no-game path; `LongStart`, `LongEnd` and `Short`
  all render their result in the column being drawn — for `LongStart` that is the
  tournament-ends-at-round-N case, where the game correctly shows as decided.
  Defects (1), (2) and (3) all disappear: there is no cross-round read and no
  branch ordering.

  Note there is deliberately **no** "unfinished long game" rendering. An earlier
  draft of this document had `LongStart` render `0-` while pending, but the
  export now refuses the whole document while any long game is unresolved, so no
  such record can reach the renderer. That is asserted rather than handled: a
  `0-` fallback would paper over a regression in the guard by quietly reporting a
  played game as no game at all.
- **Standings cross-table.** A `LongCarried` in column N followed by a `LongEnd`
  in column N+1 is drawn as **one cell straddling both columns**, showing the
  result once — because it is one game. A `LongStart` in the last, ongoing round
  stands alone in its own column: only one of its two rounds exists so far.

  **Implemented** as `crossTableColumns` (`frontend/src/lib/crossTable.ts`),
  which is the layout decision — which columns a cell covers and which round
  supplies its content — separated from the rendering so that it can be tested.
  That separation is the point: this rule has now been got wrong twice, both
  times by deciding a column's content from a *neighbouring* round, and both
  times invisibly, because no fixture could build a carried game. The fixture can
  now (`carriedLongGame` in `boardFixture.ts`, which yields both halves at once
  since half a carried game is a save the server refuses).

  This is the one place the app and the American Grid deliberately **differ**,
  and neither should be "fixed" to match the other. The grid is a fixed-format
  document for a rating body: one column per round, always, so `LongCarried`
  renders `0-` and the result lands in the round it was finished in. The
  cross-table is read by people, where two columns of a single game showing `0-`
  and then a result is exactly the thing that reads as two games.
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

Implemented as `Tournament::long_players_busy_in`, and note the emphasis on
`LongStart`: the first implementation of this reused `Board::is_long`, which is
also true of the `LongCarried` and `LongEnd` records the carry writes. That
excluded the players from the round *after* the game as well, where — with no
`LongStart` left to carry — nothing then gave them a board or a sit-out either,
reintroducing defect (5) above one round later. `LongStart` is the only record
that means "this game still reaches into the next round".

The frontend does not restate the rule. It arrives on the response as
`draft_long_players`, beside the `draft_cup_players` it already had, because the
frontend's own copy of this predicate had the identical bug — not independently,
but because it was a copy.

## Issues this design must handle

These are the places where the change is not free. Each is a real interaction,
found by walking the call sites rather than by inspection of the model.

### 1. Every path that drops a round must un-carry — handled

`force_pairing` pops the current round and re-confirms it from a *reconstructed*
draft of `absent` / `forced_boards` / `forced_byes`. A carried board is none of
those, so a naive pop strands the previous round holding an inert `LongCarried`
with nothing to make it live again: re-confirming finds no `LongStart` to carry,
and the game — result and all — disappears.

Resolved by making the carry derived from the previous round rather than from the
draft (so re-confirmation rebuilds it identically), and by routing **every** drop
through `pop_round_uncarrying`, which moves the outcome back before removing the
round. `cancel_last_round` and both `force_pairing` variants use it; a bare
`self.rounds.pop()` is now the bug.

### 2. `cancel_last_round` must move the outcome back — handled

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

### 4. Handicaps travel with the game — handled

`Board::handicap` is part of the game, not of the round, so it moves onto the
`LongEnd` record with the outcome and comes back with it on a cancel. What stays
on the inert record is only what the *round* decided: who was paired, and their
float. Leaving a copy of the handicap behind would duplicate it, and a duplicate
is a thing that can disagree — it would also put the grid's `(-r)` suffix on the
`0-` placeholder cell, which renders no game at all.

### 4b. Every write to the inert record is refused — handled

Issue (3) above was under-specified, and the gap was live: nothing stopped a
client addressing the board in the round its game *started*, which the round tab
still shows. `toggle_board_winner` would have **panicked** there (a carried record
has nowhere to put an outcome, so `set_outcome`'s `expect` fires) and
`set_board_handicap` would have silently written a handicap that scores nothing.
Both reachable from ordinary UI, so neither is the "bug in this crate" case a
panic is for.

`refuse_if_carried` now guards the result, draw, no-show and handicap paths with
`CarriedLongGame { round }`, naming the round the game is finished in.

`set_board_long` is deliberately **not** guarded that way. Its own
`NotCurrentRound` check already covers the inert record — which is always in a
past round — and says the truer thing, since the length cannot be changed in the
next round either. What it did need was the opposite guard: the *live* record is
in the current round, and demoting it there would have turned it into an ordinary
board and orphaned its partner, producing a file `validate_loaded` refuses. That
is `LongGameStartedEarlier { round }`, mirrored by the frontend disabling the
checkbox.

### 4c. A player mid-long-game is not absent — handled

The draft offered them an absence checkbox, and marking it produced a round where
they held a sit-out *and* the carried board. That double-scored them, and the
cross-table takes the sit-out in preference to the board
(`american_grid.rs`'s `round_cell` looks up the sit-out first), so the long game
vanished from the export it was about to be submitted in.

Three layers, because each catches a different caller: `confirm_round` refuses a
draft whose absent list names a player busy on a long game; `prepare_round` no
longer *defaults* them into it (a no-show on the long board resolves the game but
does not release its players, so they were being proposed automatically); and
`validate_loaded` rejects a file where a `LongEnd` board's player also has a
sit-out that round. The draft UI drops them from the absence list altogether —
cup players stay, since marking one absent is how a bracket forfeit is recorded.

### 5. `opponents` multiplicity — handled, and it was wrong

Scoring records a long-board opponent **twice** in `opponents`, which is how the
2x weighting reaches the SOS-family tie-breaks. The multiplier was per *board*,
and the opponents loop filters only forfeits — not undecided boards — so once the
game held one record per round, and both rounds were completed, the opponent was
counted **four** times. A wrong published tie-break, not an internal detail.

The multiplier is now per record: `LongCarried` and `LongEnd` contribute one game
each, adding to the same two, which also says the truer thing — one game faced per
round played. An uncarried `LongStart` (the tournament ended on it) is the whole
game in one record and still counts two on its own. Pinned by
`a_long_game_is_two_opponents_faced_not_four`.

The same loop reads each board's frozen float, and a carried game holds a record
in both of its rounds, so the float marker lands on the round the game **ended**
in — the later write wins. That is the intended reading, and it is worth stating
because the opposite is tempting: the float rules decay by distance from this
marker, and a player who has only just finished a game they floated into is
exactly as fresh a floater as one who finished an ordinary game that round. The
game being long does not make the float older. Recording it from the round the
game was *paired* in would discount the repeat penalty by one round, for no reason
the player would recognise. Pinned by
`a_carried_long_games_float_counts_from_the_round_it_ended_in`.

Points and victories needed nothing: they come from `effective_winner`, which is
`None` on the inert record, and the `LongEnd` keeps the 2x weight. Nor does
`elo.rs`, which reads the boards directly rather than these lists and sees one
decided record per game.

### 6. Cup: the schedule keeps working, but the result lookup had to follow

`cup_schedule` (`cup.rs:210-225`) derives the bracket→tournament-round mapping by
asking, for each tournament round t, whether any Cup board there is a long game,
and advancing by 2 if so. This survives **only** because the round-N record
remains in round N as `LongCarried`. Had the board been physically moved out of
round t, the mapping would collapse back to the identity and the whole bracket
would shift.

What the original analysis missed: the schedule survives, but the *result* lookup
does not. `decide` (`cup.rs`) reads the outcome off the board in round t, and
after the carry that record is the inert one. A long quarterfinal therefore
replayed as undecided and the bracket answered `CupBracketInconsistent`. Fixed by
having `decide` follow a `LongCarried` record forward to its `LongEnd` in round
t+1 — the schedule still keys off round t, only the outcome is looked up one
round later. Caught by
`long_cup_round_couples_all_cup_boards_gaps_the_next_round_and_resumes`.

Two existing cup facts preserved: a bracket round is long as a unit (all cup
boards of the round flip together), and that coupling's decided-board guard bug
is unchanged by this work.

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

That fourth bullet turned out to be a special case of something worth stating in
general, and `validate_round_membership` now states it: **every player is
accounted for exactly once in every round they are part of** — one board or one
sit-out, never both and never neither. It subsumes the `LongEnd`-vs-sit-out check
and closes the board-vs-board half that was never written, but its real value is
the *other* direction, a player with no record at all: that is what defect (5)
was, and what the `is_long` bug above reintroduced, and nothing else in the file
would have noticed either.

`validate_round_membership` states it without exception, which it can because a
player registered after the rounds started is now marked absent for each one
already played (`add_player`). That was worth doing on its own account — it is
the same situation as a player who was registered from the start and marked
absent, and `half_point_absences` is meant to keep both of them near the field
they belong to rather than dropping them among the beginners for the rest of the
event. Only one of the two was getting it. Removing the last exception to this
invariant came free with the fix.

Two consequences worth knowing, neither of which forced a new `SitoutKind`:

- **The next draft offers a late joiner pre-marked absent**, since
  `Round::absentees` selects on kind and `prepare_round` defaults from it. One
  untick, and accepted deliberately: dodging it means a sit-out kind of its own
  and every exhaustive match over `SitoutKind` — scoring, the grid,
  `scope_reason`, the UI label and its nine translations — to save a click.
- **The exported grid is unchanged.** `SitoutValue::Zero.cell()` and the
  no-record path both render `0-`, so with `half_point_absences` off the document
  is byte-identical to before. With it on, the late joiner's missed rounds now
  read `0=` instead of `0-`, which is the fix.

The check is also asserted at the end of both confirm paths in debug builds,
which makes every test that confirms a round a test of it — and is how a cup test
that had been walking straight through the `is_long` bug above started failing on
it.

### 9. Save format

`TOURNAMENT_FORMAT_VERSION` bumps. No migration is written, per the project's
stated policy; older files reject loudly. Worth doing in the same release as the
`format_version`-default fix, so that a file *lacking* the field is also rejected
rather than being read as current.

## What this does not fix

- The `american_grid` design choice — that the game is reported in the column
  where it *became known* (N+1), with N showing a non-game — is unchanged. This
  design only makes the code match it. (The app's own cross-table deliberately
  diverged later; see Rendering above.)

*Resolved since:* an earlier draft of this section said the referee could leave a
long game unresolved at the end of the tournament and that the export would then
be "at least honest (`0-` twice)". Both halves were wrong. The export refuses
outright while a long game has no result, and it now also refuses one that was
never carried — see "Scoring" below.

## Scoring

**Exactly one record of a long game ever scores it**, and it is the `LongEnd`,
at the full two-point weight. `LongCarried` scores nothing — it has no outcome to
score from. `LongStart` scores nothing *either*, and that is the part worth
writing down, because it is not obvious and it is not merely tidiness.

The model that pairs round N+1 is replayed from the rounds below it, and
confirming N+1 rewrites round N's record from `LongStart` to `LongCarried`. If
the two scored differently, the model that paired the gap round could never be
recomputed once it had been paired: `explain_counterfactual` would answer
questions about that round from a model that never paired it, silently. Since
`LongCarried` *cannot* score, `LongStart` must not. Implemented as
`Board::scoring_weight`, which every pass of `compute_scores` goes through.

Two things fall out of it:

- **A long game's points arrive once, when the round it finishes in completes** —
  which is exactly when every other result of that round arrives. Previously a
  game decided inside round N scored there and then vanished for the whole of the
  gap round, because its outcome had moved to a round that was not complete yet.
  There is no longer a moment at which a score goes backwards.
- **A forfeited long game is a forfeit in both of its rounds.** The forfeit rule
  says the pair never faced each other and neither floated; the inert record used
  to escape it, since `LongCarried` reads as `Pending` whatever the game really
  was, and so recorded the opponent once — neither the forfeit rule's nought nor
  the long rule's two — plus a float marker from a game nobody played.

The case this rule cannot answer is a `LongStart` that is **never carried**: the
tournament ends on the round it began in. It has consumed one round, so two
points would let its players outscore a field that played the same single round,
and zero silently drops a game the grid still renders. Neither is defensible, so
the state is refused rather than scored: `grid_export_blocker` returns
`UncarriedLongGame`, naming the round and the two ways out — play the next round,
which carries it, or untick the box, which demotes it to an ordinary one-point
game. The mid-tournament state stays perfectly legal; only the export refuses,
because that is the point at which "the tournament is over" is being asserted.

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
