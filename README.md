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

> **Current status:** bring-up phase. The server exposes only `GET /api/health`,
> and the web UI does nothing but confirm it can reach that endpoint. No pairing
> logic yet.

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

## Testing

```sh
cargo test          # Rust workspace (core + server)
cd frontend && npm run check   # Svelte / TypeScript type-check
```
