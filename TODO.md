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

- **Webhook for pushing results** — phase 3 of
  [docs/public-access.md](docs/public-access.md) (push the public projection
  whole, with a `{boot_id, version}` ordering key, never deltas). Reference:
  https://github.com/ffrgo/pairgoth/blob/master/doc/reference.md#pairgoth-webhook-specification

- **Pull the players list from a HelloAsso billetterie** Start from the back-office
  CSV export mapped onto the existing `POST /players/import-csv`, not the API: no
  secrets to store and no new dependency. The hard parts are identity, not format —
  the registrant is not always the player, the FESA name/club/grade live in
  free-text custom fields, so the import wants a review screen matching against the
  rating list rather than a silent bulk insert; a re-import (registrations trickle
  in) needs the HelloAsso participant id kept as an external key to dedup against.
  Do not import email addresses (see the non-goals in
  [docs/public-access.md](docs/public-access.md)).

- **Keyboard shortcut to undo** is Ctrl+Z even on Mac, maybe Command+Z would be better ?

- Show **predicted elo change** as the rightmost column of the Standings tab.

- **Pre-fill the earlier rounds on the result sheets**: printing them mid-tournament
  (for a late arrival, or to replace a lost slip) currently leaves rounds already
  played blank, so they have to be copied over by hand.

- **Cleanup the Changelog** (tons of redundant entries were made during the team tournament sequence of commits)