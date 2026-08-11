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

- **Weak club protection** Intermediate between normal and exempt.

- **Team tournaments** — design settled in
  [docs/team-tournaments.md](docs/team-tournaments.md): teams as the pairing
  unit, boards as the atom, derived match outcomes. Landed so far: the
  board-outcome sum type, the engine's unit abstraction, the team data model
  (settings, rosters, finalization) and team pairing with its score replay — a
  team tournament can be configured, rostered, finalized and played. Next: the
  justified/unjustified absence distinction (a partly absent team is refused
  until then), team-level draft operations, standings and tie-breaks, team
  point adjustments (per-player ones are refused in team mode meanwhile), the
  team CRUD routes and the UI. Team standings and the `BoardWins` tie-break have
  landed.

## Multi-referee server

- **OAuth authentication**

- **Log of which user took which action**

- **Read-only access to the pairings and standings**

- **Keep one backup on tournament deletion for a while** 1 month before losing a tournament backup sounds good

## ELO estimator

- **Glicko / Glicko v2**, they're advanced variants of ELO, look at using them once we have the full games data.

- **Add logistic prior**, should be quite similar to Laplace one, but not exactly

## Other

- **Webhook for pushing results and pulling players** See https://github.com/ffrgo/pairgoth/blob/master/doc/reference.md#pairgoth-webhook-specification

- **Make the Tauri desktop app's data directory configurable.** It's currently
  hardcoded to `dirs::data_dir()/openshogipairings/tournaments/`
  (`frontend/src-tauri/src/lib.rs`'s `local_data_dir`), matching where
  `backup.rs` already puts its automatic backups. Some users may want it
  redirected (a synced folder, a different drive, a portable-install layout)
  — read it from an environment variable, falling back to the current default
  when unset.

- **More complete american grid exporter** There should be fields in the settings with the dates
  and time control of the tournament, so they can be reported in the header of the american grid;

- **Reorganize the settings tab**. Some combinations make little sense (e.g. pure-ELO mode with any kind of cup)

- **Put ongoing games in the standings tab** Inspired by the WOSC website: just put them with neither + nor - (and greyed) until the game is decided.

- **Tooltips of player names in standings tab** should lose the "contre" word, and gain the ELO of that player in parentheses after their name.

- Check all instances of `#[allow(clippy::too_many_arguments)]`
