# Team tournaments — design

Status: **draft, under discussion**. Landed so far: the [preliminary board-outcome
refactor](#preliminary-refactor-board-outcome-as-a-sum-type) and the [engine's unit
abstraction](#engine-refactor-the-unit-abstraction) — both behaviour-preserving
groundwork. Nothing team-specific is implemented yet.

A team tournament is the format that traditionally precedes European shogi
championships (e.g. WOSC): teams of N players (usually N = 3) are the unit of
pairing and ranking, while the games themselves remain ordinary individual
boards.

## Glossary

- **Team** — an ordered list of exactly N registered players. The order is the
  **board order** (board 1 = strongest, by convention descending ELO).
- **Team match** — the pairing of two teams in a round. It consists of N
  ordinary boards: board k of team A plays board k of team B.
- **Board** — a single game between two players, exactly the existing
  [`Board`](../crates/core/src/round.rs) struct. Boards stay the atom of the
  whole system.
- **Match points** — what a team scores from a match: a win (1 point) if its
  board wins strictly exceed the opponent's, half a point each if equal, a loss
  (0) otherwise. Note the tie case is reachable even with odd N (a board
  with neither side present decides no winner), so the rule is defined for
  all N.
- **Board wins** — the total number of games won by a team's players; the new
  team tiebreak.

## Scope of v1

Supported: Swiss and MacMahon pairing at team level (MacMahon thresholds by
team average ELO only), handicaps, per-board no-shows and forfeits, team byes,
team absences, team standings with per-player breakdown, American grid export,
per-player ELO estimation, pairing explanations and counterfactuals (at team
level).

Explicitly **out of scope for v1** (rejected loudly, not silently ignored):

| Feature | In team mode |
|---|---|
| Cup (direct elimination) | rejected in settings validation |
| Long games (two-round boards) | rejected in settings validation |
| ELO pairing mode (`PairingMode::Elo`) | rejected in settings validation |
| Grade-based MacMahon thresholds | rejected in settings validation |
| `EstElo` tiebreak | rejected in settings validation |
| Player categories | hidden/disabled in team mode |
| Substitutes / per-round lineups | not planned (unused in European shogi) |
| Simulation (`crates/sim`, `core/src/sim.rs`) | returns an explicit "team tournaments unsupported" error in v1; support wanted eventually |

"Rejected in settings validation" means: enabling team mode with any of these
active — or activating one of them while team mode is on — is a settings
error naming the conflict, per the fail-loud policy. Nothing is silently
auto-disabled.

## Design philosophy

Two principles, both inherited from the cup implementation:

1. **`Board` stays the atom.** A team match is stored as N ordinary boards.
   Results are entered per board exactly as today. The American grid export,
   ELO estimation, no-show handling, handicap machinery and the cross-table
   walk `Round.boards` and keep working untouched.
2. **Derive, never store, team-level outcomes.** Match results, team scores,
   team standings, and the board↔match grouping are all recomputed by replay
   from boards + rosters, the same way the cup replays its bracket and scores
   are recomputed live. Editing a past board result automatically re-derives
   every dependent team outcome. The only stored team state is the roster
   itself (and its frozen board order).

The grouping needs no storage at all: every board's two players belong to
known teams, so the match a board belongs to — and its board number, which is
the player's index in the roster — is fully derivable, on both backend and
frontend.

## Preliminary refactor: board outcome as a sum type

**Landed** (own commit, before any team work). `Board` used to carry
`result: Option<Winner>` and `no_show: Option<NoShow>` as sibling fields, so
"a result recorded on a forfeited board" was representable and only excluded
by convention (mutations kept `result` at `None` on forfeits so ELO never
saw them). Team mode adds a third concern to that product (justified
absence), so it was folded into a single sum first:

```rust
/// What happened on a board.
pub enum Outcome {
    /// No decision yet. `drawn` records at least one draw (sennichite)
    /// before the decisive replay still in progress — it matters for ELO.
    Pending { drawn: bool },
    /// Played to a decision (the former `result: Some(w)`), possibly after
    /// draws.
    Won { winner: Winner, drawn: bool },
    /// Side(s) failed to appear — no game was played; the present side (if
    /// exactly one) is credited the point like a bye; never feeds ELO
    /// (the former `no_show`).
    Forfeit { absent: NoShow },
}

/// On Board, replacing `result` + `no_show` + `drawn`:
pub outcome: Outcome,
```

`Forfeit` carries a named field rather than the tuple this doc first
sketched, so the whole sum serializes internally-tagged like
[`PairingSource`] and reaches the frontend as a plain discriminated union on
`kind`. The field is skipped in JSON while the board is pending, so an
unplayed board's save shape is unchanged.

`drawn` lives only in the variants where play happened: the American grid
cannot express "forfeit after draws", so the type doesn't either. This also
absorbs the third loose field — `result` × `no_show` × `drawn` all collapse
into the one sum.

One consequence is not mechanical: `set_board_drawn` on a forfeited board
has nothing to write, so it now **fails loudly**
(`TournamentError::DrawnOnForfeitedBoard`) and the round view disables the
draw button there. That combination used to be reachable and was a real bug
— the ELO reader tested `drawn` before the result and scored the phantom
game ½.

Touch points (mechanical): `is_decided` / `effective_winner` /
`effective_loser` / `winner_id`, the result-entry mutations
(`toggle_board_winner`, `set_board_no_show`, `set_board_drawn`), scoring,
the American grid cells, the ELO reader, sim auto-fill, and the frontend
result-entry + `boardOutcome`/`noShow` helpers with regenerated TS types.
No legacy-save support (see Persistence); `format_version` is now 6.

## Data model

### `Team`

```rust
/// A registered team. Stored on `Tournament`.
pub struct Team {
    /// Stable identity, like `Player::id`.
    pub id: Uuid,
    /// Dense 1-based team number, assigned at finalization by descending
    /// average pairing rating (mirroring player numbering). The key the
    /// team score tables are indexed by.
    pub tournament_id: Option<TeamId>,
    /// Non-empty, unique among teams (case-insensitive).
    pub name: String,
    /// Exactly `settings.teams.size` members once finalized, in board order
    /// (index 0 = board 1). Referenced by player `Uuid`, since players lack
    /// a `TournamentId` before finalization.
    pub members: Vec<Uuid>,
}
```

`TeamId` is a new dense-id newtype in `units.rs`, exactly parallel to
`TournamentId` (bare-number serde, `TiVec`-indexable, `TS`-exported). Player
`TournamentId`s are still assigned at finalization as today — boards, the
grid, and per-player scoring all keep using them.

### Settings

Team mode is **orthogonal to `PairingMode`**, like the cup — not a third mode
variant. Since ELO pairing mode is rejected in team mode, the mode selector UI
does not need restructuring; team mode is a toggle plus a size field.

```rust
/// In TournamentSettings:
#[serde(default, skip_serializing_if = "Option::is_none")]
pub teams: Option<TeamSettings>,

pub struct TeamSettings {
    /// Players per team. Validated 2 ..= 9, default 3.
    pub size: u32,
}
```

Changing `size` (or toggling team mode) after finalization is rejected, like
other structural settings.

### Pairing-only rating for unrated players (MacMahon only)

ELO-based MacMahon starting points need every member to contribute to the
team average ELO. The established referee practice is to assign an unrated
player a fake ELO *for pairing purposes only*. To keep that from polluting
exports (the grid's `N` flag and rating column must stay honest):

```rust
/// On Player: referee-assigned rating used only for pairing-time
/// computations (team average, fold order). Never exported; the player
/// remains "unrated" everywhere user-facing. Only meaningful (and only
/// settable) in team mode with MacMahon starting points in use.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub pairing_rating: Option<u32>,
```

A player's **pairing rating** is `rating.or(pairing_rating)`. The field —
and its validation — exist only when MacMahon starting points are in use:

- **With MacMahon**: finalization fails loudly if any team member has
  neither a rating nor a pairing rating; the registration UI shows a
  distinct "pairing ELO" field for unrated members, visually marked as
  unofficial.
- **Without MacMahon** (plain Swiss): unrated members are tolerated exactly
  as unrated players are in individual mode — no fake ELO is asked for. A
  team's average is taken over its rated members (`None` if it has none),
  feeding only the soft uses: fold order, `TeamId` numbering, and the
  default board order (teams/members without ratings sort last, as unrated
  players do today).

### Point adjustments

Manual point adjustments apply to **teams** in team mode (they affect team
points, hence pairing and standings). Per-player adjustments are disabled in
team mode — a player-level delta has no team meaning.

## Registration and finalization

Registration is **players first, then grouping**: players are registered
individually exactly as today (same form, same CSV import — an optional
`Team` CSV column can come later), and teams are a grouping layered on top.

### Teams panel (Players tab, team mode only)

Alongside the player list, a Teams panel:

- **create / rename / delete** a team (name required non-empty, unique
  case-insensitive);
- each team card shows its members **in board order** with their pairing
  ratings and the **team average**, plus a live size indicator
  (`2/3` etc.) so incomplete teams are visible at a glance;
- an **unassigned players** pool; players move between the pool and a team
  (a per-player team picker or equivalent — exact affordance decided at
  implementation);
- **board order** defaults to descending pairing rating and is freely
  reorderable until finalization;
- with MacMahon starting points in use, unrated members are flagged
  inline with the "pairing ELO" field (see above).

### Finalization

Validates, loudly:

- every player belongs to exactly one team (no unassigned pool);
- every team has exactly `size` members (incomplete teams are an error —
  no ghost boards);
- with MacMahon starting points: every member has a pairing rating;
- at least 2 teams.

Then assigns `TeamId`s (descending team average pairing rating) and
**freezes rosters and board order**: later rating edits must not reshuffle
who plays board 1 (same principle as frozen cup eligibility and frozen
handicap givers).

### No late registration in team mode (v1)

After finalization, team mode rejects **all** registration — individuals
*and* teams. This is forced by the players-first model: an individual late
joiner would be teamless, and a late team would require its players to be
registered teamless first, both violating the frozen "every player is in
exactly one team" invariant. Rather than special-casing, v1 rejects with a
clear error. If a real event ever needs it, the future answer is an
**atomic add-full-team operation** (one mutation creating the N players and
the complete roster together, validated as a unit, assigned the next
`TeamId`) — documented here so the door stays visibly open, but out of
scope for v1.

## Pairing

### Engine refactor: the unit abstraction

**Landed** (own commit, no behavioural change — the simulator's output over
WOSC 2024 is byte-identical before and after). The matching engine
(`pair_round_weighted`) already operated on opaque dense keys plus derived
per-unit data; it never read `Player` fields except rating and club. The
refactor extracted that dependency into an explicit input:

```rust
/// What the engine needs to know about one pairable unit, whoever it is.
pub(crate) struct PairingUnit {
    pub points: HalfPoints,
    pub macmahon: HalfPoints,
    pub opponents: Vec<UnitKey>,     // past opposing units
    pub had_bye: bool,
    pub last_ascended: Option<u32>,
    pub last_descended: Option<u32>,
    pub rating: Option<u32>,         // fold order (team: average pairing rating)
    pub clubs: Vec<Option<String>>,  // member clubs in board order (player: one entry)
    pub prequalified: bool,          // cup, individual mode only
    pub elo: i64,                    // ELO pairing mode only
}
```

The last two fields are not in the original sketch: the cup's pre-qualified
flag and the live ELO estimate are *also* per-unit facts the rules read, so
they belong on the unit rather than in a second side table. Both are inert in
team mode, which rejects the cup and ELO pairing outright.

`pair_round_weighted` is now a function of `&TiSlice<UnitKey, PairingUnit>`
(plus settings, forced pairs, forced byes) returning `UnitPairing` — matched
key pairs, each with the `points_diff` its board(s) must freeze, and at most
one bye key. Building the `Round` (one board per pair, or `size` boards for a
team match) is the caller's job. `player_units` is the individual wrapper;
the team path will add `team_units`. The blossom solver is untouched.

`UnitKey` is a new dense-key newtype in `units.rs`, numbered from 1 so `0`
stays free for the phantom (bye) vertex. The pairing *explanations*
(`BoardLedger`, `AffectedCycle`) now reference `UnitKey` rather than
`TournamentId` — same bare-number wire shape, read as player numbers in
individual mode and team numbers in team mode.

All the rules generalized unchanged: rematch avoidance, bye-group, score gap,
float repeat, floater selection, fold — they only read `PairingUnit` fields.

The **club rule** became a graded count: an edge's club cost is the number
of board positions k where both units' board-k clubs are `Some` and equal —
i.e. the number of same-club *games* the pairing would actually create
(aligned positions only; a same-club pair on different boards never plays
each other and costs nothing). Within the club rule's ladder tier the
matching therefore minimizes the total same-club games of the round. For the
player wrapper (`clubs` of length 1) this degenerates to exactly the previous
0/1 behavior, so one rule implementation serves both modes. There is no "team
club" concept — mixed-club teams are handled naturally by the count.

**Ladder bounds.** The rule multipliers are derived from per-rule worst-case
contributions (`max_total_units`). These bounds must be recomputed from the
*team* instance parameters (team count, team points range including MacMahon
starts and adjustments, and the club rule's per-edge maximum growing from 1
to N) — any bound that silently assumes player-scale quantities breaks the
exact lexicographic separation, the worst possible failure mode. Every bound
already reads its quantity off the instance, and the club rule's per-edge
ceiling is now `max_boards` (the widest `clubs` among the free units) instead
of a hard-coded 1. The existing `ladder_overflow` guard stays; a debug
assertion should cross-check bounds against the actual instance.

### From matched teams to boards

`confirm_round_inner`, team path:

1. Compute team scores; run the engine over present teams.
2. Each matched team pair expands to N `Board`s: roster[k] of A vs roster[k]
   of B, `source: PairingSource::Swiss` (or `Forced`), and `points_diff`
   frozen to the **team** points difference, duplicated on every board of the
   match (it is a fact of the team pairing; float history replays from it).
3. The engine's bye team expands to N `Sitout`s, one per member, `kind: Bye`
   (see below).
4. Boards are sorted for display by team match (by best team rank), then
   board number within the match.

### Draft operations are team-level

In team mode, `RoundDraft` semantics shift to teams; player-level forced
pairings and byes are rejected by validation:

- **Team absence**: a team all of whose members are marked absent is excluded
  from pairing entirely; each member gets an `Absent` sitout.
- **Individual absence**: marking fewer than N members of a team absent does
  *not* exclude the team — it is paired normally, and each absent member's
  board is created pre-marked with a justified absence (see below), which the
  opponent wins by forfeit. This is how a player who leaves the tournament
  (illness, travel) is recorded without the unjustified-no-show stigma.
- **Forced pairings**: force team A vs team B (expands to N boards).
- **Forced byes**: force a team bye.
- `MIN_PRESENT_PLAYERS` becomes "at least 2 teams present".

`force_pairing`, the probe/counterfactual API, and pairing explanations all
operate on `TeamId`s in team mode; the explanation machinery is engine-level
and carries over with the unit abstraction.

## Results

Entered per board, exactly as today: winner toggle, `drawn` flag, handicap,
no-shows.

### Justified absence: the team-mode extension of `Forfeit`

In individual mode a justified absence never reaches a board — the player is
excluded from pairing and gets a sitout (`0-`/`0=`). In team mode the team
plays anyway, so the absent member's board structurally exists, and
recording the absence as a plain no-show would stamp the unjustified `0#`
cell on (for example) a player who fell ill.

Building on the preliminary outcome refactor, the team commit enriches the
`Forfeit` payload with *why* each missing side missed — every state is
meaningful by construction (at least one side missing, each missing side
has exactly one kind, no result possible on a forfeit):

```rust
/// Why a missing side missed the board.
pub enum AbsenceKind {
    /// Failed to appear, unjustified — the `0#` cell.
    NoShow,
    /// Absent for a valid reason (illness, departure) — the `0-` cell.
    /// Never occurs outside team mode (there, an absence excludes the
    /// player before a board exists) — a load-time invariant.
    Justified,
}

/// Replaces the `NoShow` payload of `Outcome::Forfeit` from the
/// preliminary refactor:
pub enum Forfeit {
    Player1(AbsenceKind),
    Player2(AbsenceKind),
    Both(AbsenceKind, AbsenceKind),
}
```

There is **no score value attached**: in team mode the `0-`/`0=`/`0+`
semantics live at team level, and individual points are meaningless (the
per-player breakdown shows win counts). Both kinds score the missing player
nothing; the two differ only in the exported cell and the honest record:

| missing side's kind | their grid cell | present opponent |
|---|---|---|
| `NoShow` | `0#` | `0+`, board win |
| `Justified` | `0-` | `0+`, board win |

For the **match derivation** the kinds are identical: a forfeit with exactly
one present side is a board win for that side; `Both` has no winner. Neither
ever feeds ELO. Set from the draft (a pre-marked individual absence) or on
the board after pairing.

### Derived team match outcome

For a match, count each side's **board wins** = boards whose effective winner
(Wiel rule included) is that side's player, plus forfeit wins where only that
side's player was `Present`. Then:

- strictly more board wins → match win (2 half-points), fewer → loss (0);
- equal → half a point each. This covers the even-N case *and* the odd-N
  case degraded by a board with no `Present` side (no winner there).

A match with undecided boards has no outcome yet; the round completes on the
same per-board rules as today.

### Absences and no-shows within a team

- **One player missing, team present**: the team plays; the missing player's
  board becomes a `Forfeit` with kind `NoShow` (didn't appear) or `Justified`
  — set in the draft or after pairing. Boards do **not** shift up — a missing
  board-2 player forfeits board 2; board 3 still plays board 3. The forfeit
  counts as a board win for the opponent in the match outcome either way.
- **Whole team missing**: team absence in the draft (before pairing), or — if
  discovered after pairing — forfeiting all N boards, which makes the match a
  derived loss.

### Team bye

A team bye is stored as N member `Sitout`s (`kind: Bye`, value `Full`), so
per-player scoring and the grid see ordinary `0+` cells. At team level a bye
round derives as a **match win** (the analogue of today's full-point player
bye). The referee edits the bye's value **at team level** (win / half /
nothing), which sets all member sitouts uniformly; per-player sitout-value
editing is disabled in team mode so the derivation can never see a mixed
team. Bye-repeat avoidance operates on the team via `had_bye`.

Round helpers whose invariants assume one bye player (`swiss_bye`) generalize
to "the bye team" (N sitouts, one team) in team mode.

## Scoring and standings

### Team scores (`compute_team_scores`)

A replay over rounds producing `TiVec<TeamId, TeamScore>`, mirroring
`PlayerScore`:

- `points` — MacMahon start (threshold criterion applied to team average
  pairing ELO) + team adjustments + match points from every decided match +
  team bye/absence values;
- `victories` — count of match *wins* (halves excluded), feeding the
  W-family tiebreaks;
- `board_wins: Wins` — total games won by members (the new tiebreak);
- `opponents`, `defeated`, `had_bye`, float history, running columns — as for
  players, at match granularity.

Per-player scoring (`compute_scores`) **keeps running unchanged** on the same
boards — it feeds the American grid, the per-player breakdown, and ELO
estimation. In team mode players simply get no MacMahon start and no
adjustments, so their points are their game results and sitouts.

### Tiebreaks

The existing `Tiebreak` enum generalizes with team semantics (SOS/SODOS/
SOSOS/CUSS over opposing *teams'* team scores; direct confrontation over team
matches). Additions and restrictions:

- new variant `BoardWins` — total member game wins;
- `EstElo` rejected in team mode (settings validation);
- default team tiebreak order: match points, then `BoardWins`, then the SOS
  family as today — matching established team-event practice (match points,
  board points, SOS).

`TeamStanding` is a new TS-exported DTO parallel to `Standing` (keyed by team
`Uuid`, carrying the tiebreak columns plus `board_wins` and per-member
references). The frontend's hand-maintained `TIEBREAKS` table in
`types.ts` gains the `BoardWins` row.

### Standings display

Primary table: teams, ranked by the team tiebreak chain — same columns and
interaction style as today's standings. Each team row expands to (or is
followed by) its N member rows showing board number, name, individual wins,
and per-round cells. The cross-table shows opposing team numbers per round
with the match result. (Inspiration: shogideutschland WOSC team results
page; consistency with our existing standings wins over matching it exactly.)

Per-player ordering, where a flat player list is needed (the grid): team
rank, then board number — no cross-team player ranking is invented, since
players only ever meet same-board opponents.

## Exports and imports

- **American grid: unchanged, by design.** Purely player-based; boards are
  ordinary games, forfeits and byes render as today (`0#`, `0+`). Player
  order = team rank then board number. Fake pairing ratings are **not**
  exported — unrated players keep the `N` flag.
- **Grid import / FESA results**: import remains individual-only; a team
  tournament round-tripped through the grid loses team structure (accepted).
- **ELO estimation**: unchanged — it reads individual boards, and team
  context is irrelevant to it.

## Server

The store stays dumb. Additions:

- routes: team CRUD (create/edit/delete team, assign/remove member, reorder
  boards, rename), team-level draft operations, team adjustments — each a
  one-line delegation to a `Tournament` method, like everything else;
- `TournamentView` gains `team_standings: Option<Vec<TeamStanding>>`
  (`None` outside team mode);
- `Tournament` gains `teams: Vec<Team>` (empty and absent from JSON outside
  team mode).

SSE live updates are version-number broadcasts and need nothing.

## Persistence

**No backwards compatibility** — the project has no users yet, so neither
the preliminary refactor nor the team commit carries any migration or
legacy-wire code. `format_version` is bumped by each commit that changes the
save shape, purely so that a stale file is rejected loudly (via the existing
version guard) instead of half-parsed.

## Frontend summary

- **Settings**: "Team tournament" toggle + size field (no mode-selector
  restructure); incompatible features (cup, long games, ELO mode, categories,
  EstElo tiebreak, grade thresholds) surfaced as validation errors when
  conflicting, with the conflicting controls disabled while team mode is on.
- **Players tab**: teams panel (creation, member assignment, board reorder,
  team average ELO, unassigned pool), pairing-ELO entry for unrated members
  (MacMahon only); finalization errors listed loudly.
- **Draft**: team-level absence/forced-match/forced-bye controls; present-team
  count gate.
- **Round view**: boards grouped by team match with a match header (team
  names, running match score); per-board result entry unchanged;
  `pairingSource` badges unchanged (Swiss/Forced still apply, now at match
  granularity).
- **Standings**: team table + expandable member rows + team cross-table.
- **i18n**: every new string in all 9 locales (test-enforced).

## Testing

- Preliminary refactor commit (done): outcome serde round-trips, the draw
  flag surviving a winner toggle, and the forfeited-board draw rejection;
  behavioral no-change pinned by the existing suites;
- Core: team analogues of the pairing/scoring/standings test suites; ladder
  bound cross-checks; derivation edge cases (forfeit-tie with odd N,
  forfeit boards, team bye values, mixed forfeit kinds);
- serde round-trips + `export_bindings` regeneration for the new DTOs;
- server integration tests for team routes and validation errors;
- frontend vitest for the derived grouping and match-outcome helpers;
- sim: a test pinning the explicit "unsupported" error.

## Settled decisions

All previously open questions are resolved (discussion of 2026-08-10):

1. **Pairing-only ELO** — separate `pairing_rating` field, shown and
   validated only when MacMahon starting points are in use.
2. **Adjustments** — team-level in team mode; player adjustments disabled.
3. **Team bye value** — edited at team level only, propagated uniformly to
   member sitouts.
4. **Late registration** — rejected entirely in team mode for v1 (see
   Registration); atomic add-full-team noted as future work.
5. **`BoardWins` default position** — immediately after match points.
6. **Team sizes** — 2..=9, default 3.
7. **Club rule** — graded per-edge count of same-club games created.
8. **Board outcome sum type** — preliminary refactor, own commit, landed
   first; no backwards compatibility anywhere (no users yet).
