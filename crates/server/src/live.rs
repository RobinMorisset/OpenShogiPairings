//! Live sync: a change-notification stream (SSE) plus an optimistic-concurrency
//! guard.
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

use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::error::ApiError;
use crate::state::AppState;

/// Header a client sets to the version its edit is based on. Lower-cased because
/// that is how `HeaderMap` stores and matches names.
const VERSION_HEADER: &str = "x-tournament-version";

/// Reject a mutating request whose declared base version is stale.
///
/// Only acts on `POST`/`PUT`/`DELETE` requests that opt in by sending
/// [`VERSION_HEADER`]; everything else (reads, and clients that don't
/// participate) passes straight through. This is a best-effort backstop — the
/// check reads the version just before the handler takes the write lock, so a
/// sub-millisecond race could slip a conflict past — but for human referees,
/// backed by live updates, it turns real lost updates into visible 409s.
pub async fn check_version(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let mutating = matches!(*req.method(), Method::POST | Method::PUT | Method::DELETE);
    if mutating {
        if let Some(expected) = req
            .headers()
            .get(VERSION_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u32>().ok())
        {
            let current = state.store.read().expect("store lock poisoned").version();
            if current != expected {
                return ApiError::VersionConflict.into_response();
            }
        }
    }
    next.run(req).await
}

/// `GET /api/tournament/events` — an SSE stream emitting the tournament
/// `version` on every change.
///
/// Public (like `/api/health`): it carries only an opaque counter, never
/// tournament data, and a browser's `EventSource` can't send the auth header
/// anyway. Clients refetch the (gated) tournament when the version rises past
/// what they hold, and ignore the echo of their own edits.
pub async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.store.read().expect("store lock poisoned").subscribe();
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
