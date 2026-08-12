# Public read-only access to standings and pairings — design

Status: **phases 1 and 2 implemented** (the projection, the capability-keyed
public endpoint and its payload-carrying stream, the publication flag, the
picker audit, the read-only frontend mode, and the static HTML export — see §6).
Phase 3 is still design only. Supersedes the `TODO.md` line
"Read-only access to the pairings and standings" and covers the read half of
the "Webhook for pushing results and pulling players" line (the *pulling
players* half — HelloAsso registration import — is a separate direction and is
explicitly out of scope here, see [Non-goals](#non-goals)).

Goal: let the players in a tournament — and anyone following it from home —
see the standings and the pairings, on their phones, without a password and
without any ability to change anything.

Scope markers: **V1** = the first shippable slice; **V2** = deferred.

---

## 0. The two decisions that matter, and the one that doesn't

The instinct is to treat this as a transport question — webhooks versus an
open endpoint. It isn't. The transport is the small half. The design is two
policy decisions:

1. **What** is public (a projection of the tournament).
2. **When** it becomes public (a timing rule).

Once those exist, "serve the projection at a URL", "POST it to a webhook",
and "write it to an HTML file the referee uploads" are three cheap sinks for
the same bytes. So this document settles the projection and the timing first,
and treats the transports as interchangeable consumers of it.

The transports are not interchangeable in one respect, and it is the thing
that decides the phasing: **who has to be reachable from the internet.**

| Transport | Direction | Works on the hosted server | Works on the desktop app |
| --- | --- | --- | --- |
| Public HTTP endpoint | inbound | yes | **no** — it listens on `127.0.0.1:<random>` |
| Webhook / push to a URL | outbound | yes | yes (survives NAT, hotspots, captive portals) |
| Static export the referee uploads | outbound, manual | yes | yes |
| LAN (`0.0.0.0` + QR of the venue IP) | inbound | n/a | unreliable — client isolation on guest wifi blocks it silently |

The desktop app *is* the server (`osp-server` embedded in-process on a random
loopback port, see [`crates/server/src/lib.rs`](../crates/server/src/lib.rs)),
so the public-endpoint option does not exist for it at all. Since most
tournaments will run the laptop and not the hosted instance, an
outbound transport is eventually mandatory — but it is not what to build
first, because it needs somewhere to push *to*.

---

## 1. What is public: the projection

### 1.1 It is nearly free

Standings, the cup bracket, the podium and the effective winner of each board
are **already computed server-side** and already shipped on every response —
`TournamentView` in
[`crates/server/src/tournament.rs`](../crates/server/src/tournament.rs), built
from [`crates/core/src/standings.rs`](../crates/core/src/standings.rs), which
exists precisely so "every client shares one ranking". The frontend only
formats them ([`frontend/src/lib/tiebreak.ts`](../frontend/src/lib/tiebreak.ts)
computes tooltip breakdowns, not the values).

So the public payload is not a new computation and carries no risk of the
public table disagreeing with the referee's. It is `TournamentView` with a
few fields removed.

### 1.2 The projection, field by field

`PublicTournamentView` = `TournamentView` minus:

| Dropped | Why |
| --- | --- |
| `can_undo` | referee session state, meaningless publicly |
| `draft_cup_players` | derived from the draft (see §2) |

and, conditionally:

| Conditional | Rule |
| --- | --- |
| `suggested_handicaps` | published when `settings.handicap_policy` is `Enabled { display: Suggested, .. }`, empty otherwise — the suggestion is primarily *for the two players*, who need to know what to set up on the board before they start. When the referee chose `Allowed` (picker, no suggestion column) they have deliberately turned the suggestion off, and the public page must not contradict the room. |

and its `tournament: Tournament` minus:

| Dropped | Why |
| --- | --- |
| `draft` | the round being hand-tuned — the whole timing rule, see §2 |

Everything else is published as-is: `id`, `name`, `settings`, `players`,
`registration_finalized`, `rounds`, `cup`, plus `version`, `standings`,
`cup_podium`, `cup_bracket`, `effective_winners`, `team_matches` (the reader's
round page groups its boards by match exactly as the referee's does, and both
groupings come from the server).

Decided: **no field of `Player` is redacted.** Name, club, nationality,
rating, grade, cup eligibility and categories are all tournament facts a
printed wall sheet would carry anyway, and `adjustments` affect points and
therefore pairings, so hiding them would make the public standings
unexplainable.

One consequence worth handling in the UI rather than in the projection:
`PointAdjustment::reason` is *mandatory, free text, and written
referee-to-referee* today
([`player.rs:164`](../crates/core/src/player.rs)). "−2,
turned up drunk" is a reasonable thing to type into a private tool and a
libel risk on a public page. The fix is not redaction (the reason is exactly
what makes the adjustment legitimate) but a hint next to the field when the
tournament is published, so the referee knows the audience.

### 1.3 The shape of the filter

Build it as an explicit conversion that **destructures `TournamentView` and
`Tournament` by value**, naming every field, rather than a serde skip-list or
a mutate-a-clone. Then adding a field to `Tournament` is a compile error at
the projection until someone decides whether it is public — fail-closed on
schema growth, which a redaction-by-omission scheme is not. This is the whole
reason to have a projection type even while it drops almost nothing.

---

## 2. When: the timing rule

Decided:

- **Never publish the draft.** The referee hand-tunes forced pairings and the
  absent set before confirming; players must not watch that happen, and a
  pairing that gets discarded must never have been visible.
- **Publish results the instant they are recorded**, board by board, within
  the current round. Seeing which games are still running, and how the people
  who already finished did, is most of the value of a live page.

The current data model makes this rule **structural rather than a filter**:
`Tournament::draft: Option<RoundDraft>` is a separate field from
`Tournament::rounds: Vec<Round>`, and a round only enters `rounds` when it is
confirmed ([`crates/core/src/tournament.rs`](../crates/core/src/tournament.rs)).
Dropping one field is the entire policy. Nothing needs filtering inside
`rounds`, and the in-progress round shows up naturally with its undecided
boards — which is precisely the "who is still playing" view.

Two lesser cases follow from the same rule and need no special handling:

- **Before round 1**, `registration_finalized` is false and `rounds` is
  empty: the public page shows the entrant list and (with MacMahon) the
  starting points. Useful — players check they are registered.
- **Undo and backup restore** move the public state *backwards* in content.
  That is correct: the referee has decided the earlier state is the true one.
  See §4.1 for why it is not a problem for ordering.

**V2, if wanted later:** a per-round "hold publication" switch, for a referee
who wants to announce the pairings on the microphone before the room sees
them on their phones. Not V1.

---

## 3. Access control

Decided: **enforced by the server, not by the frontend.** A UI mode that
hides the mutating buttons is not access control.

Concretely: the public projection is served by its **own router tree with no
mutating handler in it**, mounted outside `require_tournament_auth`, rather
than by relaxing the existing routes. In
[`crates/server/src/tournament.rs`](../crates/server/src/tournament.rs) the
`scope()` function already splits `public` from `protected`; this is a third
group, and the property to preserve is that no handler is reachable from more
than one of them. No bug in token handling can then escalate a reader into a
writer, because there is no writer to reach.

Explicitly **not** public, now or later: `/rounds/{n}/counterfactual`. It runs
the blossom solver twice per request — a baseline matching plus a re-solve
with the edge forced or forbidden (`solve_stable` in
[`crates/core/src/pairing.rs`](../crates/core/src/pairing.rs)). An
unauthenticated endpoint that runs two O(N³) solves is a one-line denial of
service against a laptop in the middle of a tournament. The standing rule is
that the public surface is **precomputed or cheap-derived data only**, and
nothing that can invoke the solver.

The round explanation is *not* in that category, and since the appendix landed
it is a plain lookup: the ledger is frozen onto the round at confirmation, so
there is no rebuild and no faithfulness problem left. It rides along inside the
projection's `rounds` already (which is why it needs no endpoint of its own);
what is still to do is the public *UI* for it, kept out of phase 1 only to keep
that phase small.

### 3.1 Opting in

Publication is per tournament, off by default, persisted next to the password
in the `{id}.auth.json` sidecar (`crates/server/src/state.rs`) — it is
access-control state, not tournament content, and it must not travel inside
the save file a referee mails around.

`GET /api/tournaments` (the picker, `crates/server/src/registry.rs`) is
**already unauthenticated** and lists every tournament's summary. Audit it as
part of this work: today it is the only thing a stranger who finds the URL
can see, and once there is a public reader UI it becomes that UI's front door
for tournaments that never opted in. Non-public tournaments should not be
listed to an unauthenticated caller.

### 3.2 Unguessable URL, not a bare public one

Decided: **capability URL** — `/t/{id}/public?k=<192 random bits>`,
the key minted at publication and printed as a QR code on the wall.

- Nothing to type on a phone; a QR code is the actual distribution mechanism
  either way.
- Not discoverable, not indexed, so an abandoned test tournament from 2024
  does not sit in a search engine with 40 people's names in it.
- Revocable independently of the tournament password (rotate the key).
- It is *not* a security boundary against a determined attacker — the link
  will be photographed and forwarded, which is fine, because the content is
  meant to be seen. It is a boundary against *accidental* discovery, which is
  the actual risk.

The rejected alternative (a bare public URL, no key) is simpler and shareable
by voice; revisit only if wall-posting a QR code turns out to be
impractical.

---

## 4. Load and abuse

The reader population is a different animal from the referee population: 3
authenticated laptops versus 200 phones on venue wifi, hammering a laptop
that is also running the solver.

The cost is **not** the notification channel. An idle SSE connection
([`crates/server/src/live.rs`](../crates/server/src/live.rs)) is a tokio task
future, a `broadcast::Receiver` over a shared ring, an fd, and kernel socket
buffers — comfortably under 100 KB all-in, so single-digit MB for a room of
100 — and `KeepAlive::default()` costs a comment ping per client per 15s.
That is nothing.

The cost is the **refetch fan-out** the notification triggers. The stream
carries only the `version`; every client then issues a full `GET`. One
mutation therefore costs 100 serializations and 100 full-payload transfers,
and a round of result entry is a few dozen mutations. Polling has exactly the
same cost, so it is not the answer.

**V1 mitigations:**

- **Serialize once per version, not once per request.** The public payload is
  identical for every reader and changes only on a mutation.
- **Push the payload in the SSE event, for public readers.** Broadcast the
  serialized projection itself (as a shared `Arc<str>`) rather than the
  version, and the refetch disappears: one serialization and zero extra round
  trips per change, instead of 100 of each. This makes SSE *cheaper* than
  polling here, not more expensive. (The referee stream keeps sending the
  version — referees need the version for the `X-Tournament-Version` conflict
  guard, and their payload includes fields readers must not receive.)
- **`ETag` = the version; honour `If-None-Match`** on the plain `GET`. This is
  the reconnect and cold-load path, and what makes a herd of refetches cheap.
- **Jittered reconnect backoff in the reader client.** The real burst is not
  steady state: a server restart or a venue wifi blip drops every client at
  once, and phones coming out of pockets at the end of a round refetch on tab
  focus together. Without jitter the herd arrives in one instant.
- **Cap concurrent public streams per tournament, answering 503 over the
  cap.** Not a scaling measure — 100 is fine — but an abuse one: an
  unauthenticated stream anyone may open is a slow resource-exhaustion
  surface, where the referee stream was implicitly bounded by knowing the
  password.

### 4.1 Ordering, for the push transports

`version` is monotonic within a **server run** — but it starts at `0` on
every boot (`TournamentStore` in
[`crates/server/src/state.rs`](../crates/server/src/state.rs)); it is session
state and is not persisted with the tournament. A receiver that keeps "the
highest version I have seen" and drops anything lower will therefore **go
deaf after a server restart**, having latched onto version 57 and being fed
version 3 forever. This is exactly the kind of failure that looks like
nothing at all — the mirror simply stops updating, with no error anywhere.

So a push carries `{boot_id, version}`, where `boot_id` is minted once per
server run (the same place the per-boot bearer tokens are minted), and the
receiver accepts a payload when `boot_id` differs **or** `version` is
greater. With that, undo and backup restore are non-events: they bump
`version` forward like any other change, so the mirror follows the referee
backwards in content without any special case.

### 4.2 Always push the whole projection

No deltas. A dropped POST self-heals on the next one, retries are idempotent,
and out-of-order arrivals are resolved by §4.1. Deltas would need
at-least-once delivery, ordering, and a receiver-side replay log to get the
same property. The payload is a few tens of KB and a round produces a few
dozen pushes; the bandwidth is irrelevant next to the reliability.

---

## 5. The reader frontend

Same SPA, a read-only mode: the Standings tab, the current and past rounds'
pairings, and the cup bracket if any. No Settings, no Players editing, no
Backups, no undo, no "why these pairings?".

It fetches the public projection, so the mode is decided by *what the server
answered*, not by a client-side flag — a reader who edits their JavaScript
gets a broken page, not a write.

Reuse of the existing components is the reason the projection keeps the
`Tournament` shape rather than inventing a flat DTO: `StandingsView` and the
pairings table already render exactly this data, and a second shape would
mean a second renderer to keep in sync.

---

## 6. Phasing

**Phase 1 — the projection and the public endpoint (hosted server). Done.**
`PublicTournamentView` + the destructuring filter, the per-tournament
publication flag and capability key, the third router group, the
serialize-once cache with `ETag` on the `GET` and the payload-carrying SSE
stream on top of it, the picker audit, and the read-only frontend mode.
End-to-end useful on its own: QR code on the wall, done.

Landed in [`crates/server/src/public.rs`](../crates/server/src/public.rs) (the
projection, the reader routes, the publication endpoints),
[`crates/server/src/state.rs`](../crates/server/src/state.rs) (the key in the
auth sidecar, the payload cache, the stream cap, the per-boot id the `ETag`
needs), and `frontend/src/lib/components/PublicView.svelte` +
`frontend/src/lib/publicAccess.ts` on the client.

Two deviations from the text above, both deliberate:

- **The `ETag` carries the boot id as well as the version.** §4.1 introduced
  `boot_id` for the push transport, but the hazard is the same here and lands
  sooner: `version` restarts at `0` on every boot, so without it a reader
  holding `"5"` from before a restart is answered `304` for an entirely
  different state — and the page simply stops updating, with no error anywhere.
- **Revocation has to reach the connections that are already open.** §3.2 calls
  the key "revocable independently of the tournament password", and a stream
  outlives the request that opened it — so checking the key only at connect made
  rotation revoke nothing for anyone already watching: they kept receiving every
  result down a connection opened with a key that no longer existed, for as long
  as the tab stayed open. The reader stream therefore re-checks its key on
  **every** event, and `set_publication` pings the change channel so the check
  happens at the moment of revocation rather than at the next edit. The revoked
  reader is told (`event: revoked`) instead of merely dropped — a silent close
  is indistinguishable from a wifi blip, and the client would reconnect against
  it forever.

- **The picker audit needed a visible answer, not just a filter.** Hiding
  unpublished tournaments from an unauthenticated caller (§3.1) leaves a
  referee looking at a short list with no hint that there is more, which reads
  as "my tournament was deleted". `GET /api/tournaments` therefore answers
  `{ tournaments, restricted }`, and the picker turns `restricted` into an
  admin-password prompt. The filter applies only when an admin password is
  configured — the marker of a host reachable by people who are not its
  referees; a server deliberately run open is unchanged.

The QR code is generated in the app (`qrcode-generator`, zero dependencies,
MIT), with a *Print QR code* button that lays out one sheet — tournament name,
code, link — for the wall. Three things about it are decisions rather than
defaults:

- **Never themed.** Reflectance reversal (light modules on dark) is optional in
  the QR spec, so an inverted code is unreadable to a fair number of phone
  scanners. It is dark-on-white in both themes, always.
- **Inline SVG, not a bitmap.** A code that has been through a raster scale is
  exactly the thing that reads fine on the referee's screen and not from the
  wall.
- **The encoder is loaded on demand.** It is ~8 kB gzipped and only the
  referee's publication panel ever renders a code — while the *reader* page
  loads the same bundle, on a phone, on venue wifi, a hundred at a time. Making
  readers pay for it would undo a chunk of §4.

**Phase 2 — static export. Done.** The same projection written as ordinary web
pages — one per tab, needing no server — regenerated on demand, for the desktop
app to upload wherever the club already has a website. This is the only thing
that serves the laptop deployment without new infrastructure.

Landed in `frontend/src/lib/publicExport.ts` (the renderer and the document
assembly), `frontend/src/lib/components/PublicSnapshot.svelte` (one page of it),
and one referee-side route, `GET /public-snapshot`, in
[`crates/server/src/public.rs`](../crates/server/src/public.rs).

Two pieces are shared with the live reader page rather than written twice, and
between them they are the whole of what a "section" is:
`frontend/src/lib/publicPage.ts` decides **which** sections a tournament has
(standings — the entrant list before round 1 — then the cup, then the rounds),
and `frontend/src/lib/components/PublicSectionBody.svelte` renders **what** each
one shows, differing only by `staticPage`. They were two decisions made
separately once, and the two pages promptly disagreed: a cup is frozen at
finalization, so between then and the first confirmed round the live page showed
the bracket and no entrant list while the export showed the entrant list and no
bracket. Neither reader saw a superset of the other, which is precisely the
failure §5 exists to prevent.

Six decisions inside it are worth recording, three of them deviations from the
sketch above:

- **The pages are pre-rendered HTML, not inlined data plus a script.** "Data
  inlined" above assumed the SPA would ship inside the file and render it. It
  would work, and it would be ~700 kB per page (every locale has to be inlined
  too, since nothing may be fetched), against ~65–110 kB pre-rendered.
  Rendering ahead of time also buys the properties that matter for a file
  dropped onto somebody else's web server: it survives any
  Content-Security-Policy, prints, is searchable and indexable as text, and
  still shows the standings with JavaScript off. The pages needed no
  interactivity anyway — a snapshot has nothing to be live about.
- **Rendered by mounting the real components and serialising the DOM.** The
  obvious way to emit static HTML is a string-building renderer, and it is the
  thing §5 argues hardest against: a second renderer drifts, and the day it
  drifts the club's website disagrees with the referee's screen and neither
  side knows. So the export mounts `PublicSnapshot` — which renders its one
  section through the same `PublicSectionBody`, and so the same `ResultsView`,
  `RoundView` and `CupBracket`, as the live page — into an off-screen host,
  flushes, and reads back the DOM. What the components gained is a `staticPage`
  prop that drops the controls (toolbars, filters, pickers) that would otherwise
  sit in the file looking clickable; a test asserts the output contains no
  `<button>`, `<input>` or `<select>` at all. The stylesheet is collected from
  the live document the same way and for the same reason, so a rule added to any
  component is in the export without anyone remembering.
- **Tabs become one file per tab, linked to each other.** The first attempt
  stacked every section into one document, on the grounds that a tab strip is a
  script. At 45 players and two rounds that is unreadable, and it is not what a
  tab strip was ever imitating — *links between pages* is. So `publicSections`
  turns the projection into one page per tab (standings, cup, then each round
  ascending, exactly the app's order), and the strip is a row of `<a href>`
  styled like the live one, with the page you are on rendered as plain selected
  text rather than a link offering to reload what you are reading. The cost is
  that the pages of one export are only correct **together**: they reference
  each other by bare file name, so the save flow asks for a directory rather
  than a file name, and the names are fixed rather than the referee's to choose.
- **Tooltips become CSS, anchored at export time.** The tables explain
  themselves through `data-tip` — who an opponent was, how a tie-break was
  built, why a sit-out scored what it did — and the app turns that into a
  floating panel with a mousemove handler. Native `title` was the first answer
  and is genuinely worse than it sounds: a browser shows it only after about a
  second, in the OS's styling, which reads as "the tooltips are gone". So the
  export ships the app's own tooltip box as a `:hover::after` rule instead. The
  one thing that rule cannot do is the handler's viewport clamping, so that is
  done when the file is written: each tooltip is told to open left, centre or
  right from where its cell sits in its row, since in a file nothing ever moves
  again. Without it a tooltip on the last tie-break column hangs off the page
  and gives the whole document a horizontal scrollbar.
- **Not gated on the publication flag.** That flag governs *this server's*
  reader endpoint. The export's whole audience is the deployment where that
  endpoint is unreachable, so requiring publication first would mean minting a
  capability key pointing at a loopback port in order to get a file. Saving the
  files is itself the act of publishing, for this transport. The one thing the
  flag did carry that the export also needs — the warning that point-adjustment
  reasons stop being referee-to-referee prose — moved up to cover the whole
  panel.
- **Every page says when it was taken, and is written `noindex`.** A static
  page that has silently gone stale is this transport's characteristic failure,
  and a reader cannot tell an old file from a live one unless the file says so.
  It is on *every* page because a reader may be sent straight to round 2. The
  `noindex` is §3.2's reasoning applied to files that will outlive the
  tournament by years on someone's web server: putting them on the club's site
  is the referee's decision, leaving forty people's names in a search index for
  good is a different one.

Not done, and deliberately: **uploading**. The referee saves a folder and puts
it where they already put things. Automating that means credentials, a protocol
choice (FTP? WebDAV? scp? a CMS API?) and a retry story, none of which can be
designed without a real club's hosting in front of us.

**Phase 3 — push.** POST the same projection, with `{boot_id, version}`, to a
URL the club configures, on every change. The receiver is *their* site: this
is for a club that wants the standings inside their own pages rather than on
a separate one. Compatibility with pairgoth's webhook shape is a
nice-to-have, not a constraint.

Deliberately **not** part of this: a second OSP instance running as a public
mirror that the tournament pushes to. A club able to run an OSP instance can
run the tournament *on* it — that is the existing hosted deployment, and
phase 1 then gives them the public page directly. A mirror would add a
display-only server mode and an authenticated *write* endpoint on a public
host, to reach a place phase 1 already reaches.

The one thing a mirror would have bought is network independence: running the
tournament on a remote server makes every referee action depend on venue
wifi, whereas publishing outward from the laptop means the tournament never
stops and the public page merely lags. That requirement is real — but it is
satisfied by phase 2 with an automatic upload, not by a second OSP. If it
becomes pressing, extend phase 2's export with an upload target; don't
resurrect the mirror.

Each phase is independently useful, and phases 2 and 3 are small *because*
phase 1 settled the projection.

---

## Non-goals

- **Pulling players from HelloAsso** (or any registration platform). Related
  only in that it also touches personal data — decide separately, and keep
  registrants' email addresses out of `Tournament` entirely so that this
  document's "publish everything" stance stays safe.
- **Per-player private views** ("your next board is 14"). Everything here is
  the same page for everyone.
- **Result entry by players.** Not now, probably not ever: the referee is the
  authority on results, and the software already assumes referees are
  trusted (see the Limitations section of the README).

## Open decisions

None outstanding. The appendix's follow-ons (exact staleness detection
instead of the coarse rule, and a recompute-and-diff tool) are refinements to
decide against real use, not blockers.

---

## Appendix: freeze the round explanation at confirmation

**Shipped.** `TOURNAMENT_FORMAT_VERSION` went 9 → 10; the notes below are the
design as decided, with the two deviations recorded at the end.

Noticed while deciding what the public surface may call. `explain_round`
([`crates/core/src/tournament.rs`](../crates/core/src/tournament.rs)) rebuilds
the `PairingModel` from `self.rounds[..idx]` — the rounds *before* the one
being explained — and re-scores the boards that were chosen. Cost is not the
issue: there is no matching solve, just `edge_units` over the N/2 confirmed
boards.

Faithfulness is. `explain_pairing`'s contract is to score each pair "against
the exact model the round was paired from", and the inputs to that model are
mutable after the fact — a referee correcting round 2's result while round 4
is playing changes the scores round 3 was paired from; editing a rating, a
club, or a setting like club protection does the same. Each is a legitimate
action, and after any of them the explanation describes a model that never
paired anything, while still looking entirely plausible.

**Decided: store the explanation on the `Round` at confirmation**, when the
model really is the one that paired it.

This is not a cache of a derived value — it is a **record of a past event**,
which is exactly why it cannot be recomputed. `Round` already carries such
records: `sitouts` holds "what the round scored them, frozen when the round
was confirmed". So this does not breach the rule that keeps `standings` and
`can_undo` out of the persisted shape; those are functions of the present
state, and this is not.

Costs, all accepted: a `TOURNAMENT_FORMAT_VERSION` bump (5 → 6), with old
saves rejected loudly rather than migrated; and some save-file growth, which
is modest because `ledger()` keeps only the rules that actually fired
([`crates/core/src/pairing.rs`](../crates/core/src/pairing.rs)), so a cleanly
paired board stores an empty contribution list.

### The staleness watermark

Freezing makes the explanation permanently faithful to the pairing, but it
can still stop matching the tournament the reader is looking at: the stored
round-3 ledger may cite a score that a later correction to round 2 has since
changed. So a warning is still wanted — but it says "the data behind this has
changed since", not "this is wrong". It remains the most correct answer
available; it is the *present* that moved.

**One correction to the shape.** The affected rounds are the newest, not the
oldest, so the watermark points the other way: the explanation of round `r`
is built from `rounds[..r-1]`, so editing a result in round `k` can only
disturb rounds **after** `k`. The accurate explanations are therefore a
*prefix*, and round 1's is never disturbed by anything.

So: `explanations_faithful_through: u32` on the `Tournament` (0 = none),
warn on every round above it, and it only ever decreases:

| Action | Effect |
| --- | --- |
| Confirm round `n` | unchanged (the new round is faithful by construction; it does not raise the mark for rounds already below it) |
| Edit a result / sitout / adjustment in round `k` | `min(mark, k)` |
| Edit a player (rating, club, categories) or a pairing-relevant setting | `0` — these are global, not per round, so every round's model is affected |

It lives **in the `Tournament`**, not in server session state: unlike
`version`, it must survive save/load, travel with a mailed save file, and be
restored by undo and by a backup restore along with the state it describes.

Two follow-ons, neither needed at first:

- **The global-edit rule is coarse.** Changing one player's club drops the
  mark to 0 even when no board's ledger would move. The exact version is to
  recompute each round's explanation on such an edit and compare it to the
  stored one, marking only the rounds that genuinely differ — cheap (no
  solver) and free of false alarms, which is what keeps a warning from being
  ignored. Worth doing if the coarse rule proves noisy in practice.
- **Show the diff, not just the warning.** Once the explanation is stored,
  "recompute from current data and show what changed" becomes a natural
  referee tool for exactly the dispute this whole appendix is about — and it
  is the same shape as the existing counterfactual probe.

### What shipped, where it differs from the above

Two deviations, both narrowing the coarse rule rather than widening it:

- **Confirming round `n` does advance the mark** — to `n`, but only when the
  prefix below it is intact (`mark + 1 >= n`). Read literally, "unchanged" would
  leave the mark at 0 forever and warn on every round of an untouched
  tournament, which is the opposite of the intent: it must not *step over* a
  stale round, and that is what the guard says. Re-pairing (`force_pairing`)
  pops the round and re-confirms it, which is why the guard is `>=` and not `==`.
- **Categories and cup eligibility do not move the mark.** The table names them
  under "edit a player", but neither is read by `player_units` or
  `compute_scores` — no board's ledger can depend on them, so warning would be a
  pure false alarm. Rating, club and nationality do move it, and so do point
  adjustments (which carry no round of their own) and a pairing ELO. A player
  edit that changes none of those (a rename) leaves the mark alone, as does a
  settings PUT that changes nothing — the client sends whole objects for
  one-field edits, and a warning that fires on a no-op is one referees learn to
  ignore.

The watermark is also validated on load (`validate_loaded`): an explanation that
names a different round than the one carrying it, or a mark past the last round,
is rejected rather than shown against the wrong pairings.

### Consequence for publication

With the explanation stored, serving it publicly is free — the objection in
§3 was the per-request model rebuild plus this faithfulness problem, and both
are gone. Publishing it is a good fit for the "why these pairings?" feature's
intent (a player asking why they drew that opponent is the original
audience), and the ledger contains nothing beyond what the projection already
publishes. Slot it in after phase 1 rather than into it, gated by the same
publication flag and accompanied by the watermark warning.
