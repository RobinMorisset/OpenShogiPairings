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

- **Team tournaments**, basically a kind of very strong club protection

## Multi-referee server

- **OAuth authentication**

- **Log of which user took which action**

- **Read-only access to the pairings and standings**

- **Keep one backup on tournament deletion for a while** 1 month before losing a tournament backup sounds good

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