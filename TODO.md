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

- **Freeze the round explanation at confirmation.** `explain_round` rebuilds the
  `PairingModel` from `rounds[..idx]`, so correcting an earlier round's result, or
  editing a rating, club or pairing setting, silently changes the reported
  rationale of later rounds — while the ledger still looks plausible. Store the
  `RoundExplanation` on the `Round` when it is confirmed (a record of a past event,
  like the frozen `sitouts` scores, not a cache), bumping
  `TOURNAMENT_FORMAT_VERSION` 5 → 6. Plus an `explanations_faithful_through`
  watermark on the `Tournament` — decreasing only, `min(mark, k)` on a result edit
  in round `k` and `0` on a player/settings edit — so rounds above it are shown
  with a "the data behind this has changed since" warning. Design in the appendix
  of [docs/public-access.md](docs/public-access.md).

## Multi-referee server

- **OAuth authentication**

- **Log of which user took which action**

- **Static HTML export of the public page** — phase 2 of
  [docs/public-access.md](docs/public-access.md). Phase 1 (the
  `PublicTournamentView` projection, the capability-keyed public endpoint and
  its payload-carrying stream, the read-only frontend mode) has landed, and
  serves the hosted deployment. It does nothing for the desktop app, whose
  embedded server listens on a random loopback port — hence this: the same
  projection written as a self-contained HTML file (data inlined, no server),
  regenerated on every change, for the referee to upload wherever the club
  already has a website.

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

- Check all instances of `#[allow(clippy::too_many_arguments)]`

- **Keyboard shortcut to undo** is Ctrl+Z even on Mac, maybe Command+Z would be better ?

- Show **predicted elo change** as the rightmost column of the Standings tab.

- Print per-player small **result reporting sheets**

- **Cleanup the Changelog** (tons of redundant entries were made during the team tournament sequence of commits)

- Better UI for standings tab when it gets wider than the screen.