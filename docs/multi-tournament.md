# Multi-tournament server — design

Status: **All three phases landed** (server registry, frontend picker, Tauri
persistence — see §7). Supersedes the "Scope decision: one tournament per
server instance" section of [`multi-referee-internet.md`](multi-referee-internet.md),
which explicitly deferred this as V2. Also gates the FESA ratings proxy
behind the admin password (added after the initial design, alongside
tournament creation — see §3.1).

Goal: one running `osp-server` (hosted, or embedded in the Tauri desktop app)
holds **several** tournaments at once; each client picks which one to connect
to, instead of the server always exposing a single implicit "the tournament".

Decided while scoping this doc:

- **Applies to both modes**, hosted/remote *and* the Tauri desktop app. The
  desktop app moves from "the app has one open tournament, backed by an
  `.osp` file you explicitly open/save" to the same registry + picker as the
  browser build; native file open/save become **import/export** actions on
  top of the registry rather than the primary workflow.
- **Per-tournament password.** Each tournament gets its own optional
  password, set at creation. A referee with tournament A's password cannot
  read or edit tournament B. (Rejected alternative: one instance-wide
  password for every tournament on a host — simpler, but means anyone with
  access to any one tournament on a shared host can see/edit all of them.)
- **A separate global admin password gates *creating* tournaments, in V1.**
  One instance could plausibly end up shared across a whole federation with
  its URL circulating on random Discord/forum links for years — creation
  needs to be locked down from the start, not added once abuse shows up. See
  §3/§6.
- **Deleting a tournament is in V1** (keeping the picker list tidy is worth
  the small extra endpoint).
- **No migration handling.** Nothing here needs to read an old
  `OSP_DATA_FILE`-shaped deployment — that's fine to handle by hand (or not
  at all) once, whenever V1 actually ships. Not designed, not needed.

`osp-core` is **not touched** — see §1, the key enabler is that
`Tournament` already carries a stable `id: Uuid` we can reuse directly.

---

## 1. The key fact that shapes this design

`crates/core/src/tournament.rs` already gives every `Tournament` a stable
`id: Uuid`, assigned once in `Tournament::new` and never changed. Server-side
backups (`crates/server/src/backup.rs`) already key their on-disk directory by
this id (`backups/{tournament.id}/`) — the backup system has been
unknowingly multi-tournament-ready since it was written.

So the registry key is just `Tournament.id`. No new id type, no osp-core
change, no migration of existing save files' shape.

---

## 2. Server data model (`osp-server`)

### 2.1 Today

```rust
pub struct TournamentStore {
    current: Option<Tournament>,
    history: Vec<Tournament>,          // undo stack
    persist_path: Option<PathBuf>,     // OSP_DATA_FILE
    version: u32,
    notifier: broadcast::Sender<u32>,
}

pub struct AppState {
    pub store: Arc<RwLock<TournamentStore>>,
    pub ratings: Arc<RwLock<Option<CachedRatings>>>,
    pub auth: Option<AuthConfig>,      // one password for the whole server
}
```

(`auth`/`version`/`notifier` are on the `feat/multi-referee-internet` branch,
not yet on `main` — this design builds on top of that branch and assumes it
lands first, or the two get merged together.)

### 2.2 Proposed

`TournamentStore` is barely touched — it already holds exactly the state
that must now exist *per tournament* (current + undo history + persistence +
version + change notifier). It just stops being a singleton:

```rust
/// One tournament's live state plus its own access control.
pub struct TournamentInstance {
    pub store: RwLock<TournamentStore>,
    /// `None` = open, no password (local/embedded mode; or a referee chose
    /// not to set one). `Some` = per-tournament shared password + token.
    pub auth: Option<AuthConfig>,
}

/// All tournaments known to this server process.
#[derive(Default)]
pub struct TournamentRegistry {
    instances: RwLock<HashMap<Uuid, Arc<TournamentInstance>>>,
}

pub struct AppState {
    pub registry: Arc<TournamentRegistry>,
    pub ratings: Arc<RwLock<Option<CachedRatings>>>,   // unchanged, global (FESA list isn't per-tournament)
    pub data_dir: Option<PathBuf>,                       // replaces `OSP_DATA_FILE`
}
```

Two lock levels, matching how often each changes:

- The **registry** `RwLock<HashMap<...>>` is only written on create (and, if
  we add it later, delete) — every read/write/mutate/undo/round/etc. request
  for an *existing* tournament only needs a **read** lock on the registry (to
  look up the `Arc<TournamentInstance>`) plus a lock on that one instance's
  own `store`. Two referees editing *different* tournaments never contend on
  the same lock — an improvement over a single global lock, not just a
  refactor.
- `TournamentStore`'s internals (`mutate`, `undo`, `version`, `persist`,
  `bump_and_notify`) are unchanged; `persist()` just needs a path derived from
  the tournament's own id instead of a single fixed `OSP_DATA_FILE`.

`AuthConfig` (from the auth branch) is generic enough to reuse as-is — it's
already "one password + one random per-boot token"; we just construct one per
tournament instead of one for the whole process.

### 2.3 Persistence layout

Replace `OSP_DATA_FILE` (one file) with `OSP_DATA_DIR` (one file per
tournament):

```
$OSP_DATA_DIR/
  <tournament-id-1>.json
  <tournament-id-2>.json
  ...
```

Each file wraps the `Tournament` with the bits that live outside osp-core:

```rust
#[derive(Serialize, Deserialize)]
struct PersistedTournament {
    tournament: Tournament,
    /// Argon2/bcrypt hash, never the plaintext. `None` = no password.
    password_hash: Option<String>,
}
```

(Password as a **hash**, not plaintext, even though the threat model here is
low-stakes — it costs one dependency and it's simply the correct thing to do
once passwords are written to disk, unlike the existing shared-password
scheme which only ever lived in an env var/process memory.)

On boot: scan the directory, `PersistedTournament::tournament.id` is the
registry key, construct one `TournamentInstance` per file — eager load-all,
final decision, not a placeholder: fine at the scale one host actually runs
(a handful of tournaments), no lazy-loading complexity needed. A file that
fails to parse is skipped with a warning, exactly like today's single-file
`load_from_disk`.

**Local/embedded (Tauri) mode**: the desktop app already resolves an
app-local data directory for backups via `dirs::data_dir()` — point
`OSP_DATA_DIR`'s embedded-mode equivalent at
`dirs::data_dir()/openshogipairings/tournaments/`, so the picker's list
*is* "the tournaments you've created on this machine," and it survives
app restarts without the user ever touching a file dialog.

**Deleting a tournament** (`DELETE /api/tournaments/{id}`, V1): removes the
registry entry, deletes `{id}.json` from `OSP_DATA_DIR`, and deletes its
backups directory (`backups/{id}/`, see `backup.rs`). Requires that
tournament's own auth (its password, or open access if it has none) — not
the global admin password, since the people who'd want to clear out a
tournament are its referees, not necessarily whoever created it.

---

## 3. Routes

Everything currently at `/api/tournament/*` (singular, implicit) moves to
`/api/tournaments/{id}/*` (plural, explicit), same shape otherwise —
`/players`, `/rounds`, `/settings`, `/backups`, `/undo`, `/draft`, `/events`,
per-tournament `/login`, etc. New routes for the registry itself:

| Method | Path | Auth | Notes |
|---|---|---|---|
| `GET` | `/api/tournaments` | none | List `{ id, name, has_password }` for every known tournament — enough to render the picker, no tournament content. |
| `POST` | `/api/admin/login` | — | `{ password }` → `{ token }`, exchanging `OSP_ADMIN_PASSWORD` for the admin bearer token. 404 if no admin password is configured. |
| `POST` | `/api/tournaments` | admin token | `{ name, password? }` → creates, persists, returns `{ id }` (and a token immediately if a password was given, so the creator doesn't have to log in to their own tournament). See §3.1. |
| `GET`  | `/api/ratings`, `POST /api/ratings/refresh` | admin token | The FESA ratings proxy — an instance-wide resource, not per-tournament data, so it shares the admin gate rather than any tournament's password. See §3.1. |
| `DELETE` | `/api/tournaments/{id}` | that tournament's own token | Removes the registry entry, its persisted file, and its backups directory. 404 if `id` unknown. |
| `POST` | `/api/tournaments/{id}/login` | — | Existing shape, scoped. 404 if `id` unknown, no-op/200 with no token needed if that tournament has no password. |
| `GET` | `/api/tournaments/{id}/events` | none (SSE can't send auth) | Same as today, scoped to one instance's broadcaster. |
| everything else | `/api/tournaments/{id}/...` | per-instance token | Existing handlers, just nested + reading from the looked-up `TournamentInstance` instead of the global `AppState.store`. |

### 3.1 Global admin password (creation + ratings gate)

`POST /api/tournaments` is the one endpoint that has to be reachable before
anyone holds any tournament's password — so on a host whose URL has leaked
beyond the referees who were meant to have it (very plausible over a long
enough timeline for a shared federation instance), it's the one place someone
with no legitimate access could still spam the server with junk tournaments
and fill disk. The FESA ratings proxy (`/api/ratings*`) has the same problem
from a different angle: it's an instance-wide resource, not scoped to any one
tournament, and left open it lets a stranger use this server as a free relay
to fetch/refresh the FESA list — so it's gated the same way rather than by
any individual tournament's password (which a stranger, by definition,
doesn't have either).

Fix: a separate, single, instance-wide `OSP_ADMIN_PASSWORD` env var, exchanged
for a bearer token via `POST /api/admin/login` (mirroring per-tournament
login) — reusing the existing `AuthConfig` shape as one more instance
singleton, not per-tournament. That token then gates both `POST
/api/tournaments` and `/api/ratings*`; everything else about a created
tournament is exactly the per-tournament-password model from §0. Unset (e.g.
local/embedded Tauri mode, or a throwaway dev server) disables the check on
both, matching how `OSP_PASSWORD` already behaves today for the whole API.

Axum-wise: nest the protected routes under `/api/tournaments/{id}` so a
`Path<Uuid>` extractor is available to a middleware chain applied via
`route_layer` on that nested router (needed to look up the right
`TournamentInstance` before the existing `auth::require_auth` and
`live::check_version` middlewares run — both become "resolve the instance
from `id`, 404 if missing, then do what they do today against that
instance" instead of reading `state.store`/`state.auth` directly).

Handlers stop taking `State<AppState>` and reading a global store; instead
they take `State<AppState>` + `Path<Uuid>`, resolve
`registry.get(id).ok_or(ApiError::NotFound)?`, then operate on
`instance.store` exactly as today. This is the bulk of the "huge" server
change — every handler in `tournament.rs`, `backup.rs`'s call sites, and the
version/auth middlewares gets threaded with one extra parameter — but each
individual change is mechanical, not risky.

---

## 4. Frontend

"Moderately big" because it's one new screen plus threading one id through
an existing client, not a rewrite.

- **New `TournamentPicker.svelte`**: fetches `GET /api/tournaments`, shows a
  list (name + lock icon if `has_password`) plus a "create tournament" form
  (name + optional password). Selecting or creating one sets the app into
  that tournament's context.
- **`currentTournamentId` store** (`lib/session.ts`, alongside the existing
  `authRequired`/`connectionStatus`): `writable<string | null>`, persisted to
  `localStorage` (like the auth token today) so a page refresh stays on the
  same tournament instead of bouncing to the picker.
- **`App.svelte`**: renders `TournamentPicker` when `currentTournamentId` is
  null, else the existing tab shell scoped to that id. Add a "switch
  tournament" action (e.g. next to the connection-status badge) that clears
  the id, tears down the SSE subscription, and returns to the picker.
- **`api.ts`**: today every call hardcodes a path like `/api/tournament/players`
  against a single fixed base. Centralize the change: one helper builds
  `/api/tournaments/{id}` from `currentTournamentId` and every existing call
  site keeps its relative suffix (`/players`, `/rounds`, …) unchanged —
  avoids touching dozens of call sites individually. Things that must become
  **per-tournament-id-keyed** instead of single global values:
  - `knownVersion` (the optimistic-concurrency counter) — reset on switch.
  - the session token (`session.ts`'s `TOKEN_KEY`) — becomes
    `osp_auth_token:{id}`, since each tournament has its own password/token.
  - the SSE subscription — close the old `EventSource`, open a new one against
    `/api/tournaments/{id}/events` on switch.
- **Login overlay**: already exists (Phase 1 of the multi-referee doc); just
  needs to trigger per-tournament instead of globally (a 401 while tournament
  A is open shouldn't force a password for tournament B later).

### 4.1 Tauri desktop specifics

- The embedded server (already started in-process on an OS-assigned port)
  gets the same `OSP_DATA_DIR`-equivalent wired to a local app-data path (see
  §2.3) — no config needed from the user, just a fixed local directory.
- Native "Open file…" / "Save file…" (currently the primary way a `.osp` file
  becomes "the" tournament) become explicit **import** (`PUT /api/tournaments`
  with a body read from a picked file → creates a new registry entry from
  it) and **export** (`GET /api/tournaments/{id}` → write the JSON to a
  picked file) actions, reachable from wherever a tournament is currently
  open — for sharing a tournament as a file, or backing it up outside the
  app's own data directory. The picker becomes the primary flow; file
  open/save becomes secondary.

---

## 5. What doesn't change

- `osp-core`: nothing. `Tournament.id` already exists and is exactly what's
  needed.
- `TournamentStore`'s internals (mutate/undo/version/persist/notify) and the
  domain logic in `tournament.rs` handlers: unchanged, just invoked with an
  extra id-lookup indirection.
- Backups (`backup.rs`): already keyed by `tournament.id` on disk — no change.
- The FESA ratings cache itself: stays global in `AppState` (it's a shared
  external list, not per-tournament data) — only its *access gate* changes,
  from the old whole-API password to the new admin password (§3.1).
- Sente/Phase 1–4 auth/live-sync mechanics from
  [`multi-referee-internet.md`](multi-referee-internet.md): unchanged in
  *shape* (bearer token, SSE version stream, 409-on-stale-version), just
  instantiated once per tournament instead of once per process.

---

## 6. V2 (deliberately not designed further here)

- **Renaming a tournament / changing its password** after creation:
  straightforward additions (`PUT /api/tournaments/{id}/settings`-style) if
  wanted later, not designed now.
- **Migration** from an existing single-tournament `OSP_DATA_FILE` deployment:
  not handled. Not needed until there's an actual V1 deployment to migrate
  away from; a one-off manual step (or a small script) is enough whenever
  that day comes.

---

## 7. Suggested phasing

1. **Server — done.** Registry + per-tournament `TournamentInstance` +
   `OSP_DATA_DIR` + route renesting + delete endpoint + the global admin
   password (later extended to also gate the ratings proxy, §3.1), with the
   `feat/multi-referee-internet` auth/live-sync mechanics adapted to be
   per-instance. Includes the "create two tournaments, verify they don't see
   each other's state/undo/version" isolation coverage. Axum gotcha hit along
   the way: a bare `Path<Uuid>` needs *exactly one* captured path param, so
   once nested under `/api/tournaments/{id}` every extractor (including the
   `TournamentCtx` resolving the instance itself) had to switch to
   named-field structs that pick their field by name and ignore the rest.
2. **Frontend (browser/hosted) — done.** `TournamentPicker.svelte` (list +
   create + delete), `currentTournamentId` (`session.ts`, persisted) +
   per-tournament token storage + a separate admin token, `api.ts` scoping
   every call off the open tournament and resetting version/SSE on switch,
   `App.svelte`'s old create/load screen replaced by the picker plus a
   "Switch tournament…" action. Verified end-to-end in the browser (both
   plain and password-protected tournaments, the login gate on a session with
   no stored token, switching, deleting).
3. **Tauri — done.** The embedded server now boots via `serve_with_config`
   with `data_dir` pointed at `dirs::data_dir()/openshogipairings/tournaments/`
   (a sibling of the existing `backups/` directory, same base as `backup.rs`
   already uses) instead of the in-memory-only `serve()` — so tournaments
   created in the desktop app persist across restarts, no admin password (matches
   local mode staying open, like `OSP_PASSWORD` before it). The picker and
   file-based import/export (create-from-file in the picker, "Save" in the
   open tournament) needed no Tauri-specific frontend changes — they already
   went through the same platform-abstracted `tournamentFile.ts` from phase 2.
