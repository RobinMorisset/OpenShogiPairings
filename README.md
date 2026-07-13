# OpenShogiPairings

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
- **Hybrid direct-elimination cup** alongside the Swiss (the French / European
  Championship format): top 8/16/32/64 eligible players play a seeded bracket
  over the first rounds, eliminated players drop back into the Swiss, and the
  Standings tab shows the podium medals.
- **Handicap games**, with a recommended handicap (FFS Annexe 7, from the
  rating gap) suggested automatically, and draws-before-the-decisive-game
  recorded for ELO purposes.
- **FESA rating list integration**: registration autocompletes name, ELO, and
  grade from the federation's list, refreshable on demand; a completed round
  can be exported as an **American Grid** cross-table for the federation's
  ELO update, and a grid can be imported back to rebuild a tournament.
- **Manual point adjustments** (a bonus or penalty with a mandatory reason),
  for corrections outside the normal scoring.

### Exotic pairing options

- **Experimental ELO-based pairing mode**: ignores MacMahon and Swiss score
  groups entirely and instead maintains a live Bayesian estimate of every
  player's strength, pairing each round to minimize the estimated ELO gap —
  a "continuous Swiss" for fields where a smooth strength axis fits better
  than integer points.
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
- **English and French UI**, light/dark theme.
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
- **Guarded round lifecycle**: a round can't be completed with games still
  unplayed, registration can't be skipped, and destructive actions (discard,
  delete) always ask for confirmation first.

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

An experimental **ELO-based pairing mode**
([`crates/core/src/elo.rs`](crates/core/src/elo.rs),
[`docs/elo-pairing-mode.md`](docs/elo-pairing-mode.md)) can replace MacMahon
and the Swiss-specific rules entirely: OSP maintains a live Bayesian
(maximum-a-posteriori, Bradley–Terry) estimate of every player's strength from
results so far, and the rule ladder collapses to minimizing the squared
estimated-ELO gap on each board — a "continuous Swiss" where winners and
losers drift along a smooth strength axis instead of jumping between integer
score groups.

### The UI

The web UI organizes a tournament into tabs: **Settings** (MacMahon groups,
degressive schedule, club protection, floater style, ELO mode, hybrid cup),
**Players**, **Standings** (per-round results plus Wins and total Points),
and one tab per round. Points are each player's wins plus their MacMahon
starting points (one per threshold they reach — an ELO rating or a dan/kyu
grade), and the pairing engine scores by total points. The round lifecycle is
gated: **finalize registration** → **prepare round** (a draft state to mark
players absent, force pairings, and force the bye) → **start round** (confirm)
→ play games → **complete round** (only once every game is played) → prepare
the next round; **cancel last round** peels back one stage (discarding an open
draft, else removing the most recent round) to replay it or undo a mistake.
Finalizing registration assigns each player a tournament number (by ELO,
unrated last; later additions get the next free number). The Standings tab is
a ranked table: a row per player (ordered by the referee-chosen criteria) with
one column per completed round (`opponent-number` + `+`/`−`, or `0+` for a bye
/ `0-` for an absence), a win count, and one column per selected ranking
criterion — Points plus twelve tie-break metrics (SOS / SODOS / SOSOS, the
Buchholz cuts, and the cumulative score, each in a MacMahon-inclusive and a
wins-only flavour), reorderable in Settings.

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
| `PUT /` | Replace the tournament wholesale (used by "load"); resets undo history. |
| `DELETE /` | Delete the tournament. |
| `POST /undo` | Revert the last change (server-side undo history). |
| `GET /american-grid` | Export the cross-table (American Grid) as `text/plain` for an ELO update: one row per player in final-rank order, opponents referenced by final rank, drawn games as `=`. |
| `PUT /american-grid` | Import an American Grid (raw `text/plain` body), rebuilding the tournament from it — registers the players, forces every round's pairings, and replays the results. Meant for seeding a non-trivial state in tests/simulations, not surfaced in the UI. |
| `PUT /settings` | Update settings (the whole `TournamentSettings`): `{ "macmahon_thresholds": [{ "criterion": { "kind": "elo", "value": 1200 } }, { "criterion": { "kind": "grade", "grade": { "kind": "dan", "level": 1 } }, "drops_after_round": 3 }], "airtight_groups_rounds": 2, "club_protection_enabled": true, "club_protection_rounds": 3, "club_protection_exempt_clubs": ["Paris"], "elo_mode_enabled": … }`. Each threshold's `criterion` is either an ELO rating (`{ "kind": "elo", "value": … }`) or a dan/kyu grade (`{ "kind": "grade", "grade": { "kind": "dan"｜"kyu", "level": … } }`) — a tournament can freely mix both kinds, each counted independently; `airtight_groups_rounds`, if set, forbids pairing players with a different number of MacMahon points during rounds `1..=n`; `floater_style` is `"classic"｜"median"`; `cup_enabled` toggles the hybrid direct-elimination cup (its size is chosen at finalization). |
| `POST /finalize-registration` | Finalize registration (unlocks round 1). Body optional: `{ "cup_size": 8｜16｜32｜64 }` when the hybrid cup is enabled — seeds the top-N eligible players into a direct-elimination bracket. |
| `POST /complete-round` | Complete the current round (all games must be played). |
| `POST /cancel-round` | Cancel the last round — discards the open draft if one is being prepared, otherwise removes the most recent round (undoable). |
| `POST /rounds/prepare` | Begin drafting the next round. |
| `PUT /draft` | Edit the draft (absent set, forced pairings, forced bye). |
| `POST /rounds` | Confirm the draft: pair remaining players and start the round. |
| `GET /rounds/{n}/explanation` | Explain a round's Swiss pairings: per-board rule ledger and round report. Read-only. |
| `POST /rounds/{n}/counterfactual` | Explain forcing (`"force"`) or forbidding (`"forbid"`) a pairing `{a, b}` in this round. Read-only. |
| `POST /rounds/force-pairing` | Re-pair the current round with board `{a, b}` fixed. |
| `POST /rounds/{n}/boards/{i}/result` | Toggle a board's winner: `{ "clicked": "player1"｜"player2" }`. |
| `POST /rounds/{n}/boards/{i}/drawn` | Set the "a draw occurred" flag: `{ "drawn": true｜false }`. |
| `PUT /rounds/{n}/boards/{i}/handicap` | Set/clear the handicap: `{ "handicap": "4p"｜null }` (giver frozen from ratings; 400 if ratings equal). |
| `POST /players` | Register a player: `{ "last_name", "first_name?", "rating?", "grade?", "nationality?", "club?" }` (`grade` is `{ "kind": "dan"｜"kyu", "level": … }`). |
| `PUT /players/{id}` | Edit a player's fields in place. |
| `DELETE /players/{id}` | Remove a player (400 if they are seeded in the cup bracket). |
| `POST /players/{id}/eligible` | Set cup eligibility: `{ "eligible": true｜false }`. |
| `POST /players/{id}/adjustments` | Apply a manual point bonus/penalty: `{ "delta", "reason" }` (reason mandatory). |
| `DELETE /players/{id}/adjustments/{adjustment_id}` | Remove a point adjustment. |
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
cargo test          # Rust workspace (core + server)
cd frontend && npm run check   # Svelte / TypeScript type-check
```

Dev helpers (Windows/PowerShell) in [`scripts/`](scripts):

- `scripts/check.ps1` — runs both of the above.
- `scripts/restart-server.ps1` — restart `osp-server` and wait until it responds
  (the running server doesn't hot-reload, so restart it after backend changes).

### Git hooks

Versioned hooks live in [`scripts/git-hooks/`](scripts/git-hooks) (not
`.git/hooks/`, which isn't checked in). Point git at them once per clone:

```sh
git config core.hooksPath scripts/git-hooks
```

- `pre-commit` — `cargo fmt --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, and `svelte-check`. Fast; keeps the tree clean commit by
  commit.
- `pre-push` — `cargo test --workspace`. Slower, so it only runs before
  sharing work.

### Coverage

Rust coverage uses [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
(`cargo install cargo-llvm-cov`, plus `rustup component add llvm-tools-preview`):

```sh
cargo llvm-cov -p osp-core --summary-only      # core only
cargo llvm-cov -p osp-server --summary-only    # server only
cargo llvm-cov --workspace --summary-only      # both combined
cargo llvm-cov --workspace --html              # HTML report in target/llvm-cov/html/index.html
```

The frontend has no test framework set up yet (no vitest/jest), so there is
nothing to measure coverage on there — see [TODO.md](TODO.md).
