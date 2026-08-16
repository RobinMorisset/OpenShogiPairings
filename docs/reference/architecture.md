# Architecture

How OpenShogiPairings is put together, as it currently is. The HTTP surface has
its own document, [`api.md`](api.md).

![The crate stack: integer-blossom at the bottom, osp-core on it, osp-server and
osp-sim side by side above, the frontend on osp-server. Everything but osp-sim is
packaged into the Tauri executable.](architecture.svg)

| Piece | Location | Tech | Role |
|-------|----------|------|------|
| Domain / pairing engine | [`crates/core`](../../crates/core) (`osp-core`) | Rust | Correctness-critical logic + shared DTOs. Reused by every client. |
| Matching solver | [`crates/matching`](../../crates/matching) (`integer-blossom`) | Rust | Standalone blossom (min-weight perfect matching) solver, self-contained and dependency-free. |
| HTTP server | [`crates/server`](../../crates/server) (`osp-server`) | Rust + axum | Multi-tournament source of truth; exposes the API, both as a standalone binary and as a library. |
| Simulation CLI | [`crates/sim`](../../crates/sim) (`osp-sim`) | Rust | Monte-Carlo comparison of pairing-settings variants — links `osp-core` directly and drives its pairing/scoring loop in-process, parallelized. See [`docs/guides/simulation-cli.md`](../guides/simulation-cli.md). |
| Web UI | [`frontend`](../../frontend) | TypeScript + Svelte 5 + Vite | Browser client; also the frontend embedded by Tauri. |
| Desktop app | `frontend/src-tauri` | Tauri 2 (Rust + system webview) | Self-contained app: **embeds the server** (`osp-server` as a library) and runs it in-process. |

The `osp-server` crate is both a standalone binary (browser dev, and the hosted
remote deployment — see [`docs/archive/multi-referee-internet.md`](../archive/multi-referee-internet.md))
and a library. The desktop app links the library and starts the API in-process
on an **OS-assigned port** (bound to `127.0.0.1:0` to avoid clashes and
firewall prompts); the frontend asks Rust for that port via the `api_base`
command. This is what lets the packaged app ship as a single self-contained
executable.

## Multi-tournament registry, and per-tournament access control

The server holds **any number of tournaments** at once, keyed by each
`Tournament`'s stable `id: Uuid` — a `TournamentRegistry`
([`crates/server/src/state.rs`](../../crates/server/src/state.rs)) mapping ids to
`TournamentInstance`s, each wrapping its own `TournamentStore` (current state +
undo history) and its own optional password. Clients start at a **picker**
(`GET /api/tournaments`, rendered by `TournamentPicker.svelte`) and open one
tournament at a time.

Auth is two-level, both a shared-password-plus-bearer-token model (see
[`crates/server/src/auth.rs`](../../crates/server/src/auth.rs)):

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

## Public read-only access

A tournament can be made public two ways — a **live link** served by this
server, or a **web page exported to a file** — and the players in the room, and
anyone following from home, then see the standings and the pairings on their
phones with no password and no ability to change anything. Both live behind the
*Public page…* button in the toolbar; both are off until the referee acts.

**The live link.** Publishing mints a **capability key** and gives the referee a link —
`/t/{id}/public?k=<192 random bits>` — shown as a QR code, with a *Print QR
code* button that lays out one sheet (name, code, link) for the playing room.
The code is inline SVG so it scales to the page without resampling, and it is
deliberately never themed: reflectance reversal is optional in the QR spec, so
a light-on-dark code is unreadable to a fair number of phone scanners. It is not a
boundary against a determined attacker (the link will be photographed and
forwarded, which is fine: the content is meant to be seen) but against
*accidental* discovery, so an abandoned tournament doesn't sit in a search
engine with forty people's names in it. Publishing again rotates the key,
revoking every link already handed out, independently of the tournament
password.

**The exported pages.** *Export web pages…* writes the same standings and
pairings out as ordinary web pages — one per tab, so the standings and each
round are their own file, linked to each other and needing no server, no
scripts and no external request. The referee picks a folder, uploads its
contents wherever their club already has a website, and exports again after
each round. This is what serves the **desktop app**, whose embedded server
listens on a random loopback port that nothing in the room can reach; there the
live link above is not offered at all.

Four things about those files are decisions rather than defaults. Their bodies
are produced by mounting the very same components the live page uses and
serialising the DOM they build, so a second renderer cannot drift away from the
first. Every control is dropped, since none would work — and the tab strip
becomes real `<a href>` links, which is what it was imitating anyway, so the
pages must stay side by side in one directory. The app's floating tooltips
become CSS ones, with each anchored left, centre or right at export time
according to where its cell sits, because CSS alone cannot keep a tooltip on
the last column from hanging off the page. And every page states when the
snapshot was taken, because a page that has quietly gone stale is this
transport's failure mode. They are written `noindex` by default — they outlive
the tournament by years on somebody's web server.

What readers get, by either route, is a projection — `PublicTournamentView`
([`crates/server/src/public.rs`](../../crates/server/src/public.rs)) — of the very
same `TournamentView` the referee sees, so the public table can never disagree
with theirs. Two things are dropped: the referee's own session state, and
`Tournament::draft`. That second one *is* the timing rule: the round being
hand-tuned is never public (a pairing that gets discarded must never have been
visible), while every result becomes public the instant it is recorded,
board by board. The conversion destructures both structs by value naming every
field, so adding a field to `Tournament` fails to compile until someone decides
whether it is public — fail-closed on schema growth.

Enforcement is structural, not cosmetic: the reader routes are their own router
group with no mutating handler in it, mounted outside the auth middleware, so
no bug in credential handling can escalate a reader into a writer. The
solver-invoking `/rounds/{n}/counterfactual` is deliberately never public —
two O(N³) solves per unauthenticated request is a one-line denial of service
against a laptop in the middle of a tournament.

A room of phones is a different load from three referees' laptops, so: the
payload is serialized once per version and shared by every reader, the plain
`GET` carries an `ETag` (making the end-of-round refresh herd cheap), the SSE
stream pushes the *payload itself* rather than the version — which removes the
refetch fan-out entirely, making SSE cheaper here than polling — reader clients
reconnect with jittered backoff so a wifi blip doesn't bring the room back in
one instant, and concurrent public streams are capped per tournament.

Full design, including the webhook phase still to come, in
[`docs/archive/public-access.md`](../archive/public-access.md).

## Where the files live

Two directories, configured independently by environment variable — the desktop
app reads the same two, so one name means one thing everywhere. These are the
two that matter to how the software behaves; for the full set the server reads
(`OSP_BIND`, `OSP_STATIC_DIR`, `OSP_ADMIN_PASSWORD`, the backup retention),
with defaults, see [`deploy/README.md`](../../deploy/README.md) — which is the
one table, not a copy of one.

| Variable | Holds | Default (desktop app) | Default (`osp-server`) |
| --- | --- | --- | --- |
| `OSP_DATA_DIR` | One `{id}.json` (+ `{id}.auth.json`) per tournament. | `<data dir>/openshogipairings/tournaments` | *(unset = in-memory only)* |
| `OSP_BACKUP_DIR` | One directory of rotating automatic backups per tournament. | `<data dir>/openshogipairings/backups` | same |

`<data dir>` is the per-user data directory (`~/Library/Application Support` on
macOS, `%APPDATA%` on Windows, `$XDG_DATA_HOME` on Linux). They are separate
knobs on purpose: the backups are the recovery copy, so putting the live
tournaments on a synced folder while the backups stay on the local disk (or the
reverse) is a reasonable thing to want. Both are logged at startup, and the
backups path is also shown in the app — in the Backups button's tooltip and at
the top of its panel.

**Deleting a tournament does not delete its backups.** A final backup, labelled
*deleted*, is taken of the state it was deleted in; the directory is then marked
with a `deleted.json` (when, under what name, and the password hash if it had
one) and kept for `OSP_BACKUP_RETENTION_DAYS` — 30 by default, `0` to delete it
outright as older versions did. Marked directories past their retention are
removed at startup and after each deletion; a directory with no marker belongs
to a live tournament and is never touched.

The picker lists what is in there under **Recently deleted**, with a *Restore*
button per entry, optionally naming which backup to come back at. It comes back
as *itself*: same id, same backups directory with its whole history, marker
dropped — indistinguishable from a tournament that was never deleted, which is
why it leaves the list. Going further back afterwards is then the ordinary
Backups panel, not the bin. A protected tournament's hash outlives its
`{id}.auth.json` on purpose: restoring one demands the password it had, and
gives it back that same password — deleting is not a way to unlock a tournament.
Its old session tokens do not survive, since restoring builds fresh auth.

## Live multi-referee sync and conflict detection

Several referees can edit the same tournament from different machines at
once, over the internet. Each mutation bumps a per-tournament monotonic
`version` counter; `GET /api/tournaments/{id}/events`
([`crates/server/src/live.rs`](../../crates/server/src/live.rs)) is a Server-Sent
Events stream that pushes that counter to every connected client, which then
refetches. Clients echo the version they last saw in an
`X-Tournament-Version` header on every mutation; if another referee's change
landed first, the request is rejected `409 Conflict` and the client refetches
and re-presents the edit rather than silently clobbering it (see the
`ConnectionStatus.svelte` Live/Reconnecting/Offline indicator and the
"another referee changed the tournament" reload message in the UI). See
[`docs/archive/multi-referee-internet.md`](../archive/multi-referee-internet.md) for the
full design (auth, hosted deployment, SSE sync, and reconnect resilience were
delivered as four separate phases, all now landed).

## Pairing engine

The pairing engine models a round as a **minimum-weight perfect matching** over a
complete player graph. The matching itself is an integer **blossom** solver
([`crates/matching`](../../crates/matching/src/lib.rs), general graphs —
Hungarian doesn't apply); the edge weights come from a set of Swiss rules
([`crates/core/src/pairing`](../../crates/core/src/pairing)) combined on a
priority ladder of multipliers — most important first: never rematch or repeat a
bye, give the bye to the lowest score group (penalty ∝ gap²), prefer equal
scores (penalty ∝ gap²), avoid repeating a float in the same
direction (decaying with time), select floaters (the weakest of the upper group
drops, the first — classic Swiss — or median lower-group player rises), avoid
club-mates (optional per tournament, and optionally only for the first N rounds
or with some clubs exempt), avoid compatriots (the same rule one tier weaker,
also optional), and fold each score group (top-half Nth meets
bottom-half Nth). The odd player's bye is modeled
as a phantom vertex. The multipliers are *derived* from each rule's worst-case
contribution, so the tiers are strictly disjoint by construction (lexicographic
priority) with no hand-tuned gaps; an ILP/CP-SAT backend for very large fields and
formats needing hard constraints is future work (see [TODO.md](../../TODO.md)).

Because a global optimum has no local "reason" for any one board, the engine
can also **explain itself**
([`docs/archive/pairing-explanations.md`](../archive/pairing-explanations.md)): a per-board
penalty ledger and a per-round report of which rules had to bend are **frozen
onto the round when it is confirmed**, so correcting an earlier result or a
rating can never rewrite why a later round was paired the way it was. Each round
also carries a flag saying whether its ledger still matches the present — an
edit clears it on the rounds it could have disturbed, and a round paired
afterwards is born valid again — and a round whose flag is down is shown with a
"the data behind this has changed since" warning rather than quietly. A referee can also probe a specific
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
([`crates/core/src/elo.rs`](../../crates/core/src/elo.rs),
[`docs/archive/elo-pairing-mode.md`](../archive/elo-pairing-mode.md)) build on a live Bayesian
(maximum-a-posteriori, Bradley–Terry) estimate of every player's strength from
results so far. In **mixed mode**, MacMahon and the Swiss score-group rules
(score gap, float repeat, club and nationality protection, airtight groups) are all kept, but
the fold and floater-selection rules are replaced by minimizing the squared
estimated-ELO gap — so a score group is still formed exactly as in Swiss, and
only the ordering _within_ (and across) it follows current form instead of the
static registration rating; it stays fully compatible with MacMahon points. The
**pure ELO mode** is the more extreme variant: it drops MacMahon and the
Swiss-specific rules entirely, collapsing the rule ladder to the estimated-ELO
gap alone — a "continuous Swiss" where winners and losers drift along a smooth
strength axis instead of jumping between integer score groups.

## The UI

The web UI organizes a tournament into tabs: **Settings** (MacMahon groups,
degressive schedule, club and nationality protection, floater style, pairing
mode, hybrid cup),
**Players**, **Standings** (per-round results plus Wins and total Points),
and one tab per round. Points are each player's wins plus their MacMahon
starting points (one per threshold they reach — an ELO rating or a dan/kyu
grade), and the pairing engine scores by total points. Wins are counted in
halves, following the EGF "number of wins" convention: a half-point sit-out
(`0=`) is worth half a point *and* half a win, so the Wins column and the
Points column never disagree about the same round, and the tie-breaks summed
from wins (SOSW, SODOSW, SOSOSW, CUSSW) stay exact rather than rounding the
half away. The round lifecycle is
gated: **prepare round** (a draft state to mark players absent, force pairings,
and force the bye — for round 1 this also finalizes registration in the same
step) → **start round** (confirm) → play games → the round **completes
automatically** once every board has a result (or no-show), unlocking the next
round; **cancel last round** peels back one stage (discarding an open draft,
else removing the most recent round) to replay it or undo a mistake.
Finalizing registration assigns each player a tournament number (by ELO,
unrated last; later additions get the next free number). The Standings tab is
a ranked table: a row per player (ordered by the referee-chosen criteria) with
one column per round that has started (`opponent-number` + `+`/`−`, or `0+` for
a bye / `0-` for an absence), a win count, and one column per selected ranking
criterion — Points plus fourteen tie-break metrics (SOS / SODOS / SOSOS, the
Buchholz cuts, and the cumulative score, each in a MacMahon-inclusive and a
wins-only flavour; direct confrontation; and the estimated ELO in the ELO
pairing modes), reorderable in Settings. The round in progress gets its column
(marked `R3*`) as soon as it is paired, each cell filling in as its result is
recorded and showing `5?` until then; the scored columns and the ranking count
completed rounds only, so the table re-sorts in one step once the round ends.

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
