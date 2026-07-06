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
| Desktop client | `frontend/src-tauri` | Tauri 2 (Rust + system webview) | Optional native wrapper of the web UI. |

The pairing engine will model a round as a **minimum-weight perfect matching**
over a weighted player graph (Blossom algorithm), graduating to an ILP/CP-SAT
solver for experimental formats beyond Swiss / MacMahon.

> **Current status:** early. The server holds a single in-memory tournament (a
> name + a list of players) as the shared source of truth, with a REST API to
> create it, register/remove players, and replace it wholesale (for load). The
> web UI drives all of that and can save/load the tournament as a JSON file. No
> rounds or pairing logic yet.

### API

| Method & path | Purpose |
|---------------|---------|
| `GET /api/health` | Liveness check. |
| `POST /api/tournament` | Create a new (empty) tournament: `{ "name": "..." }`. |
| `GET /api/tournament` | Fetch the current tournament (404 if none). |
| `PUT /api/tournament` | Replace the current tournament (used by "load"). |
| `POST /api/tournament/players` | Register a player: `{ "name", "rating?", "club?" }`. |
| `DELETE /api/tournament/players/{id}` | Remove a player. |

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

### Desktop client (optional)

```sh
cd frontend
npm run tauri dev
```

> `tauri dev` starts its **own** Vite dev server on port 5173 (via
> `beforeDevCommand`), so don't also have `npm run dev` (or another preview)
> running on 5173 at the same time — the port is fixed (`strictPort`) and the
> second one will fail with "Port 5173 is already in use". You still need the
> **server** (`cargo run -p osp-server`) running for the app to reach the API.

## Testing

```sh
cargo test          # Rust workspace (core + server)
cd frontend && npm run check   # Svelte / TypeScript type-check
```
