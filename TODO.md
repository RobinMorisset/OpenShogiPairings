# TODO

Known limitations and future work, roughly ordered by area.

## Pairing

- **ILP/CP-SAT backend.** Pairing is now a real **minimum-weight perfect
  matching** (integer blossom in the standalone [`integer-blossom`](crates/matching/src/lib.rs)
  crate) over a rule-weighted graph ([`crates/core/src/pairing`](crates/core/src/pairing)):
  rematch/repeat-bye ≫ bye-group² ≫ score gap² ≫ float-repeat ≫ floater-selection ≫
  same-club ≫ within-group fold, ordered by a scalar multiplier ladder whose tiers are derived
  from each rule's worst-case contribution (so the lexicographic separation is
  exact, not hand-tuned). An ILP/CP-SAT backend is still motivated by experimental
  formats (MacMahon-beyond, hard multi-round constraints) that need constraints a
  plain matching can't express, and by very large fields. Plan: `good_lp` + HiGHS
  first, then CP-SAT.

- "force_pairing on a past round reads the current player list, so re-pairing a round that a late joiner missed would now pair them into it.", found by the long-boards redesign session.

## Multi-referee server

- **OAuth authentication**

- **Log of which user took which action**

- **Push the public projection to a URL the club configures** — phase 3 of
  [docs/archive/public-access.md](docs/archive/public-access.md). Phases 1 (the live
  capability-keyed page) and 2 (the static HTML export) have landed, and
  between them cover both deployments. What is left is for a club that wants
  the standings *inside* their own pages rather than on a separate one: POST
  the same projection, carrying `{boot_id, version}` so the receiver can order
  it across a server restart, on every change. Depends on having a receiver to
  talk to, so it waits for a club that wants it.

## ELO estimator

- **Glicko / Glicko v2**, they're advanced variants of ELO, look at using them once we have the full games data.

- **Add logistic prior**, should be quite similar to Laplace one, but not exactly

## Documentation

- **Fold the archived design docs into `docs/reference/`.** Each was written
  before its feature shipped and has drifted since; see
  [docs/README.md](docs/README.md) for what the four directories mean. The work,
  per document: mine the parts still true and the rationale worth keeping into a
  maintained reference doc — its own file under `docs/reference/`, split off
  from [architecture.md](docs/reference/architecture.md) where that section has
  outgrown it — then delete the archived one. Remaining:
  [elo-pairing-mode.md](docs/archive/elo-pairing-mode.md),
  [multi-referee-internet.md](docs/archive/multi-referee-internet.md),
  [multi-tournament.md](docs/archive/multi-tournament.md),
  [pairing-explanations.md](docs/archive/pairing-explanations.md),
  [public-access.md](docs/archive/public-access.md),
  [team-tournaments.md](docs/archive/team-tournaments.md),
  [two-round-boards.md](docs/archive/two-round-boards.md).
  `public-access.md` is the one exception to "then delete": the phase-3 entry
  under Multi-referee server still cites it as that phase's specification, and
  the HelloAsso entry cites its non-goals — so it has to outlive the others, or
  hand that design over first.

## Other

- **Pull the players list from a HelloAsso billetterie** Start from the back-office
  CSV export mapped onto the existing `POST /players/import-csv`, not the API: no
  secrets to store and no new dependency. The hard parts are identity, not format —
  the registrant is not always the player, the FESA name/club/grade live in
  free-text custom fields, so the import wants a review screen matching against the
  rating list rather than a silent bulk insert; a re-import (registrations trickle
  in) needs the HelloAsso participant id kept as an external key to dedup against.
  Do not import email addresses (see the non-goals in
  [docs/archive/public-access.md](docs/archive/public-access.md)).

- Show **predicted elo change** as the rightmost column of the Standings tab.

- **Pre-fill the earlier rounds on the result sheets**: printing them mid-tournament
  (for a late arrival, or to replace a lost slip) currently leaves rounds already
  played blank, so they have to be copied over by hand.

- The difference between HalfPoints and Wins is no longer important since b69d35e, think about
  getting rid of the Wins unit.

- At the bottom of the screen, rather than just the version number, maybe also a commit hash if it does not match the version number's tag ?

- More cost details in Question this pairing

- Reuse the team roster UI to pick forced pairings/byes

- Finer invalidation for the Why these pairings at the top of each round's pairings: it is now a flag per round, so a new round is always valid, but a settings change still invalidates every round wholesale. A bunch of changes should not affect it at all, e.g. altering the tiebreak criteria.