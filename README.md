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

The pairing engine will model a round as a **minimum-weight perfect matching**
over a weighted player graph (Blossom algorithm), graduating to an ILP/CP-SAT
solver for experimental formats beyond Swiss / MacMahon.

> **Current status:** early. The server holds a single in-memory tournament (a
> name + a list of players) as the shared source of truth, with a REST API to
> create it, register/remove players, and replace it wholesale (for load).
> Players have a last name, first name, optional rating, nationality and club.
> Registration autocompletes names + ELOs from the FESA rating list. The web UI
> drives all of that and can save/load the tournament as a JSON file. No rounds
> or pairing logic yet.

### API

| Method & path | Purpose |
|---------------|---------|
| `GET /api/health` | Liveness check. |
| `GET /api/ratings` | FESA rating list (server-cached) for registration autocomplete. |
| `POST /api/tournament` | Create a new (empty) tournament: `{ "name": "..." }`. |
| `GET /api/tournament` | Fetch the current tournament (404 if none). |
| `PUT /api/tournament` | Replace the current tournament (used by "load"). |
| `POST /api/tournament/players` | Register a player: `{ "last_name", "first_name?", "rating?", "nationality?", "club?" }`. |
| `DELETE /api/tournament/players/{id}` | Remove a player. |

The FESA rating list is fixed-width, Latin-1 text (parsed in
[`crates/core`](crates/core/src/fesa.rs)). It's shared reference data, so the
**server** owns the cache — it fetches from FESA at most once per TTL, keeps the
parsed list in memory, and persists it to a per-user cache file (serving stale
data if FESA is unreachable). Clients pull the list once and filter locally.

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
