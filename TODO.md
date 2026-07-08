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
- **No-shows.** A player who was paired for a round but did not show up (distinct
  from a game simply not yet recorded) should appear as `0#` in the results
  table. This needs a way to mark a board as a no-show (a new board-result state,
  giving the opponent the win); not handled yet. Byes (`0+`, win) and absences
  (`0-`, loss) are already handled.
- **Weak club protection** Intermediate between normal and exempt.

## Frontend

- **Add a button to load a CSV of player names in the players tab**

- **Show 8th, 16th, 32nd, etc.. eligible players** when cup mode is enabled. I'm not sure whether this should live in Settings or Players.
  Probably the latter, something next to the eligibility checkbox ?

- **Replace the padlock icon by an open one for unlocked tournaments**

## Multi-tournament server

Support several tournaments on one running server (hosted or Tauri-embedded),
with clients picking which to connect to. Design:
[`docs/multi-tournament.md`](docs/multi-tournament.md).

- **Make the Tauri desktop app's data directory configurable.** It's currently
  hardcoded to `dirs::data_dir()/openshogipairings/tournaments/`
  (`frontend/src-tauri/src/lib.rs`'s `local_data_dir`), matching where
  `backup.rs` already puts its automatic backups. Some users may want it
  redirected (a synced folder, a different drive, a portable-install layout)
  — read it from an environment variable, falling back to the current default
  when unset.

- **OAuth authentication**

- **Log of which user took which action**

## Other

- **Webhook for pushing results and pulling players** See https://github.com/ffrgo/pairgoth/blob/master/doc/reference.md#pairgoth-webhook-specification

- **Team tournaments**

- **Standings by direct confrontation**

- **Package the executable** so it can be easily downloaded through github, rather than requiring building from source.