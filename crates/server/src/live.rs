//! Live sync: a change-notification stream (SSE) plus an optimistic-concurrency
//! guard. Both are per-tournament now (see `docs/multi-tournament.md`).
//!
//! Several referees share one tournament, so once they edit concurrently two
//! problems appear (see `docs/multi-referee-internet.md` §3):
//!
//! - they must see each other's changes without manually reloading, and
//! - an edit made against a stale view must not silently clobber a newer one.
//!
//! The first is solved by [`events`]: a Server-Sent Events stream that emits the
//! tournament `version` whenever it changes, so clients refetch on a newer one.
//! The second by [`check_version`]: middleware that rejects a mutating request
//! whose `X-Tournament-Version` no longer matches the server (409 Conflict), so
//! the client refetches instead of overwriting.

use std::convert::Infallible;

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::Method;
use axum::middleware::Next;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::error::ApiError;
use crate::scope::TournamentCtx;

/// Header a client sets to the version its edit is based on. Lower-cased because
/// that is how `HeaderMap` stores and matches names.
const VERSION_HEADER: &str = "x-tournament-version";

/// Parse the [`VERSION_HEADER`] out of request parts.
///
/// `Ok(None)` when the header is absent (the client opts out of the check);
/// `Err` when it is present but not a `u32` — a malformed value must be a hard
/// error, not silently treated like "no header" (which would drop conflict
/// detection for that request).
fn parse_version_header(parts: &axum::http::HeaderMap) -> Result<Option<u32>, ApiError> {
    match parts.get(VERSION_HEADER) {
        None => Ok(None),
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .map(Some)
            .ok_or_else(|| ApiError::BadRequest(format!("invalid {VERSION_HEADER} header"))),
    }
}

/// Extractor yielding the version a mutating request declares it is based on
/// (see [`VERSION_HEADER`]), so a handler can re-check it *inside* the write lock
/// for an atomic conflict check (`store.mutate(expected, …)`). A malformed value
/// is rejected 400 — the same way [`check_version`] treats it — so the extractor
/// itself stays lenient about the absent case only.
pub struct ExpectedVersion(pub Option<u32>);

impl<S> FromRequestParts<S> for ExpectedVersion
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parse_version_header(&parts.headers).map(ExpectedVersion)
    }
}

/// Reject a mutating request whose declared base version is stale, for the
/// tournament addressed by the path.
///
/// Only acts on `POST`/`PUT`/`DELETE` requests that opt in by sending
/// [`VERSION_HEADER`]; everything else (reads, and clients that don't
/// participate) passes straight through. This is an early pre-screen that
/// rejects an already-stale edit before running the handler at all; because it
/// reads the version and releases the lock before the handler takes the write
/// lock, the *authoritative* check for the fine-grained mutations is repeated
/// atomically inside `store.mutate(expected, …)` (see
/// [`crate::state::TournamentStore::mutate`]), which closes the sub-millisecond
/// race two edits from the same base version would otherwise slip through.
pub async fn check_version(
    TournamentCtx(instance): TournamentCtx,
    req: Request,
    next: Next,
) -> Response {
    let mutating = matches!(*req.method(), Method::POST | Method::PUT | Method::DELETE);
    if mutating {
        let expected = match parse_version_header(req.headers()) {
            Ok(expected) => expected,
            Err(e) => return e.into_response(),
        };
        if let Some(expected) = expected {
            let current = instance.read().version();
            if current != expected {
                return ApiError::VersionConflict.into_response();
            }
        }
    }
    next.run(req).await
}

/// `GET /api/tournaments/{id}/events` — an SSE stream emitting one
/// tournament's `version` on every change.
///
/// Public (like `/api/health`): it carries only an opaque counter, never
/// tournament data, and a browser's `EventSource` can't send the auth header
/// anyway. Clients refetch the (gated) tournament when the version rises past
/// what they hold, and ignore the echo of their own edits.
pub async fn events(
    TournamentCtx(instance): TournamentCtx,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = instance.read().subscribe();
    let stream = BroadcastStream::new(receiver).map(|message| {
        let data = match message {
            Ok(version) => version.to_string(),
            // Lagged past the buffer: tell the client to resync unconditionally.
            Err(_) => "reload".to_string(),
        };
        Ok(Event::default().event("changed").data(data))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
