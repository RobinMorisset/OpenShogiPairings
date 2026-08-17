//! Public read-only access to the standings and pairings — see
//! `docs/archive/public-access.md` (phase 1).
//!
//! Players in the room, and anyone following from home, get the standings and
//! the pairings on their phones with no password and no ability to change
//! anything. Three decisions carry the whole feature:
//!
//! - **What** is public: [`PublicTournamentView`], a projection of the
//!   referee's own [`TournamentView`] — the same server-computed standings, so
//!   the public table can never disagree with the referee's.
//! - **When**: the draft is never published, and a result is published the
//!   instant it is recorded. Both fall out of the data model rather than a
//!   filter — a round only enters `Tournament::rounds` when it is confirmed, so
//!   dropping `Tournament::draft` *is* the entire timing policy.
//! - **Who**: a capability key in the URL, minted at publication and printed as
//!   a QR code on the wall. Not a boundary against a determined attacker (the
//!   link will be photographed and forwarded, which is fine — the content is
//!   meant to be seen) but against accidental discovery, which is the real risk.
//!
//! Enforcement is structural: [`scope`] is a router with no mutating handler in
//! it, mounted outside `require_tournament_auth`, so no bug in credential
//! handling can escalate a reader into a writer.

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use osp_core::{
    Cup, CupBracketView, CupPodium, Handicap, HandicapDisplay, HandicapPolicy, Player, Round,
    Standing, Team, TeamMatchView, TeamStanding, Tournament, TournamentSettings, Winner,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use uuid::Uuid;

use crate::error::ApiError;
use crate::scope::TournamentCtx;
use crate::state::{AppState, TournamentInstance};
use crate::tournament::{build_view, TournamentView};

/// How many public SSE streams one tournament will hold open at once.
///
/// Not a scaling limit — the cost of an idle stream is a tokio task, a
/// broadcast receiver and a socket, so a room of a hundred phones is a few
/// megabytes — but an abuse one. The referee stream is implicitly bounded by
/// knowing the password; this one is open to anyone holding a link that is
/// meant to circulate, which makes it a slow resource-exhaustion surface unless
/// something says no.
const MAX_PUBLIC_STREAMS: usize = 500;

/// The reader group: the projection, and its own push stream. No mutating
/// handler belongs here, ever — see the module docs.
pub(crate) fn scope() -> Router<AppState> {
    Router::new()
        .route("/public", get(get_public))
        .route("/public/events", get(public_events))
}

// --- The projection ---------------------------------------------------------

/// [`TournamentView`] minus everything a reader must not see.
///
/// Built by an explicit conversion that **destructures by value, naming every
/// field** (see [`From<TournamentView>`]). That is the whole reason this type
/// exists even though it drops so little: adding a field to `TournamentView` or
/// `Tournament` then fails to compile here until someone decides whether it is
/// public. A serde skip-list or a mutate-a-clone would fail *open* on schema
/// growth instead, publishing the new field by default.
#[derive(Serialize, ts_rs::TS)]
#[ts(
    export,
    rename = "PublicTournamentResponse",
    export_to = "../../../frontend/src/lib/generated/"
)]
pub(crate) struct PublicTournamentView {
    tournament: PublicTournament,
    /// The same monotonic change version the referee sees. A reader never sends
    /// it back (there is nothing to conflict with); it drives the `ETag` and
    /// lets the client ignore an out-of-order update.
    version: u32,
    standings: Vec<Standing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    team_standings: Option<Vec<TeamStanding>>,
    /// The rounds' team matches, exactly as the referee sees them: the reader's
    /// round page groups its boards by match and shows each match's score, and
    /// it must be the same grouping — it is derived from the published rosters
    /// and the published boards either way.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    team_matches: Vec<Vec<TeamMatchView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cup_podium: Option<CupPodium>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cup_bracket: Option<CupBracketView>,
    /// Published only when the referee chose to *show* a suggested handicap
    /// (`HandicapDisplay::Suggested`); empty otherwise. The suggestion is
    /// primarily for the two players, who need to know what to set up on the
    /// board — but a referee who chose the plain picker has deliberately turned
    /// it off, and the public page must not contradict the room.
    suggested_handicaps: Vec<Vec<Option<Handicap>>>,
    effective_winners: Vec<Vec<Option<Winner>>>,
}

/// [`Tournament`] minus the draft — the timing rule, structurally (see the
/// module docs). Every other field is published as-is, including each player's
/// name, club, rating and point adjustments: those are tournament facts a
/// printed wall sheet would carry anyway, and an adjustment moves points and
/// therefore pairings, so hiding it would make the public standings
/// unexplainable.
#[derive(Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub(crate) struct PublicTournament {
    format_version: u32,
    id: Uuid,
    name: String,
    settings: TournamentSettings,
    players: Vec<Player>,
    registration_finalized: bool,
    /// Each round carries its frozen pairing `explanation` and whether that
    /// explanation still matches the present, so both ride along with the
    /// projection — nothing in the ledger goes beyond what is published here
    /// anyway (it names the same players and the same rules), and a reader shown
    /// an explanation must be shown whether the data behind it has moved since.
    /// No public UI reads either yet; showing them is phase 4 in
    /// `docs/archive/public-access.md`.
    rounds: Vec<Round>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cup: Option<Cup>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    teams: Vec<Team>,
}

impl From<TournamentView> for PublicTournamentView {
    fn from(view: TournamentView) -> Self {
        // Exhaustive destructuring, deliberately: no `..`, so a new field is a
        // compile error here rather than a silent publication.
        let TournamentView {
            tournament,
            // Referee session state — whether *their* undo button is live, and
            // why their next-round / export buttons are not. A reader has
            // neither button, and the reasons name rounds and rules that are
            // the referee's business.
            can_undo: _,
            next_round_blocked: _,
            grid_export_blocked: _,
            // The referee's problem to see and act on, not the room's: a reader
            // can do nothing about the server's disk, and the results on screen
            // are correct either way.
            persisted: _,
            version,
            standings,
            team_standings,
            team_matches,
            cup_podium,
            cup_bracket,
            // Derived from the draft, which is not published.
            draft_cup_players: _,
            draft_long_players: _,
            suggested_handicaps,
            effective_winners,
        } = view;
        let Tournament {
            format_version,
            id,
            name,
            settings,
            players,
            registration_finalized,
            // The round being hand-tuned. Players must not watch the referee
            // work, and a pairing that gets discarded must never have been
            // visible.
            draft: _,
            rounds,
            cup,
            teams,
        } = tournament;
        let suggested_handicaps = match settings.handicap_policy {
            HandicapPolicy::Enabled {
                display: HandicapDisplay::Suggested,
                ..
            } => suggested_handicaps,
            _ => Vec::new(),
        };
        Self {
            tournament: PublicTournament {
                format_version,
                id,
                name,
                settings,
                players,
                registration_finalized,
                rounds,
                cup,
                teams,
            },
            version,
            standings,
            team_standings,
            team_matches,
            cup_podium,
            cup_bracket,
            suggested_handicaps,
            effective_winners,
        }
    }
}

// --- Serving it -------------------------------------------------------------

/// The capability key, as the query string carries it: `?k=<192 random bits>`.
///
/// Optional so that a *missing* key is rejected by [`authorize`] like a wrong
/// one — a 404 — rather than by the extractor with a 400, which would tell an
/// unauthenticated caller that this tournament id is real.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityKey {
    #[serde(default)]
    k: Option<String>,
}

/// Reject a reader who has no valid key for this tournament.
///
/// Always a 404, never a 401: an unpublished tournament, a rotated key and a
/// tournament that does not exist must be indistinguishable, or the endpoint
/// becomes an oracle for which ids are real. There is nothing to log in *to*
/// here, so a 401 would also send the frontend to a password prompt no reader
/// can satisfy.
fn authorize(instance: &TournamentInstance, key: &CapabilityKey) -> Result<(), ApiError> {
    let presented = key.k.as_deref().unwrap_or_default();
    if instance.public_key_matches(presented) {
        Ok(())
    } else {
        Err(ApiError::NotFound("no public page for this link".into()))
    }
}

/// The serialized public projection and the version it describes, reusing the
/// cached copy when the tournament has not changed since.
///
/// One mutation costs one serialization, not one per reader: the payload is
/// byte-identical for everyone and changes only when the version does. The
/// store read lock is held across the whole function so the version can't move
/// under us and cache a payload against the wrong number.
fn payload(instance: &TournamentInstance) -> Result<(u32, Bytes), ApiError> {
    let store = instance.read();
    let version = store.version();
    if let Some(cached) = instance.cached_public_payload(version) {
        return Ok((version, cached));
    }
    let public = PublicTournamentView::from(build_view(&store)?);
    let bytes =
        Bytes::from(serde_json::to_vec(&public).map_err(|e| {
            ApiError::BadRequest(format!("could not serialize the tournament: {e}"))
        })?);
    instance.cache_public_payload(version, bytes.clone());
    Ok((version, bytes))
}

/// The `ETag` for `version` of a tournament, on this server run.
///
/// The boot id is not decoration: `version` restarts at `0` on every boot, so
/// without it a reader holding `"5"` from before a restart would be answered
/// `304 Not Modified` for a completely different state — and the page would
/// simply stop updating, with no error anywhere.
fn etag(state: &AppState, version: u32) -> String {
    format!("\"{}-{version}\"", state.boot_id)
}

/// `GET /api/tournaments/{id}/public?k=…` — the projection, with an `ETag`.
///
/// `If-None-Match` is what makes the reconnect and cold-load path cheap: a herd
/// of phones coming out of pockets at the end of a round mostly gets a 304.
async fn get_public(
    State(state): State<AppState>,
    TournamentCtx(instance): TournamentCtx,
    Query(key): Query<CapabilityKey>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    authorize(&instance, &key)?;
    let (version, body) = payload(&instance)?;
    let etag = etag(&state, version);
    let known = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|candidate| candidate.trim() == etag));
    if known {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
    }
    Ok((
        [
            (header::ETAG, etag),
            (header::CONTENT_TYPE, "application/json".to_string()),
        ],
        body,
    )
        .into_response())
}

/// `GET /api/tournaments/{id}/public-snapshot` — the same projection, for the
/// **referee**, so the desktop app can write it out as plain web pages
/// (`docs/archive/public-access.md` phase 2).
///
/// It lives in the protected group rather than behind the capability key, for
/// two reasons. The laptop deployment is the whole point of the static export —
/// its embedded server listens on `127.0.0.1:<random>`, so the reader endpoint
/// is unreachable there and publishing would mint a key pointing at nothing.
/// And the referee is already authenticated, so there is nothing for a key to
/// add. Saving the file is itself the act of publishing, for this transport.
///
/// It is deliberately *this* projection and not the referee's own view: the
/// exhaustive destructuring in [`From<TournamentView>`] is what keeps a new
/// field out of a public file until someone decides it belongs there, and a
/// client-side re-implementation of the filter would fail open instead.
///
/// The `route_layer`-installed version guard is a no-op for a `GET`, so there
/// is no conflict header to send.
pub(crate) async fn get_public_snapshot(
    TournamentCtx(instance): TournamentCtx,
) -> Result<Response, ApiError> {
    let (_, body) = payload(&instance)?;
    Ok((
        [(header::CONTENT_TYPE, "application/json".to_string())],
        body,
    )
        .into_response())
}

/// `GET /api/tournaments/{id}/public/events?k=…` — the same projection, pushed
/// on every change.
///
/// The referee stream carries only the version, and every client then issues a
/// full `GET`: one mutation costs a hundred serializations and a hundred
/// transfers. Readers get the payload *in* the event instead, so the refetch
/// disappears — one serialization and zero extra round trips per change. That
/// makes SSE strictly cheaper than polling here rather than more expensive.
async fn public_events(
    TournamentCtx(instance): TournamentCtx,
    Query(key): Query<CapabilityKey>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    authorize(&instance, &key)?;
    let Some(slot) = instance.open_public_stream(MAX_PUBLIC_STREAMS) else {
        return Err(ApiError::Overloaded(format!(
            "this tournament already has {MAX_PUBLIC_STREAMS} public readers connected"
        )));
    };
    let receiver = instance.read().subscribe();
    // Send the current state straight away rather than making a fresh reader
    // wait for the next change — which, between rounds, can be minutes.
    let initial = tokio_stream::once(Ok(state_event(&instance)));
    // Authorization is re-checked on **every** event, not just at connect.
    //
    // A stream outlives the request that opened it, so checking the key once
    // would make rotating it revoke nothing for the people already watching —
    // they would keep receiving results down a connection opened with a key
    // that no longer exists. Revocation has to reach the open connections, or
    // "New link" is a lie told to the referee who clicked it.
    let presented = key.k.unwrap_or_default();
    let mut revoked = false;
    let updates = BroadcastStream::new(receiver).map_while(move |message| {
        // The slot lives as long as the stream does, and releases on disconnect.
        let _slot = &slot;
        // Whether this is a change or a lagged-past-the-buffer notice makes no
        // difference: the payload is the whole state, so catching up and
        // keeping up are the same operation.
        let _ = message;
        if revoked {
            return None; // the goodbye below has already gone out
        }
        if !instance.public_key_matches(&presented) {
            // Say so rather than just dropping the connection: a silent close
            // is indistinguishable from a wifi blip, and the client would
            // reconnect against it for as long as the tab stayed open.
            revoked = true;
            return Some(Ok(Event::default().event("revoked").data("")));
        }
        Some(Ok(state_event(&instance)))
    });
    Ok(Sse::new(initial.chain(updates)).keep_alive(KeepAlive::default()))
}

/// One SSE event carrying the whole public projection — or, if the tournament
/// has been deleted under the reader, a `gone` event saying so. Going quiet
/// instead would leave the page showing a tournament that no longer exists.
///
/// Only [`ApiError::NoTournament`] means "deleted". Anything else out of
/// [`payload`] is a serialization bug on our side, and answering *that* with
/// `gone` would tell a room full of phones the tournament was deleted while the
/// referee's own screen looks fine — with nothing logged anywhere. So it is
/// logged and sent as its own `failure` event, which the reader shows as an error
/// and retries from, rather than as a tombstone it can never come back from.
/// (`failure`, not `error`: an SSE event named `error` is dispatched on the
/// browser's `EventSource` object itself, where it reads as a dropped
/// connection.)
fn state_event(instance: &TournamentInstance) -> Event {
    match payload(instance) {
        Ok((_, body)) => Event::default()
            .event("state")
            .data(String::from_utf8(body.to_vec()).expect("serde_json emits UTF-8")),
        Err(ApiError::NoTournament) => Event::default().event("gone").data(""),
        Err(e) => {
            tracing::error!("public stream: could not build the projection: {e:?}");
            Event::default()
                .event("failure")
                .data("the server could not build this tournament's public view")
        }
    }
}

// --- Opting in --------------------------------------------------------------

/// `GET`/`PUT /api/tournaments/{id}/publication` — the referee's view of it.
///
/// The key is handed only to an authenticated referee: it is what the QR code
/// on the wall encodes, and handing it out is the act of publishing.
#[derive(Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub(crate) struct PublicationState {
    published: bool,
    /// The capability key, when published. `None` otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

impl PublicationState {
    fn from_key(key: Option<String>) -> Self {
        Self {
            published: key.is_some(),
            key,
        }
    }
}

/// Whether this tournament is published, and under which key.
pub(crate) async fn get_publication(
    TournamentCtx(instance): TournamentCtx,
) -> Json<PublicationState> {
    Json(PublicationState::from_key(instance.public_key()))
}

/// Body of `PUT /publication`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetPublicationRequest {
    /// `true` publishes and mints a **fresh** key — so this is also how a key
    /// is rotated, revoking every link already handed out. `false` unpublishes.
    published: bool,
}

/// Publish, rotate the key, or unpublish.
///
/// Deliberately not a tournament mutation: it changes no content, bumps no
/// version and is not undoable — it is access-control state, stored next to the
/// password hash rather than in the save file.
pub(crate) async fn set_publication(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    Json(req): Json<SetPublicationRequest>,
) -> Result<Json<PublicationState>, ApiError> {
    state
        .registry
        .set_publication(id, req.published)
        .map(|key| Json(PublicationState::from_key(key)))
        .map_err(|e| ApiError::NotFound(e.to_string()))
}
