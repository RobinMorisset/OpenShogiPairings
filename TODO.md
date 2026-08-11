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

- **Nationality protection** Same idea as club protection, just a hair weaker.

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

- **Team tournaments** — design settled in
  [docs/team-tournaments.md](docs/team-tournaments.md): teams as the pairing
  unit, boards as the atom, derived match outcomes. First step is the
  preliminary board-outcome sum-type refactor (own commit), then the team
  mode itself.

## Multi-referee server

- **OAuth authentication**

- **Log of which user took which action**

- **Read-only access to the pairings and standings** — design settled in
  [docs/public-access.md](docs/public-access.md): a `PublicTournamentView`
  projection (`TournamentView` minus the draft and the referee-only fields),
  never showing a draft round but publishing each result as it lands, served
  by its own unauthenticated router group. Three phases, each useful alone:
  a public endpoint on the hosted server (capability URL + read-only frontend
  mode), a static HTML export of the same projection for the desktop app, then
  pushing it to a club's own site.

- **Keep one backup on tournament deletion for a while** 1 month before losing a tournament backup sounds good

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

- **Reorganize the settings tab**. Some combinations make little sense (e.g. pure-ELO mode with any kind of cup)

- Check all instances of `#[allow(clippy::too_many_arguments)]`
