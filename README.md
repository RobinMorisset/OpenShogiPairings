# OpenShogiPairings

Tournament management software for shogi, built to fit shogi's needs rather than
reusing go/chess software. Designed around a **client/server** architecture so
that multiple referees can edit the same tournament from their own machines, and
so that additional clients (a CLI for quick simulations, a Tauri desktop app) can
be added over time.

## Architecture

| Piece | Location | Tech | Role |
|-------|----------|------|------|
| Domain / pairing engine | [`crates/core`](crates/core) | Rust | Correctness-critical logic + shared DTOs. Reused by every client. |
| HTTP server | [`crates/server`](crates/server) | Rust + axum | Single source of truth; exposes the API. |
| Web UI | [`frontend`](frontend) | TypeScript + Svelte 5 + Vite | Browser client; also the frontend embedded by Tauri. |
| Desktop app | `frontend/src-tauri` | Tauri 2 (Rust + system webview) | Self-contained app: **embeds the server** (`osp-server` as a library) and runs it in-process. |

The `osp-server` crate is both a standalone binary (browser dev, future CLI) and
a library. The desktop app links the library and starts the API in-process on an
**OS-assigned port** (bound to `127.0.0.1:0` to avoid clashes and firewall
prompts); the frontend asks Rust for that port via the `api_base` command. This
is what lets the packaged app ship as a single self-contained executable.

The pairing engine models a round as a **minimum-weight perfect matching** over a
complete player graph. The matching itself is an integer **blossom** solver
([`crates/core/src/matching.rs`](crates/core/src/matching.rs), general graphs —
Hungarian doesn't apply); the edge weights come from a set of Swiss rules
([`crates/core/src/pairing.rs`](crates/core/src/pairing.rs)) combined on a
priority ladder of multipliers — most important first: never rematch or repeat a
bye, prefer equal scores (penalty ∝ gap²), avoid repeating a float in the same
direction (decaying with time), avoid club-mates, and fold each score group
(top-half Nth meets bottom-half Nth). The odd player's bye is modeled as a
phantom vertex. The multiplier ladder approximates strict lexicographic priority
and is sized for realistic events; an ILP/CP-SAT backend for stricter tiers and
experimental formats is future work (see [TODO.md](TODO.md)).

The UI organizes a tournament into tabs: **Settings** (MacMahon groups),
**Players**, **Results** (per-round results plus Victories and total Points),
and one tab per round. Points are each player's victories plus their MacMahon
starting points (one per ELO threshold their rating reaches), and the pairing
engine scores by total points.

> **Current status:** early. The server holds a single in-memory tournament (a
> name + a list of players) as the shared source of truth, with a REST API to
> create it, register/remove players, and replace it wholesale (for load).
> Players have a last name, first name, optional rating, nationality and club.
> Registration autocompletes names + ELOs from the FESA rating list. The player
> table is sorted by descending ELO (unrated last), any cell is editable in
> place, and a server-side undo history reverts changes. Rounds can be started,
> which pairs players by weighted minimum-weight matching (see below). The round lifecycle is
> gated: **finalize registration** → **prepare round** (a draft state to mark
> players absent, force pairings, and force the bye) → **start round** (confirm)
> → play games → **complete round** (only once every game is played) → prepare
> the next round. In a round
> tab, clicking a player records them as the winner (click the other to switch,
> click the winner again to clear — three states); completed rounds stay editable
> with a warning. Finalizing registration assigns each player a tournament
> number (by ELO, unrated last; later additions get the next free number). The
> **Results** tab is a ranked table: a row per player (ordered by points, then
> SOS / SODOS / SOSOS tie-breaks) with one column per completed round
> (`opponent-number` + `+`/`−`, or `0+` for a bye / `0-` for an absence), a
> victory count, and Points / SOS / SODOS / SOSOS columns. The web UI is organized
> into tabs (Settings / Players / Results / one per round) and can save/load the
> tournament as a JSON file.

Mutations go through a `TournamentStore` that keeps the current tournament plus a
stack of prior snapshots (the undo history); create/load reset it. Endpoints
return `{ tournament, can_undo, standings }` so the client refreshes the view,
the undo button, and the ranked table together (the persisted save-file shape
stays the bare tournament). `standings` is computed server-side (in `osp-core`)
so every client — and the future American grid — shares one canonical ranking:
by points, then the SOS / SODOS / SOSOS tie-breaks, then tournament number.

### API

| Method & path | Purpose |
|---------------|---------|
| `GET /api/health` | Liveness check. |
| `GET /api/ratings` | FESA rating list (server-cached) for registration autocomplete. |
| `POST /api/ratings/refresh` | Re-download the FESA list now (manual refresh). |
| `POST /api/tournament` | Create a new (empty) tournament: `{ "name": "..." }`. |
| `GET /api/tournament` | Fetch the current tournament (404 if none). |
| `PUT /api/tournament` | Replace the current tournament (used by "load"). |
| `POST /api/tournament/undo` | Revert the last change (server-side undo history). |
| `PUT /api/tournament/settings` | Update settings: `{ "macmahon_thresholds": [1200, 1700] }` (stored sorted & de-duplicated). |
| `POST /api/tournament/finalize-registration` | Finalize registration (unlocks round 1). |
| `POST /api/tournament/complete-round` | Complete the current round (all games must be played). |
| `POST /api/tournament/rounds/prepare` | Begin drafting the next round. |
| `PUT /api/tournament/draft` | Edit the draft (absent set, forced pairings, forced bye). |
| `POST /api/tournament/rounds` | Confirm the draft: pair remaining players and start the round. |
| `POST /api/tournament/rounds/{n}/boards/{i}/result` | Toggle a board's winner: `{ "clicked": "player1"｜"player2" }`. |
| `POST /api/tournament/rounds/{n}/boards/{i}/drawn` | Set the "a draw occurred" flag: `{ "drawn": true｜false }`. |
| `PUT /api/tournament/rounds/{n}/boards/{i}/handicap` | Set/clear the handicap: `{ "handicap": "4p"｜null }` (giver frozen from ratings; 400 if ratings equal). |
| `POST /api/tournament/players` | Register a player: `{ "last_name", "first_name?", "rating?", "nationality?", "club?" }`. |
| `PUT /api/tournament/players/{id}` | Edit a player's fields in place. |
| `DELETE /api/tournament/players/{id}` | Remove a player. |

The FESA rating list is fixed-width, Latin-1 text (parsed in
[`crates/core`](crates/core/src/fesa.rs)). It's shared reference data, so the
**server** owns the cache: it downloads from FESA **only once**, the first time a
list is needed with nothing cached, then keeps it in memory and persists it to a
per-user cache file. It never re-downloads on its own — updating the list is a
manual action (the "Refresh FESA list" button → `POST /api/ratings/refresh`).
Clients pull the list once and filter locally.

Known limitations and future work are tracked in [TODO.md](TODO.md).

Every mutating endpoint returns the full updated tournament. Save/load is
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
