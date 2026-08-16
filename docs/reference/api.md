# API

Every route `osp-server` exposes. For what sits behind them, see
[`architecture.md`](architecture.md).

Registry-level routes (not scoped to any one tournament):

| Method & path | Purpose |
|---------------|---------|
| `GET /api/health` | Liveness check. |
| `GET /api/tournaments` | List tournaments for the picker: `{ "tournaments": [{ "id", "name", "has_password", "problem"? }], "restricted" }`. `problem` (`{ code, values, message }`, localized client-side like any error) is present on a tournament whose save this build cannot read — listed anyway, so it can be seen and deleted rather than silently missing; never sent to a caller who only gets the published ones. Public, since the picker has to render before anyone has logged in anywhere — but on a server that has `OSP_ADMIN_PASSWORD` set, a caller without a valid admin token gets only the *published* tournaments and `restricted: true` (which the picker turns into an admin-password prompt rather than a silently short list). A server deliberately run open lists everything, as before. |
| `POST /api/tournaments` | Create a new tournament: `{ "name": "...", "password"? }`. Admin-gated if `OSP_ADMIN_PASSWORD` is set. Returns `{ "id", "token"? }` (a token if the new tournament has a password, so the creator needn't immediately log in to it). |
| `POST /api/tournaments/import` | Create a tournament from a save file (what the picker's "Load from file…" does): `{ "tournament": {…the file verbatim…}, "password"? }`. Same admin gate and same `{ "id", "token"? }` response as creating one. Deliberately a *single* request: the format version and the tournament's own invariants are checked before anything is registered, so a file this build can't read leaves nothing behind. The file's own `id` is ignored — the registry mints a fresh one, so importing the same file twice can't collide. |
| `GET /api/tournaments/deleted` | The bin: `[{ "id", "name", "deleted_at", "has_password", "backups", "problem"? }]`, most recently deleted first. Admin-gated — it names tournaments somebody deliberately deleted. `problem` is present only when the entry *cannot* be restored (its save is a format this build no longer reads — what deleting an already-unopenable tournament leaves behind); the picker greys those out, and omitting it is the normal case. |
| `POST /api/tournaments/deleted/{id}/restore` | Bring a deleted tournament back under **its own id**: `{ "backup_id"?, "password"? }`, defaulting to the newest backup — the one taken as it was deleted. `password` is the password it had, required if it had one (403 otherwise) and given back to it. Same admin gate and `{ "id", "token"? }` response as creating one. Its backups directory becomes an ordinary live one again (marker dropped, history kept), so it leaves the bin; restoring one that is already back is a 409. |
| `POST /api/admin/login` | Exchange the admin password for a bearer token. |
| `GET /api/ratings` | FESA rating list (server-cached) for registration autocomplete. Admin-gated. |
| `POST /api/ratings/refresh` | Re-download the FESA list now (manual refresh). Admin-gated. |

Per-tournament routes, all nested under `/api/tournaments/{id}` and requiring
that tournament's bearer token if it has a password (except `/login`,
`/events`, and the two `/public` reader routes, which take a capability key
instead — see [public read-only access](architecture.md#public-read-only-access)):

| Method & path | Purpose |
|---------------|---------|
| `POST /login` | Exchange this tournament's password for a bearer token. |
| `GET /events` | SSE stream of this tournament's change `version`, for live sync. |
| `GET /public?k=…` | The public projection (`PublicTournamentView`), for a reader holding the capability key. `ETag` = the tournament version paired with the server's boot id; honours `If-None-Match`. A wrong key, a rotated key and an unpublished tournament are all `404`, deliberately — the endpoint must not tell a stranger which ids are real. |
| `GET /public/events?k=…` | SSE stream carrying the **whole** public projection on connect and on every change, so a reader never refetches. `503` once the tournament is at its cap of concurrent public streams. |
| `GET /publication` | Whether this tournament is published, and under which key: `{ "published", "key"? }`. Referee-only. |
| `PUT /publication` | `{ "published": true｜false }`. `true` publishes and always mints a **fresh** key, so it is also how a key is rotated (revoking every link already handed out); `false` unpublishes. Not a tournament mutation: it bumps no version and is not undoable — it is access-control state, stored in the `{id}.auth.json` sidecar next to the password hash. |
| `GET /public-snapshot` | The same `PublicTournamentView`, for the referee's *Export web pages…* — so the client renders the static pages from the one fail-closed projection rather than re-deriving it. Referee-only, and deliberately **not** gated on `/publication`: the export exists for the desktop app, where the reader endpoint is unreachable and a key would point at nothing. |
| `GET /` | Fetch the tournament (`TournamentView`; 404 if unknown). |
| `DELETE /` | Delete the tournament: its registry entry and its persisted file. Its backups are kept (see `GET /api/tournaments/deleted`), so this is recoverable. Gated by *this tournament's* token, like the rest of this table — deleting one is not an admin capability, unlike creating one. |
| `POST /undo` | Revert the last change (server-side undo history). `409` when there is nothing left to undo — a caller whose view is current cannot reach that (everything which empties the history bumps the version), so it means the request was built from a stale one. |
| `GET /american-grid` | Export the cross-table (American Grid) as `text/plain` for an ELO update: a header carrying the tournament's name, place, dates and time control, then one row per player in final-rank order, opponents referenced by final rank, drawn games as `=`. |
| `PUT /settings` | Replace the whole `TournamentSettings`. Its shape is nested: `pairing` is a tagged union — either `{ "kind": "swiss", "floater_style": "classic"｜"median", "macmahon": { "thresholds": [ { "criterion": { "kind": "elo", "value": 1200 }, "drops_after_round"?: 3 }, { "criterion": { "kind": "grade", "grade": { "kind": "dan"｜"kyu", "level": 1 } } } ], "source": { "kind": "static" } }, "airtight_groups"?: 2, "club_protection": { "kind": "on", "rounds"?: 3, "exempt_clubs"?: ["Paris"] }, "nationality_protection": { "kind": "on", "rounds"?: 3, "exempt_nationalities"?: ["JP"] } }` or `{ "kind": "elo", "estimator": { … } }` for the experimental pure-ELO pairing mode. Alongside `pairing`, the top level carries `cup_enabled`, `cup_format` (`"direct"`｜`"qualifier"` — see the hybrid-cup section above; only consulted when `cup_enabled`), `long_boards_enabled`, `handicap_policy` (`{ "kind": "none" }｜{ "kind": "enabled", "display": …, "wiel_rule"?: false }`), `half_point_absences`, `tiebreaks` (an ordered array, e.g. `["points","sos_m",…]`), and `categories` (referee-defined player categories, an array of `{ "id", "name" }`; blank-named entries are dropped on normalization). Notes: a threshold's `criterion` mixes ELO and grade freely (each counted independently) and `drops_after_round` makes it a degressive threshold; `airtight_groups`, if set, forbids pairing across MacMahon groups during rounds `1..=n`; `club_protection` is `{ "kind": "off" }` or `{ "kind": "on", … }`, and `nationality_protection` is the same shape one rule tier weaker (its exempt list is `exempt_nationalities`); both default to off and are omitted from the response when off; `macmahon.source` is `{ "kind": "static" }` or `{ "kind": "from_estimate", "estimator": { … } }` (estimate-based MacMahon). The `estimator` knobs are described in [`docs/archive/elo-pairing-mode.md`](../archive/elo-pairing-mode.md). |
| `POST /cancel-round` | Cancel the last round — discards the open draft if one is being prepared, otherwise removes the most recent round (undoable). |
| `POST /rounds/prepare` | Begin drafting the next round. For round 1, finalizes registration first, in the same undo step — that is the only way to finalize. Body optional: `{ "cup_size": 8｜16｜32｜64 }` when the hybrid cup is enabled — `cup_size` is the *bracket* size; it seeds the top eligible players into it, taking `cup_size` of them under `cup_format: "direct"` and `1.5 × cup_size` under `"qualifier"` (400 if fewer are marked eligible). Ignored from round 2 on (already finalized). A round completes automatically once every board has a result, so there is no separate "complete round" call. |
| `PUT /draft` | Edit the draft: `{ "absent", "forced_boards", "forced_byes" }` — or, in a team tournament, `{ "absent", "forced_matches": [{ "team1", "team2" }], "forced_team_byes" }`, since teams are what get paired there. The absent set is per player in either mode: a team member can be absent without their team being, and their board is then forfeited for them. Sending the other mode's forced lists is an error, not a silently ignored field. |
| `POST /rounds` | Confirm the draft: pair remaining players and start the round. |
| `POST /rounds/{n}/counterfactual` | Explain forcing (`"force"`) or forbidding (`"forbid"`) a pairing `{a, b}` in this round. `a`/`b` are player numbers, or **team** numbers in a team tournament — whichever the engine paired; `0` means the bye. Read-only. |
| `POST /rounds/force-pairing` | Re-pair the current round with `{a, b}` fixed — a board, or a **team match** in a team tournament (same numbering as the counterfactual; `0` forces the other side onto the bye). |
| `POST /rounds/{n}/boards/{i}/result` | Toggle a board's winner: `{ "clicked": "player1"｜"player2" }`. |
| `POST /rounds/{n}/boards/{i}/drawn` | Set the "a draw occurred" flag: `{ "drawn": true｜false }`. |
| `POST /rounds/{n}/boards/{i}/no-show` | Mark a board a no-show (or clear it): `{ "absent": "player1"｜"player2"｜"both"｜null }`. A no-show settles the board (counting toward auto-completing the round) and carries the absence into the next round's draft. |
| `PUT /rounds/{n}/boards/{i}/handicap` | Set/clear the handicap: `{ "handicap": "4p"｜null }` (giver frozen from ratings; 400 if ratings equal). |
| `POST /rounds/{n}/boards/{i}/long` | Flag/unflag a board as a two-round "long game": `{ "long": true｜false }` (only when `long_boards_enabled`). Flagging the last-undecided board can complete the round, like recording a result. |
| `PUT /rounds/{n}/sitouts/{player}` | Re-score what a round was worth to a player who sat it out: `{ "value": "zero"｜"half"｜"full" }`. Allowed on completed rounds. |
| `PUT /rounds/{n}/team-sitouts/{team_id}` | The same for a whole team, writing the value to every member's entry at once — a team sits out together and its score for the round is read from entries that must agree, so this is how a team's bye or absence is re-scored. Team mode only; rejects a team that played that round. |
| `POST /players` | Register a player: `{ "last_name", "first_name?", "rating?", "grade?", "nationality?", "club?" }` (`grade` is `{ "kind": "dan"｜"kyu", "level": … }`). |
| `POST /players/import-csv` | Register a roster from a raw CSV text body (parsed server-side, ratings matched against the FESA cache), as a single undo step. Entries naming an already-registered player are skipped and returned in `skipped_duplicates`. |
| `POST /players/licence-check` | Which registered players of one nationality are **not** on a federation's licence list: `{ "nationality", "csv" }` → `{ "listed", "checked", "missing": [{ "id", "near_misses": [list spelling, …] }, …] }`, each `near_misses` entry being a listed name one edit from that player's. Read-only, and 400 on a list that doesn't parse — never an empty `missing` standing in for one. |
| `PUT /players/{id}` | Edit a player's fields in place. |
| `DELETE /players/{id}` | Remove a player (400 if they are seeded in the cup bracket). |
| `POST /players/eligible` | Set cup eligibility for a list of players at once: `{ "player_ids": [id, …], "eligible": true｜false }`. All-or-nothing — the whole list is applied inside one mutation, so an id that is no longer registered leaves *every* player as they were rather than the roster half-changed, and the lot costs one version bump and one undo step. What the Players tab's "by nationality" control uses; it sends ids rather than a nationality because which players to include is the client's rule (those with no nationality are a group too), not something worth restating server-side. |
| `POST /players/{id}/eligible` | Set cup eligibility: `{ "eligible": true｜false }`. |
| `POST /players/{id}/category` | Add/remove membership in a referee-defined category: `{ "category_id", "member": true｜false }` (404 if the category doesn't exist). |
| `POST /players/{id}/adjustments` | Apply a manual point bonus/penalty: `{ "delta", "reason" }` (reason mandatory). |
| `DELETE /players/{id}/adjustments/{adjustment_id}` | Remove a point adjustment. |
| `PUT /players/{id}/pairing-rating` | Set (or clear, with `null`) a player's pairing ELO: `{ "pairing_rating": 1400｜null }`. Team mode with MacMahon starting points only — it feeds the team average and nothing user-facing, and is never exported. |
| `POST /teams` | Create a team: `{ "name" }` (non-empty, unique ignoring case). Team mode only, and only while registration is open — as for every team route below. |
| `PUT /teams/{team_id}` | Rename a team: `{ "name" }`. |
| `DELETE /teams/{team_id}` | Delete a team; its members go back to the unassigned pool (they stay registered). |
| `POST /teams/{team_id}/members` | Add a registered player to a team, at the board their pairing rating calls for: `{ "player_id" }`. Inserted in front of the first weaker member (unrated last), leaving the rest of the order untouched. A player belongs to exactly one team, and a team stops at its configured size. |
| `DELETE /teams/{team_id}/members/{player_id}` | Take a player out of a team. |
| `PUT /teams/{team_id}/board-order` | Set the board order (index 0 = board 1): `{ "order": [player_id, …] }`, which must be a permutation of the team's current members. |
| `POST /teams/{team_id}/sort-by-rating` | Reset the board order to descending pairing rating (unrated last). |
| `POST /teams/{team_id}/adjustments` | Apply a manual point bonus/penalty to a team: `{ "delta", "reason" }` (reason mandatory). Unlike the roster routes this stays available after finalization — adjustments are team-level in team mode, and the per-player ones are refused there. |
| `DELETE /teams/{team_id}/adjustments/{adjustment_id}` | Remove a team adjustment. |
| `GET /backups` | The automatic backups (taken at every round-lifecycle transition): `{ "directory", "backups": [...] }`, newest first. `directory` is the absolute path they are written to — shown in the Backups button's tooltip and the panel, so the files can be found outside the app — or `null` when no backups directory could be resolved and nothing is being backed up. |
| `POST /backups/{backup_id}/restore` | Restore a backup as the current tournament; resets undo history. |

The FESA rating list is fixed-width, Latin-1 text (parsed in
[`crates/core`](../../crates/core/src/fesa.rs)). It's shared reference data, so the
**server** owns the cache: it downloads from FESA **only once**, the first time a
list is needed with nothing cached, then keeps it in memory and persists it to a
per-user cache file. It never re-downloads on its own — updating the list is a
manual action (the "Refresh FESA list" button → `POST /api/ratings/refresh`).
Clients pull the list once and filter locally.

Known limitations and future work are tracked in [TODO.md](../../TODO.md).

Every mutating endpoint returns the full updated `TournamentView`. Its
`undo_label` — `{ code, values }`, absent when there is nothing to undo — says
what `POST /undo` would revert, so the button can name somebody else's change
rather than only offering to take it back; the codes are the `UndoCode` enum in
`crates/server/src/undo.rs`, and the client localizes them under `undo.*`.
Save/load is
platform-aware: in the **Tauri** desktop app it uses native OS file dialogs (the
`dialog` plugin plus small `read_text_file`/`write_text_file` commands), and in
the browser it falls back to a JSON download / file-picker upload. Either way a
loaded tournament is `PUT` back to the server, which stays authoritative.
