# Two-round boards ("long games")

Some tournaments give the top boards **double the time control**, so those games
run for two rounds while the rest of the field plays two ordinary ones. The
referee ticks a per-board checkbox (the tournament-level setting
`long_boards_enabled` turns the column on); there is no "the top N boards are
always long" rule.

A long game is **one game played across two rounds**, and everything else follows
from taking that literally:

- its winner scores **two points and two victories**, because two rounds went by;
- for the opponent-based tie-breaks it is **two games against the same
  opponent** — the opponent appears twice in `opponents`/`defeated`, so SOS,
  SODOS, SOSOS and Buchholz weight them double;
- it feeds the **ELO estimate once**, since that reads the boards themselves;
- its two players are **not paired in the second round**, whatever the game did.

That last point is the one with teeth. A long game occupies both of its rounds
even if it was decided inside the first, or resolved by a no-show. Freeing its
players the moment it was decided let them take three wins out of two rounds
while the rest of the field could take two — a wrong final standing, not a
cosmetic oddity. The referee who decides a game really only took one round
unticks the box **before the round advances**, demoting it to an ordinary
one-point board. That is the only escape hatch, and it is deliberate.

## Why three records

A long game is stored as one [`GameRecord`](../../crates/core/src/round.rs) per
round it occupies:

```rust
enum GameRecord {
    Short(Outcome),      // an ordinary one-round game
    LongStart(Outcome),  // a long game whose second round is not paired yet
    LongCarried,         // the inert record left behind once it has been
    LongEnd(Outcome),    // the live record, in the round the game finishes in
}
```

The obvious alternative — keep the game on one board in the round it started, and
have everything else reach across rounds to find it — is what this replaced. Each
cross-round lookup produced a defect: the grid exported an unfinished game as a
loss for *both* players, back-to-back long games dropped a result entirely, the
`round − 1` lookup silently read the wrong round once an earlier result was
cleared, and the second round could close while the game was still being played.
Worst of all, the two players had **no record at all** in the second round: they
were filtered out of the pairing and never given anything in exchange, so they
were simply missing from the data.

Giving the game a record in each of its rounds makes every question local to the
round being looked at. Completion, rendering, scoring and pairing all read the
round in front of them and no other.

**The outcome lives inside the enum, not beside it.** `LongCarried` has nowhere
to put a result, so the state "both records claim a result and they disagree" is
not merely rejected but unrepresentable. That is the whole reason for a sum type
rather than a `long: bool` flag next to an `Outcome`.

**`LongStart` is not `LongEnd` waiting to happen.** It is the state before the
second round exists at all — the game is under way, but nothing has been paired
around it yet. It becomes `LongCarried` the moment the next round is confirmed.

## The carry

Confirming round N+1 **moves** the outcome: the round-N record becomes
`LongCarried`, and a `LongEnd` board holding the outcome (and the handicap, which
belongs to the game) is added to round N+1 with `PairingSource::Carried`. Exactly
one record is authoritative at every instant.

It is symmetric. `pop_round_uncarrying` moves the outcome back and reverts the
record to `LongStart`, and it is the **only** supported way to drop a round: a
bare `rounds.pop()` strands an inert `LongCarried` with nothing to make it live
again, and the game disappears, result and all. `cancel_last_round` and both
`force_pairing` paths go through it.

The carry is derived from the previous round rather than from the draft, so
re-confirming a round rebuilds it identically. This is also why a carried result
is the one result that survives `force_pairing` re-pairing its round
(`result_survives_repairing`): everything else — Swiss, cup, even the referee's
own forced boards — is rebuilt pending.

## Invariants

These are what the code is built on. Breaking one is silent, which is why several
are checked rather than assumed.

**Exactly one record holds the outcome.** The carry and the un-carry each move
it; neither copies it. Every API-facing write to an inert `LongCarried` record is
refused (`refuse_if_carried`), because the board is still visible in the round it
started in and clicking it is an ordinary mistake.

**Exactly one record scores the game** — the `LongEnd`, at double weight.
`LongCarried` cannot score, having no outcome. `LongStart` scores *nothing*
either, and that is load-bearing rather than tidy: round N+1's pairing model is
replayed from the rounds below it, and confirming N+1 rewrites the round-N record
from `LongStart` to `LongCarried`. If the two scored differently, the model that
paired the gap round could never be recomputed, and counterfactual probes of that
round would answer from a model that never paired it. See
`Board::scoring_weight`, which every pass of `compute_scores` goes through.

The visible consequence is that a long game's points arrive once, when the round
it *finishes* in completes — which is when every other result of that round
arrives too.

**Every player is accounted for exactly once in every round**: one board or one
sit-out, never both and never neither (`validate_round_membership`, also asserted
at the end of both confirm paths in debug builds). The "never neither" half is
the one this feature keeps threatening — a pairing exclusion that hands out no
board is how players vanished from a round twice.

**A long game reaches exactly one round forward.** The pairing exclusion is keyed
on `LongStart` alone — *not* on "is this board long?", which is also true of
`LongCarried` and `LongEnd` and so excluded the players from the round *after*
the game as well, where nothing was left to give them a board. It ships to
clients as `draft_long_players` rather than being re-derived there.

**A round is never held open by a record that cannot be filled in.**
`complete_for_round` treats `LongStart` and `LongCarried` as settled — the field
plays on, which is the point of the feature — while `LongEnd` is an ordinary
board that holds its round open until the result is in. No cross-round recompute,
so no never-completing deadlock.

**Halves are never orphaned.** `validate_long_games` rejects a loaded file with a
`LongCarried` and no following `LongEnd` (or the reverse), a `LongEnd` in round
1, or a `LongStart` in any round but the last.

## What blocks the American Grid export

The grid is the tournament's final record, so `grid_export_blocker` refuses two
states that are perfectly legal while it is still running:

- **A round that is not finished.** This is the general rule, and it is not
  specific to long games at all. It is stated here because the export used to
  *filter* unfinished rounds out rather than refuse them, so it succeeded and
  produced a document with a round silently missing.
- **A long game that was never carried** — a `LongStart` still sitting in the
  last round, decided or not. Not covered by the rule above: `LongStart` does not
  hold its round open, so that round is complete and the export would otherwise
  go ahead. The game has taken one round rather than two, so two points would let
  its players outscore a field that played the same single round, and the scoring
  rule gives it none. The referee plays the next round, which carries it, or
  unticks the box, which demotes it to an ordinary one-point game.

A third check, for a long game with **no result**, adds no rule to those two —
an unresolved `LongEnd` leaves its round unfinished, and an unresolved
`LongStart` is uncarried by definition. It runs first purely so the referee is
told to finish the game rather than that a round is unfinished or that a board
needs demoting, which are true but not what to do about it.

## Rendering

The two surfaces differ deliberately, and neither should be "fixed" into the
other.

The **American Grid** keeps one column per round, as that fixed format requires:
`LongCarried` renders `0-`, and the result appears in the round the game was
finished in.

The **results cross-table** in the app draws the game the way it was played: one
cell straddling both round columns, showing the result once (`crossTableColumns`
in `frontend/src/lib/crossTable.ts`). `0-` followed by a result reads as two
games, which is exactly the misreading to avoid.

## Cup interaction

A cup bracket round is long or short as a **whole** — a bracket that moved at two
speeds would desynchronise from the field — so ticking one cup board ticks them
all, and in a qualifier cup's qualification round that unit takes in the
pre-qualified players' games in the open too. The flip is refused once *any* game
in the unit has a result, not just the board clicked. Gap-round boards carry
`PairingSource::Carried`, not `Cup`, so the bracket schedule is undisturbed.

Team mode has no interaction at all: the two settings are mutually exclusive
(`TeamModeConflict::LongGames`), so a team tournament never has a long board
to reason about.
