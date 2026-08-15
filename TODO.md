# TODO

Known limitations and future work, roughly ordered by area.

## Pairing

- **ILP/CP-SAT backend.** Pairing is now a real **minimum-weight perfect
  matching** (integer blossom in the standalone [`integer-blossom`](crates/matching/src/lib.rs)
  crate) over a rule-weighted graph ([`crates/core/src/pairing.rs`](crates/core/src/pairing.rs)):
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
  [docs/public-access.md](docs/public-access.md). Phases 1 (the live
  capability-keyed page) and 2 (the static HTML export) have landed, and
  between them cover both deployments. What is left is for a club that wants
  the standings *inside* their own pages rather than on a separate one: POST
  the same projection, carrying `{boot_id, version}` so the receiver can order
  it across a server restart, on every change. Depends on having a receiver to
  talk to, so it waits for a club that wants it.

## ELO estimator

- **Glicko / Glicko v2**, they're advanced variants of ELO, look at using them once we have the full games data.

- **Add logistic prior**, should be quite similar to Laplace one, but not exactly

## Other

- **Pull the players list from a HelloAsso billetterie** Start from the back-office
  CSV export mapped onto the existing `POST /players/import-csv`, not the API: no
  secrets to store and no new dependency. The hard parts are identity, not format —
  the registrant is not always the player, the FESA name/club/grade live in
  free-text custom fields, so the import wants a review screen matching against the
  rating list rather than a silent bulk insert; a re-import (registrations trickle
  in) needs the HelloAsso participant id kept as an external key to dedup against.
  Do not import email addresses (see the non-goals in
  [docs/public-access.md](docs/public-access.md)).

- Show **predicted elo change** as the rightmost column of the Standings tab.

- **Pre-fill the earlier rounds on the result sheets**: printing them mid-tournament
  (for a late arrival, or to replace a lost slip) currently leaves rounds already
  played blank, so they have to be copied over by hand.

- The difference between HalfPoints and Wins is no longer important since b69d35e, think about
  getting rid of the Wins unit.

- At the bottom of the screen, rather than just the version number, maybe also a commit hash if it does not match the version number's tag ?

- More cost details in Question this pairing

- Reuse the team roster UI to pick forced pairings/byes

- Better invalidation for the Why these pairings at the top of each round's pairings: not really a prefix, instead should be a bitfield, so starting a new round always gives a valid result for this new round. Also a bunch of changes should not affect it, e.g. altering the tiebreak criteria.