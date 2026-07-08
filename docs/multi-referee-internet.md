# Multiple referees on one tournament, over the internet — design

Status: **Phases 1–3 landed.** Phase 1: shared-password auth (server middleware +
`POST /api/login`, `OSP_PASSWORD`, client login overlay). Phase 2: the standalone
server can serve the built SPA same-origin (`OSP_STATIC_DIR`), persist the
tournament through to disk and reload on boot (`OSP_DATA_FILE`), and bind a
configurable address (`OSP_BIND`); a `deploy/` recipe (Docker, systemd, Caddy)
ties it together with TLS. Phase 3: live sync via an SSE stream
(`GET /api/tournament/events`) that pushes a monotonic `version` on every change
so clients refetch, plus optimistic-concurrency guarding — clients echo the
version in `X-Tournament-Version` and a stale edit is rejected 409. Phase 4:
flaky-connection resilience — a connection-status indicator (Live / Reconnecting
/ Offline) driven by the SSE stream, and auto-reconnect + resync (refetch on
every SSE (re)connect and on tab focus/visibility, re-adopting the server's
version so a restarted server doesn't strand clients). The "safe writes while
degraded" item was already covered by the Phase 3 version/409 guard, and
optimistic offline editing was deliberately left out. **All four phases are
now implemented.** Supersedes the two `TODO.md` lines "Add authentication to the
distributed instance" and (partly) the webhook item.

Goal: let several referees edit **one** tournament simultaneously, from
different machines, **over the public internet**, safely (only referees, and
only over an encrypted channel) and ergonomically (no IP-address fiddling, no
rebuilds, live updates, tolerant of flaky venue wifi).

Scope markers: **V1** = the first shippable slice of a phase; **V2** = deferred.

---

## 0. Why a central server (the topology decision)

The instinct for "networking between machines" is peer-to-peer or
one-referee-hosts-the-others. **That does not work reliably over the internet**,
and especially not at a tournament venue:

- Venue wifi is hostile and often a phone hotspot. A phone on mobile data sits
  behind **CGNAT** — it has no reachable public IP and cannot accept inbound
  connections. Neither can a laptop on most guest wifi. So no referee machine is
  a dependable server for the others.
- Even on a cooperative LAN, "connect to `192.168.x.y:3000`" means chasing IPs
  that change per venue, per reboot, per hotspot — the opposite of ergonomic.

The reliable answer is a **small always-on server with a stable public HTTPS
URL** (a cheap VPS, or any host with a domain). Every referee — laptop or phone
— connects to it as a *client*. This is the only topology that survives NAT,
hotspots, and roaming, so the whole design is built around it.

This is a genuine shift from today's model, where the app **is** the server
(the Tauri desktop app embeds `osp-server` in-process on a random loopback port;
see [`crates/server/src/lib.rs`](../crates/server/src/lib.rs)). We keep that
mode and add a second one:

- **Local mode** (unchanged): Tauri desktop app, embedded server on loopback,
  offline, single machine. No auth, no TLS — nothing leaves the box.
- **Remote mode** (new): a hosted `osp-server` that referees reach from a
  browser. Auth + TLS required.

The same server code serves both; the difference is configuration, not a fork.

### Scope decision: one tournament per server instance

The server already holds exactly one tournament as its single source of truth
(`TournamentStore.current` in [`state.rs`](../crates/server/src/state.rs)). We
keep that for V1: **one hosted instance = one live tournament**, and the
instance password *is* the tournament password. Running two tournaments at once
means running two instances (different subdomain/port). Multi-tournament routing
(`/api/tournaments/{id}/…` with a password each) is a real feature but a large
one — **V2**, only if demand appears. Now designed: see
[`multi-tournament.md`](multi-tournament.md).

---

## 1. Phase 1 — Authentication (shared password) + TLS

The "safely" half. Threat model: keep everyone who isn't a referee out; assume
the handful of referees mutually trust each other. No per-user accounts, no
spectator/read-only tier (the user confirmed spectators are not needed) — **one
shared password gates the entire API**. This matches the earlier decision to
avoid OAuth.

### 1.1 Server: a shared-secret bearer token

- The server boots with a password supplied out-of-band — **CLI flag / env var
  / stdin prompt**, never baked into the `.osp` save file (save files get
  emailed around; a password inside one leaks instantly).
- Add `POST /api/login { password }` → returns an opaque **session token** (a
  random per-boot value; no need for JWTs). Compare the submitted password with
  a **constant-time** check (`subtle` / `constant_time_eq`) to avoid a timing
  oracle.
- An axum middleware layer (`tower` `from_fn`) requires
  `Authorization: Bearer <token>` on **every** route except `/api/health` and
  `/api/login`. Missing/bad token → `401`.
- Make it optional so local mode is unaffected: thread an
  `Option<AuthConfig>` into `router()`
  ([`lib.rs`](../crates/server/src/lib.rs)). Tauri passes `None` (loopback-only,
  in-process — auth there is pure friction); the standalone binary passes
  `Some` when a password is configured. This is a small change to the one router
  builder.
- Cheap brute-force protection on `/api/login`: a small fixed delay on failure
  and/or a per-IP attempt cap. The password should be long enough that this is
  belt-and-suspenders.

### 1.2 Client: a login screen

- On `401` (or first launch in remote mode), show a password prompt; on success
  store the token in `localStorage` and send it on every request. A `401` from
  any later call bounces back to the prompt.
- Centralize this in [`api.ts`](../frontend/src/lib/api.ts): the existing
  `fetchOk` helper is the one place to inject the `Authorization` header and to
  catch `401` → clear token → emit a "needs login" event.

### 1.3 TLS

Over the internet the password and the whole tournament travel in cleartext
without TLS — non-negotiable here. **Do not add TLS to axum directly.** Put the
server behind a reverse proxy that terminates TLS:

- **Caddy** is the recommendation — automatic Let's Encrypt certificates for a
  domain in ~3 lines of config, HTTP→HTTPS redirect included.
- The proxy also lets us keep axum bound to loopback on the host and expose only
  443 to the world.

**Deliverable of Phase 1:** a referee can run the standalone server with a
password, put Caddy in front, and other referees must log in over HTTPS before
touching anything. (Ergonomics of *reaching* it come next.)

---

## 2. Phase 2 — A hostable server (deploy, persist, serve the app)

Phase 1 secures the server; Phase 2 makes running one on the internet turnkey
and durable.

### 2.1 Serve the single-page app (SPA) from the server (same-origin)

Today the frontend is a separate Vite bundle — a **single-page app (SPA)**, i.e.
the whole UI is one HTML/JS bundle that talks to the API over `fetch` — and its
API address is the **build-time** `VITE_API_BASE`
([`api.ts`](../frontend/src/lib/api.ts)), so a referee cannot just "open the
app." Instead, have axum serve the built SPA as static files (`tower-http`
`ServeDir` with a fallback to `index.html`), alongside `/api/*`.

Payoff — this is the ergonomic core of remote mode:

- Referees open **one HTTPS URL** in **any browser**, laptop or phone. No
  install, no rebuild, no IP configuration.
- Frontend and API share an origin, so the **permissive CORS** layer
  ([`lib.rs:36`](../crates/server/src/lib.rs)) — flagged in-code as "lock down
  before exposing this beyond the machine" — can be dropped for remote mode.
- `VITE_API_BASE` becomes `""` (same-origin relative URLs) for the hosted
  build; the Tauri build keeps asking the embedded server for its port.

### 2.2 Durable persistence

Right now the live tournament exists **only in memory**
(`TournamentStore.current`); only coarse round-transition snapshots reach disk,
rotating at 10 ([`backup.rs`](../crates/server/src/backup.rs)). A hosted server
that restarts mid-round (deploy, crash, VPS reboot) would lose everything since
the last transition. For an always-on server we need:

- **Load on boot** from a configured data file, if present.
- **Autosave on every mutation** (write-through in `TournamentStore::mutate` /
  `set_current` / `undo`), atomically (temp file + rename). The existing
  rotating backups stay as the coarse recovery layer on top.
- A configurable data directory (defaults to `dirs::data_dir()` as backups
  already do).

### 2.3 Configuration & deployment recipe

- All knobs via env/CLI: bind address (default `0.0.0.0` behind the proxy;
  loopback for the embedded case), port, password, data dir.
- Ship a deployment story: a single static Linux binary + a `systemd` unit **or**
  a small `Dockerfile`, plus the Caddyfile from §1.3 and a note on pointing a
  (sub)domain at the host. `osp-server` is already a self-contained binary, so
  this is packaging + docs, not new runtime code.

**Deliverable of Phase 2:** "spin up a tournament server" is a documented
~10-minute operation, referees join by opening a URL and logging in, and the
tournament survives a restart.

---

## 3. Phase 3 — Live sync & concurrency safety

By here, multiple referees *can* work over the internet, but they still load the
tournament once on mount ([`App.svelte`](../frontend/src/App.svelte)) and never
refresh — B doesn't see A's result until a manual reload — and the shared undo
stack is a footgun. This phase makes concurrent editing actually correct and
live. Two independent pieces:

### 3.1 Optimistic concurrency (prevent lost updates)

Two referees editing off the same view can silently clobber each other, and the
**global linear undo** ([`state.rs:16`](../crates/server/src/state.rs)) makes it
worse: B's "undo" pops A's last edit.

- Give the tournament a monotonically increasing **version** (bump it in
  `mutate`/`undo`/`set_current`); ride it on the existing
  `{ tournament, can_undo }` envelope.
- Mutations carry the base version the client saw (an `If-Match`-style header or
  body field). If the server's version has moved on, reject with **`409
  Conflict`**; the client refetches and either retries or tells the referee.
  This turns silent clobbers into visible, recoverable conflicts.
- Reframe undo honestly as **global, shared, live** ("undo the last change,
  whoever made it") rather than attempting per-referee undo on shared state,
  which is hard and low-value here. With live updates (§3.2) a shared history
  behaves like a collaborative document's — acceptable for cooperating referees.

### 3.2 Push updates (Server-Sent Events)

- Add `GET /api/tournament/events`, an **SSE** stream. On every mutation the
  server broadcasts either the new state (reuse the `TournamentView` envelope)
  or a lightweight "changed, version N" ping that clients react to by
  refetching.
- SSE over websockets: updates are one-directional (server→client; mutations
  keep going over plain REST), SSE rides ordinary HTTP so it sails through the
  TLS reverse proxy with no extra config, and it **auto-reconnects** natively —
  which feeds directly into Phase 4. A `tokio::sync::broadcast` channel in
  `AppState` fans out to connected clients.
- Client: subscribe on mount, apply pushes through the existing `apply(res)`
  path so every open tab stays current within a second of any change.

**Deliverable of Phase 3:** referees see each other's results and pairings live,
and concurrent edits produce a clear conflict rather than a lost update.

---

## 4. Phase 4 — Resilience on flaky connections

The venue-wifi reality: connections drop, hotspots stall, laptops sleep. The
hosted server is always the source of truth, so recovery is mostly client-side
robustness.

- **Connection-status indicator.** A visible online/offline/reconnecting badge,
  driven by SSE stream health + request failures, so a referee knows whether
  what they see is live. (`fetchOk` already models a network failure as
  `ApiError(status 0)` — surface it instead of swallowing it.)
- **Auto-reconnect & resync.** SSE reconnects on its own; on reconnect, refetch
  the full tournament so a client that missed pushes while offline catches up in
  one shot. Same on tab `focus`/`visibilitychange`.
- **Safe writes while degraded.** With the §3.1 version check, a write composed
  against stale state is *rejected*, not silently lost — so a referee coming
  back from a dropout is told to refresh rather than clobbering newer results.
- **V2 — optimistic/offline editing.** Queue mutations locally and replay on
  reconnect. Tempting, but it reintroduces real merge conflicts on shared state;
  defer unless dropouts prove disruptive in practice. The V1 stance is "writes
  need the server; reads degrade gracefully."

---

## 5. Suggested cut & sequencing

The phases are ordered by dependency, but the **minimum viable internet
tournament** is Phases 1–2: a secured, hosted, persistent server that referees
open in a browser and log into. That alone delivers multi-referee-over-internet
*safely* — the only rough edge is needing a manual reload to see others' changes.

Phase 3 (live sync + conflict safety) is what makes it feel collaborative rather
than "take turns and refresh," so it's the clear next priority. Phase 4 hardens
it for the messy-wifi reality.

Recommended order: **1 → 2 → 3 → 4**.

## 6. Settled decisions

- **Shared password, set at deploy time.** Changing it means a restart (or a
  small "set password" admin step) — acceptable for a per-tournament instance.
- **Multi-tournament is V2, not V1.** One hosted instance = one live tournament
  (§0); run a second instance for a second tournament.
- **Backups stay on the host.** Server-side backups
  ([`backup.rs`](../crates/server/src/backup.rs)) living on the VPS rather than a
  referee's machine is fine as-is; no change planned.
