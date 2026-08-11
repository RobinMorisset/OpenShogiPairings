# OpenShogiPairings

[![CI](https://github.com/RobinMorisset/OpenShogiPairings/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/RobinMorisset/OpenShogiPairings/actions/workflows/ci.yml)
[![coverage](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/RobinMorisset/f05cf66791d190fa3defbb2ebf1dbcb6/raw/osp-coverage.json)](#coverage)

Tournament management software for shogi, built to fit shogi's needs rather than
reusing go/chess software. Built around a **client/server** architecture so
that multiple referees can edit the same tournament live, from their own
machines — as a portable single-file desktop app, or hosted on the internet and
reached from a browser.

## Download

Ready-to-run builds for **Windows** and **macOS** are on the
[**Releases**](https://github.com/RobinMorisset/OpenShogiPairings/releases/latest)
page:

- **Windows** — download the `.exe` (or the `.msi` installer) and double-click.
  No install or command line needed: the server, UI, and logic are all embedded
  in the single file.
- **macOS** — download the `.dmg`, open it, and drag OpenShogiPairings into
  Applications. It's a universal build (both Apple Silicon and Intel Macs).

> **You'll see an "unknown publisher" / "damaged" warning the first time.** These
> builds are **not code-signed** — I haven't paid for the Apple and Windows
> signing certificates — so the operating system shows a scary-looking warning on
> first launch. The app is safe; here's how to get past it:
> - **Windows**: on the blue SmartScreen popup, click **More info → Run anyway**.
> - **macOS**: right-click (or Ctrl-click) the app and choose **Open**, then
>   confirm in the dialog. If macOS insists the app is "damaged", run this once in
>   Terminal and then open it normally:
>   ```sh
>   xattr -dr com.apple.quarantine /Applications/OpenShogiPairings.app
>   ```

## Features

A tour of what OpenShogiPairings does for the referee actually running a
tournament. For how it's built, see [Architecture](#architecture) below.

### Shogi-specific

- **MacMahon groups**, with thresholds on either ELO rating or dan/kyu grade
  (freely mixed), and an optional **degressive ("accelerated") schedule** that
  lets a starting-point head start fade out over the first few rounds.
  Optionally, MacMahon can be awarded from a **live ELO estimate** rather than
  the registration rating, so a player's starting points track their estimated
  strength round by round (see the pairing options below).
- **Hybrid direct-elimination cup** alongside the Swiss (the French / European
  Championship format): top 8/16/32/64 eligible players play a seeded bracket
  over the first rounds, eliminated players drop back into the Swiss, and the
  Standings tab shows the podium medals. Optionally the bracket is fed by a
  **qualification round** (the German Championship format): half the bracket is
  pre-qualified and plays the open in round 1 while the next players play off
  for the remaining slots.
- **Handicap games**, with a recommended handicap (FFS Annexe 7, from the
  rating gap) suggested automatically, and draws-before-the-decisive-game
  recorded for ELO purposes.
- **FESA rating list integration**: registration autocompletes name, ELO, and
  grade from the federation's list, refreshable on demand; the tournament
  cross-table can be exported as an **American Grid** for the federation's
  ELO update.
- **Manual point adjustments** (a bonus or penalty with a mandatory reason),
  for corrections outside the normal scoring.
- **Two-round "long games"** (niche): for tournaments that give the top boards
  double the time control, a board can be flagged to span two rounds — its two
  players sit out the intervening round's pairing and the winner scores two
  points. Off by default.

### Exotic pairing options

- **Experimental pure ELO pairing mode**: ignores MacMahon and Swiss score
  groups entirely and pairs each round purely to minimize the estimated ELO gap,
  a "continuous Swiss" for fields where a smooth strength axis fits better than
  integer points.
- **Estimate-based MacMahon**: a lighter hybrid that leaves pairing alone and
  instead awards MacMahon starting points from a live ELO estimate — so the
  groups themselves react to results, while plain Swiss pairing runs as usual.
  Enabled from the MacMahon settings; needs at least one ELO threshold.
- **Club protection**, avoiding pairing players from the same club, optionally
  limited to the first N rounds and with specific clubs (e.g. the host club)
  exempted.
- **Airtight groups**: for the first N rounds, forbid pairing players with a
  different MacMahon point total, ahead of the usual score-gap penalty.
- **Floater selection style** — classic Swiss (the strongest of the lower
  group floats up) or median Swiss (the median floats up) — when a score
  group must pair across group lines.

### Convenience & ergonomics

- **"Why these pairings?"**: every round explains itself — a per-board
  compromise ledger, a round-level report of which rules had to bend, and a
  counterfactual probe ("why weren't these two paired?" / "why were they?")
  that shows exactly what forcing or forbidding a pairing would cost.
- **Click-to-edit** any player cell in place; forced pairings and a forced
  bye when hand-tuning a round draft.
- **Print** pairings, **save/load** a tournament as a portable JSON file.
- **UI in nine languages** (English, French, German, Japanese, Russian,
  Belarusian, Ukrainian, Slovak, Polish), light/dark theme.
- Runs as a **single portable desktop executable** (no install, no command
  line) or in a plain browser against a hosted server — same app either way.

### Safety

- **Multi-referee live collaboration over the internet**: several referees can
  edit the same tournament from different machines at once, with changes
  appearing live and simultaneous edits caught by conflict detection rather
  than silently overwritten.
- **Password-protected tournaments** (each with its own password, set at
  creation) plus a separate admin password gating who can create tournaments
  on a shared server — so one hosted instance can safely serve many
  tournaments/federations at once, picked from a list.
- **Full undo history** and **automatic backups** at every round-lifecycle
  transition (finalize / prepare / start / complete / cancel), restorable
  from the UI.
- **Guarded round lifecycle**: the next round can't start until the current
  one's games are all recorded, registration can't be skipped, and destructive
  actions (discard, delete) always ask for confirmation first.

## Limitations

- **Not yet battle-tested:** this project has not been used in a real tournament yet.
- **Not exact FIDE-style Swiss:** like OpenGotha or pairgoth, this software uses an engine
  that searches for a global optimum for pairings. This is intrinsically incompatible with
  the intricate rules governing the order in which pairings are to be tried under FIDE's
  Swiss regulations. All that can be guaranteed is that the resulting pairings are no worse,
  by a number of metrics, than those a more classical Swiss pairing program would find.
- **Not protected from untrusted referees:** the software assumes that all of the referees
  with access to a tournament can be trusted. In particular, it has no detailed log of
  which referee took which action.
- **Portability:** this project has only been tested on one Windows 10 desktop and one macOS
  laptop. It should run fine on Linux, and the web UI should work in any modern browser, but
  neither has been tested yet.

## Architecture

| Piece | Location | Tech | Role |
|-------|----------|------|------|
| Domain / pairing engine | [`crates/core`](crates/core) (`osp-core`) | Rust | Correctness-critical logic + shared DTOs. Reused by every client. |
| Matching solver | [`crates/matching`](crates/matching) (`integer-blossom`) | Rust | Standalone blossom (min-weight perfect matching) solver, self-contained and dependency-free. |
| HTTP server | [`crates/server`](crates/server) (`osp-server`) | Rust + axum | Multi-tournament source of truth; exposes the API, both as a standalone binary and as a library. |
| Simulation CLI | [`crates/sim`](crates/sim) (`osp-sim`) | Rust | Monte-Carlo comparison of pairing-settings variants — links `osp-core` directly and drives its pairing/scoring loop in-process, parallelized. See [`docs/simulation-cli.md`](docs/simulation-cli.md). |
| Web UI | [`frontend`](frontend) | TypeScript + Svelte 5 + Vite | Browser client; also the frontend embedded by Tauri. |
| Desktop app | `frontend/src-tauri` | Tauri 2 (Rust + system webview) | Self-contained app: **embeds the server** (`osp-server` as a library) and runs it in-process. |

The `osp-server` crate is both a standalone binary (browser dev, and the hosted
remote deployment — see [`docs/multi-referee-internet.md`](docs/multi-referee-internet.md))
and a library. The desktop app links the library and starts the API in-process
on an **OS-assigned port** (bound to `127.0.0.1:0` to avoid clashes and
firewall prompts); the frontend asks Rust for that port via the `api_base`
command. This is what lets the packaged app ship as a single self-contained
executable.

### Multi-tournament registry, and per-tournament access control

The server holds **any number of tournaments** at once, keyed by each
`Tournament`'s stable `id: Uuid` — a `TournamentRegistry`
([`crates/server/src/state.rs`](crates/server/src/state.rs)) mapping ids to
`TournamentInstance`s, each wrapping its own `TournamentStore` (current state +
undo history) and its own optional password. Clients start at a **picker**
(`GET /api/tournaments`, rendered by `TournamentPicker.svelte`) and open one
tournament at a time.

Auth is two-level, both a shared-password-plus-bearer-token model (see
[`crates/server/src/auth.rs`](crates/server/src/auth.rs)):

- **Per-tournament**: each tournament may have its own password, set at
  creation and checked at `POST /api/tournaments/{id}/login`; the hash (never
  the plaintext) persists in a `{id}.auth.json` sidecar file.
- **Admin**: one process-wide password (`OSP_ADMIN_PASSWORD`) gates *creating*
  tournaments and the FESA ratings proxy — both instance-wide capabilities
  rather than something scoped to one tournament.

A login exchanges the password for a per-boot random bearer token
(`Authorization: Bearer <token>`); a restart rotates every token. When
`OSP_DATA_DIR` is set, tournaments (and their passwords) persist to disk and
reload on boot; otherwise the registry is in-memory only.

### Live multi-referee sync and conflict detection

Several referees can edit the same tournament from different machines at
once, over the internet. Each mutation bumps a per-tournament monotonic
`version` counter; `GET /api/tournaments/{id}/events`
([`crates/server/src/live.rs`](crates/server/src/live.rs)) is a Server-Sent
Events stream that pushes that counter to every connected client, which then
refetches. Clients echo the version they last saw in an
`X-Tournament-Version` header on every mutation; if another referee's change
landed first, the request is rejected `409 Conflict` and the client refetches
and re-presents the edit rather than silently clobbering it (see the
`ConnectionStatus.svelte` Live/Reconnecting/Offline indicator and the
"another referee changed the tournament" reload message in the UI). See
[`docs/multi-referee-internet.md`](docs/multi-referee-internet.md) for the
full design (auth, hosted deployment, SSE sync, and reconnect resilience were
delivered as four separate phases, all now landed).

### Pairing engine

The pairing engine models a round as a **minimum-weight perfect matching** over a
complete player graph. The matching itself is an integer **blossom** solver
([`crates/matching`](crates/matching/src/lib.rs), general graphs —
Hungarian doesn't apply); the edge weights come from a set of Swiss rules
([`crates/core/src/pairing.rs`](crates/core/src/pairing.rs)) combined on a
priority ladder of multipliers — most important first: never rematch or repeat a
bye, give the bye to the lowest score group (penalty ∝ gap²), prefer equal
scores (penalty ∝ gap²), avoid repeating a float in the same
direction (decaying with time), select floaters (the weakest of the upper group
drops, the first — classic Swiss — or median lower-group player rises), avoid
club-mates (optional per tournament, and optionally only for the first N rounds
or with some clubs exempt), and fold each score group (top-half Nth meets
bottom-half Nth). The odd player's bye is modeled
as a phantom vertex. The multipliers are *derived* from each rule's worst-case
contribution, so the tiers are strictly disjoint by construction (lexicographic
priority) with no hand-tuned gaps; an ILP/CP-SAT backend for very large fields and
formats needing hard constraints is future work (see [TODO.md](TODO.md)).

Because a global optimum has no local "reason" for any one board, the engine
can also **explain itself**
([`docs/pairing-explanations.md`](docs/pairing-explanations.md)): a per-board
penalty ledger and a per-round report of which rules had to bend are always
computed (`GET /rounds/{n}/explanation`), and a referee can probe a specific
counterfactual — "why weren't these two paired?" or "why were they?" — via
`POST /rounds/{n}/counterfactual`, which re-solves with that pairing forced or
forbidden and reports the chain of boards that would change and why.

An optional **hybrid cup** runs a seeded direct-elimination bracket (top 8/16/32/
64 eligible players) over the first `log2(size)` rounds alongside the Swiss — the
French / European Championship format. Eligibility is marked per player during
registration, the size is chosen at finalization, and eliminated players drop
back into the Swiss (their cup games count, but a bracket pairing is not a Swiss
float). The semifinal losers play a small final for third place; the pairings
view badges every board as Swiss, referee-forced, or its cup stage, and the
Standings tab shows the podium medals (without reordering the Swiss ranking).

The cup fills its bracket one of two ways (`cup_format` in the settings). With
the **direct** format the top `size` eligible players *are* the bracket, which
starts in round 1. With the **qualifier** format — the German Championship
format — the top `size / 2` are pre-qualified and spend round 1 playing an
ordinary Swiss game in the open, while the next `size` play a **qualification**
round among themselves; its winners complete the bracket, which then runs from
round 2. That takes `1.5 × size` eligible players (12/24/48/96 for a bracket of
8/16/32/64) and one more round. The pre-qualified are ordinary Swiss players
that round with one exception the pairing engine enforces: they are never paired
with **each other** (a rule that exists only in that round). Seeding is unchanged: bracket round 1 folds the
pre-qualified against the qualifiers in match order, so the top seed meets the
winner of the weakest qualification match.

Two experimental **ELO-based pairing modes**
([`crates/core/src/elo.rs`](crates/core/src/elo.rs),
[`docs/elo-pairing-mode.md`](docs/elo-pairing-mode.md)) build on a live Bayesian
(maximum-a-posteriori, Bradley–Terry) estimate of every player's strength from
results so far. In **mixed mode**, MacMahon and the Swiss score-group rules
(score gap, float repeat, club protection, airtight groups) are all kept, but
the fold and floater-selection rules are replaced by minimizing the squared
estimated-ELO gap — so a score group is still formed exactly as in Swiss, and
only the ordering _within_ (and across) it follows current form instead of the
static registration rating; it stays fully compatible with MacMahon points. The
**pure ELO mode** is the more extreme variant: it drops MacMahon and the
Swiss-specific rules entirely, collapsing the rule ladder to the estimated-ELO
gap alone — a "continuous Swiss" where winners and losers drift along a smooth
strength axis instead of jumping between integer score groups.

### The UI

The web UI organizes a tournament into tabs: **Settings** (MacMahon groups,
degressive schedule, club protection, floater style, pairing mode, hybrid cup),
**Players**, **Standings** (per-round results plus Wins and total Points),
and one tab per round. Points are each player's wins plus their MacMahon
starting points (one per threshold they reach — an ELO rating or a dan/kyu
grade), and the pairing engine scores by total points. The round lifecycle is
gated: **prepare round** (a draft state to mark players absent, force pairings,
and force the bye — for round 1 this also finalizes registration in the same
step) → **start round** (confirm) → play games → the round **completes
automatically** once every board has a result (or no-show), unlocking the next
round; **cancel last round** peels back one stage (discarding an open draft,
else removing the most recent round) to replay it or undo a mistake.
Finalizing registration assigns each player a tournament number (by ELO,
unrated last; later additions get the next free number). The Standings tab is
a ranked table: a row per player (ordered by the referee-chosen criteria) with
one column per completed round (`opponent-number` + `+`/`−`, or `0+` for a bye
/ `0-` for an absence), a win count, and one column per selected ranking
criterion — Points plus fourteen tie-break metrics (SOS / SODOS / SOSOS, the
Buchholz cuts, and the cumulative score, each in a MacMahon-inclusive and a
wins-only flavour; direct confrontation; and the estimated ELO in the ELO
pairing modes), reorderable in Settings.

Mutations go through a `TournamentStore` that keeps the current tournament plus a
stack of prior snapshots (the undo history); create/load/restore reset it.
Endpoints return a `TournamentView` — the tournament, `can_undo`, the change
`version`, server-computed `standings`, the cup podium (once decided), and
suggested handicaps per board — so the client refreshes the view, the undo
button, and the ranked table together from one response (the persisted
save-file shape stays the bare `Tournament`). `standings` is computed
server-side (in `osp-core`) so every client — and the American Grid export —
shares one canonical ranking: by the criteria chosen in the settings (in
order; points is one of them, normally first), then tournament number.

### API

Registry-level routes (not scoped to any one tournament):

| Method & path | Purpose |
|---------------|---------|
| `GET /api/health` | Liveness check. |
| `GET /api/tournaments` | List every known tournament (id, name, whether it's password-protected) — public, needed to render the picker before logging in anywhere. |
| `POST /api/tournaments` | Create a new tournament: `{ "name": "...", "password"? }`. Admin-gated if `OSP_ADMIN_PASSWORD` is set. Returns `{ "id", "token"? }` (a token if the new tournament has a password, so the creator needn't immediately log in to it). |
| `POST /api/tournaments/import` | Create a tournament from a save file (what the picker's "Load from file…" does): `{ "tournament": {…the file verbatim…}, "password"? }`. Same admin gate and same `{ "id", "token"? }` response as creating one. Deliberately a *single* request: the format version and the tournament's own invariants are checked before anything is registered, so a file this build can't read leaves nothing behind. The file's own `id` is ignored — the registry mints a fresh one, so importing the same file twice can't collide. |
| `DELETE /api/tournaments/{id}` | Delete a tournament: its registry entry, persisted file, and backups. |
| `POST /api/admin/login` | Exchange the admin password for a bearer token. |
| `GET /api/ratings` | FESA rating list (server-cached) for registration autocomplete. Admin-gated. |
| `POST /api/ratings/refresh` | Re-download the FESA list now (manual refresh). Admin-gated. |

Per-tournament routes, all nested under `/api/tournaments/{id}` and requiring
that tournament's bearer token if it has a password (except `/login` and
`/events`):

| Method & path | Purpose |
|---------------|---------|
| `POST /login` | Exchange this tournament's password for a bearer token. |
| `GET /events` | SSE stream of this tournament's change `version`, for live sync. |
| `GET /` | Fetch the tournament (`TournamentView`; 404 if unknown). |
| `DELETE /` | Delete the tournament. |
| `POST /undo` | Revert the last change (server-side undo history). |
| `GET /american-grid` | Export the cross-table (American Grid) as `text/plain` for an ELO update: one row per player in final-rank order, opponents referenced by final rank, drawn games as `=`. |
| `PUT /settings` | Replace the whole `TournamentSettings`. Its shape is nested: `pairing` is a tagged union — either `{ "kind": "swiss", "floater_style": "classic"｜"median", "macmahon": { "thresholds": [ { "criterion": { "kind": "elo", "value": 1200 }, "drops_after_round"?: 3 }, { "criterion": { "kind": "grade", "grade": { "kind": "dan"｜"kyu", "level": 1 } } } ], "source": { "kind": "static" } }, "airtight_groups"?: 2, "club_protection": { "kind": "on", "rounds"?: 3, "exempt_clubs"?: ["Paris"] } }` or `{ "kind": "elo", "estimator": { … } }` for the experimental pure-ELO pairing mode. Alongside `pairing`, the top level carries `cup_enabled`, `cup_format` (`"direct"`｜`"qualifier"` — see the hybrid-cup section above; only consulted when `cup_enabled`), `long_boards_enabled`, `handicap_policy` (`{ "kind": "none" }｜{ "kind": "enabled", "display": …, "wiel_rule"?: false }`), `half_point_absences`, `tiebreaks` (an ordered array, e.g. `["points","sos_m",…]`), and `categories` (referee-defined player categories, an array of `{ "id", "name" }`; blank-named entries are dropped on normalization). Notes: a threshold's `criterion` mixes ELO and grade freely (each counted independently) and `drops_after_round` makes it a degressive threshold; `airtight_groups`, if set, forbids pairing across MacMahon groups during rounds `1..=n`; `club_protection` is `{ "kind": "off" }` or `{ "kind": "on", … }`; `macmahon.source` is `{ "kind": "static" }` or `{ "kind": "from_estimate", "estimator": { … } }` (estimate-based MacMahon). The `estimator` knobs are described in [`docs/elo-pairing-mode.md`](docs/elo-pairing-mode.md). |
| `POST /cancel-round` | Cancel the last round — discards the open draft if one is being prepared, otherwise removes the most recent round (undoable). |
| `POST /rounds/prepare` | Begin drafting the next round. For round 1, finalizes registration first, in the same undo step — that is the only way to finalize. Body optional: `{ "cup_size": 8｜16｜32｜64 }` when the hybrid cup is enabled — `cup_size` is the *bracket* size; it seeds the top eligible players into it, taking `cup_size` of them under `cup_format: "direct"` and `1.5 × cup_size` under `"qualifier"` (400 if fewer are marked eligible). Ignored from round 2 on (already finalized). A round completes automatically once every board has a result, so there is no separate "complete round" call. |
| `PUT /draft` | Edit the draft: `{ "absent", "forced_boards", "forced_byes" }` — or, in a team tournament, `{ "absent", "forced_matches": [{ "team1", "team2" }], "forced_team_byes" }`, since teams are what get paired there. The absent set is per player in either mode: a team member can be absent without their team being, and their board is then forfeited for them. Sending the other mode's forced lists is an error, not a silently ignored field. |
| `POST /rounds` | Confirm the draft: pair remaining players and start the round. |
| `GET /rounds/{n}/explanation` | Explain a round's Swiss pairings: per-board rule ledger and round report. Read-only. |
| `POST /rounds/{n}/counterfactual` | Explain forcing (`"force"`) or forbidding (`"forbid"`) a pairing `{a, b}` in this round. `a`/`b` are player numbers, or **team** numbers in a team tournament — whichever the engine paired; `0` means the bye. Read-only. |
| `POST /rounds/force-pairing` | Re-pair the current round with `{a, b}` fixed — a board, or a **team match** in a team tournament (same numbering as the counterfactual; `0` forces the other side onto the bye). |
| `POST /rounds/{n}/boards/{i}/result` | Toggle a board's winner: `{ "clicked": "player1"｜"player2" }`. |
| `POST /rounds/{n}/boards/{i}/drawn` | Set the "a draw occurred" flag: `{ "drawn": true｜false }`. |
| `POST /rounds/{n}/boards/{i}/no-show` | Mark a board a no-show (or clear it): `{ "absent": "player1"｜"player2"｜"both"｜null }`. A no-show settles the board (counting toward auto-completing the round) and carries the absence into the next round's draft. |
| `PUT /rounds/{n}/boards/{i}/handicap` | Set/clear the handicap: `{ "handicap": "4p"｜null }` (giver frozen from ratings; 400 if ratings equal). |
| `POST /rounds/{n}/boards/{i}/long` | Flag/unflag a board as a two-round "long game": `{ "long": true｜false }` (only when `long_boards_enabled`). Flagging the last-undecided board can complete the round, like recording a result. |
| `PUT /rounds/{n}/sitouts/{player}` | Re-score what a round was worth to a player who sat it out: `{ "value": "zero"｜"half"｜"full" }`. Allowed on completed rounds. |
| `POST /players` | Register a player: `{ "last_name", "first_name?", "rating?", "grade?", "nationality?", "club?" }` (`grade` is `{ "kind": "dan"｜"kyu", "level": … }`). |
| `POST /players/batch` | Register several players at once (one undo step): a JSON array of player objects, each shaped like the `POST /players` body. |
| `POST /players/import-csv` | Register a roster from a raw CSV text body (parsed server-side, ratings matched against the FESA cache), as a single undo step. |
| `PUT /players/{id}` | Edit a player's fields in place. |
| `DELETE /players/{id}` | Remove a player (400 if they are seeded in the cup bracket). |
| `POST /players/{id}/eligible` | Set cup eligibility: `{ "eligible": true｜false }`. |
| `POST /players/{id}/category` | Add/remove membership in a referee-defined category: `{ "category_id", "member": true｜false }` (404 if the category doesn't exist). |
| `POST /players/{id}/adjustments` | Apply a manual point bonus/penalty: `{ "delta", "reason" }` (reason mandatory). |
| `DELETE /players/{id}/adjustments/{adjustment_id}` | Remove a point adjustment. |
| `PUT /players/{id}/pairing-rating` | Set (or clear, with `null`) a player's pairing ELO: `{ "pairing_rating": 1400｜null }`. Team mode with MacMahon starting points only — it feeds the team average and nothing user-facing, and is never exported. |
| `POST /teams` | Create a team: `{ "name" }` (non-empty, unique ignoring case). Team mode only, and only while registration is open — as for every team route below. |
| `PUT /teams/{team_id}` | Rename a team: `{ "name" }`. |
| `DELETE /teams/{team_id}` | Delete a team; its members go back to the unassigned pool (they stay registered). |
| `POST /teams/{team_id}/members` | Add a registered player to a team, at the end of its board order: `{ "player_id" }`. A player belongs to exactly one team, and a team stops at its configured size. |
| `DELETE /teams/{team_id}/members/{player_id}` | Take a player out of a team. |
| `PUT /teams/{team_id}/board-order` | Set the board order (index 0 = board 1): `{ "order": [player_id, …] }`, which must be a permutation of the team's current members. |
| `POST /teams/{team_id}/sort-by-rating` | Reset the board order to descending pairing rating (unrated last). |
| `GET /backups` | List automatic backups, newest first (taken at every round-lifecycle transition). |
| `POST /backups/{backup_id}/restore` | Restore a backup as the current tournament; resets undo history. |

The FESA rating list is fixed-width, Latin-1 text (parsed in
[`crates/core`](crates/core/src/fesa.rs)). It's shared reference data, so the
**server** owns the cache: it downloads from FESA **only once**, the first time a
list is needed with nothing cached, then keeps it in memory and persists it to a
per-user cache file. It never re-downloads on its own — updating the list is a
manual action (the "Refresh FESA list" button → `POST /api/ratings/refresh`).
Clients pull the list once and filter locally.

Known limitations and future work are tracked in [TODO.md](TODO.md).

Every mutating endpoint returns the full updated `TournamentView`. Save/load is
platform-aware: in the **Tauri** desktop app it uses native OS file dialogs (the
`dialog` plugin plus small `read_text_file`/`write_text_file` commands), and in
the browser it falls back to a JSON download / file-picker upload. Either way a
loaded tournament is `PUT` back to the server, which stays authoritative.

## Prerequisites

- **Rust** (stable, ≥ 1.77) via [rustup](https://rustup.rs)
- **Node.js** LTS (≥ 20) with npm
- For the Tauri desktop client: platform WebView (WebView2 is preinstalled on
  current Windows; `webkit2gtk` on Linux) — see the
  [Tauri prerequisites](https://tauri.app/start/prerequisites/).

## Running (development)

Two processes; run them in separate terminals.

**1. Server** (listens on `http://127.0.0.1:3000`):

```sh
cargo run -p osp-server
```

**2. Web UI** (Vite dev server on `http://localhost:5173`):

```sh
cd frontend
npm install   # first time only
npm run dev
```

Open <http://localhost:5173>. The page reports whether it can reach the server.

### Desktop app

```sh
cd frontend
npm run tauri dev
```

The desktop app **embeds the server**, so you do *not* run `cargo run -p osp-server`
alongside it — it starts its own API in-process.

> `tauri dev` starts its own Vite dev server on port 5173 (via
> `beforeDevCommand`), so don't also have a browser `npm run dev` (or another
> preview) running on 5173 at the same time — the port is fixed (`strictPort`)
> and the second one will fail with "Port 5173 is already in use".

## Packaging (Windows)

> To publish prebuilt Windows **and** macOS apps to the GitHub Releases page,
> don't build by hand — push a version tag and let CI do it. See
> [Cutting a release](docs/cutting-a-release.md). The rest of this section is for
> producing a one-off local build.

Build the self-contained portable executable:

```sh
cd frontend
npm run tauri build -- --no-bundle
```

The result is a single file at
`frontend/src-tauri/target/release/OpenShogiPairings.exe` — server, UI, and logic
all embedded. Referees just double-click it; no install, no command line, no
separate server.

The one system dependency is **WebView2** (the browser engine the UI renders
in), which is preinstalled on all Windows 10 (2021+) and 11. To instead produce
an installer that bundles WebView2 for fully-offline install on any machine, drop
`--no-bundle` and set `bundle.windows.webviewInstallMode` to `offlineInstaller`
in [`tauri.conf.json`](frontend/src-tauri/tauri.conf.json).

## Testing

```sh
cargo test          # Rust workspace (core, server, sim, matching)
cd frontend && npm run check   # Svelte / TypeScript type-check
cd frontend && npm test        # frontend unit tests (vitest)
```

Dev helpers (Windows/PowerShell) in [`scripts/`](scripts):

- `scripts/check.ps1` — runs `cargo test` and the frontend type-check
  (`npm run check`).
- `scripts/restart-server.ps1` — restart `osp-server` and wait until it responds
  (the running server doesn't hot-reload, so restart it after backend changes).

### Git hooks

Versioned hooks live in [`scripts/git-hooks/`](scripts/git-hooks) (not
`.git/hooks/`, which isn't checked in). Point git at them once per clone:

```sh
git config core.hooksPath scripts/git-hooks
```

- `pre-commit` — rustfmt (auto-formats staged `.rs` files and re-stages them, so
  a formatting nit doesn't fail the commit; a partially-staged file it would
  rewrite is reported and gates instead), `cargo clippy --workspace
  --all-targets -- -D warnings`, `svelte-check`, and an i18n locale-key check
  (`scripts/check-i18n-keys.mjs`, catching keys that exist in some locales but
  not all). Fast; keeps the tree clean commit by commit.
- `pre-push` — the full test suite: `cargo test --workspace` and the frontend
  tests (`npm test`). Slower, so it only runs before sharing work.

### Coverage

Rust coverage uses [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
(`cargo install cargo-llvm-cov`, plus `rustup component add llvm-tools-preview`):

```sh
cargo llvm-cov -p osp-core --summary-only      # core only
cargo llvm-cov -p osp-server --summary-only    # server only
cargo llvm-cov --workspace --summary-only      # everything, incl. binaries
cargo llvm-cov --workspace --html              # HTML report in target/llvm-cov/html/index.html

# The exact figure the CI badge reports: the shipped library crates, with
# binary entry points (server/src/main.rs) excluded.
cargo llvm-cov -p osp-core -p integer-blossom -p osp-server \
  --ignore-filename-regex 'main\.rs$' --summary-only
```

**What the badge measures.** The `coverage` job in CI reports the shipped library
crates (`osp-core`, `integer-blossom`, `osp-server`) and *excludes* binary entry
points — `server/src/main.rs` and the whole `osp-sim` dev tool are 0%-by-nature
glue that would understate the real figure. The measured logic sits at ~95% line
coverage; the residual gaps are deliberately-untested server I/O (the live FESA
fetch in `ratings.rs`, the SSE stream in `live.rs`), not core logic. The job is
informational — it never fails the build.

**Enabling the badge (one-time GitHub setup).** The badge reads its number from a
GitHub Gist that CI keeps up to date. Two account-side steps, which the repo owner
must do (they can't be scripted here):

1. Create a **secret** Gist (any single file, e.g. `osp-coverage.json` with `{}`),
   and note its ID (the hash in its URL).
2. Create a [fine-grained PAT](https://github.com/settings/tokens) with **Gist**
   read/write, then in this repo's **Settings → Secrets and variables → Actions**
   add a secret `GIST_TOKEN` (the PAT) and a variable `COVERAGE_GIST_ID` (the ID).

With both in place, every push to `main` refreshes the number in the Gist
([`f05cf66…`](https://gist.github.com/RobinMorisset/f05cf66791d190fa3defbb2ebf1dbcb6)),
which the coverage badge at the top of this README reads via
[shields.io](https://shields.io).

The frontend uses [vitest](https://vitest.dev) (`npm test`) for unit tests but has
no coverage tooling wired up, by design — very little testable logic lives there.
