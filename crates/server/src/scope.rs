//! Resolving the tournament a request is scoped to.
//!
//! Every route under `/api/tournaments/{id}/...` needs to look up the
//! [`TournamentInstance`] for that `id` before doing anything else — auth
//! middleware, the version-check middleware, and the handlers themselves all
//! do this same lookup independently (cheap: an `Arc` clone under a brief
//! registry read lock).

use axum::extract::{FromRef, FromRequestParts, Path};
use axum::http::request::Parts;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::{AppState, TournamentInstance};

/// Just the `{id}` segment, extracted by name — not a bare `Path<Uuid>`
/// (positional), which requires the *whole* matched route to have exactly one
/// captured param. Nested routes typically have more (e.g. `{id}/players/
/// {player_id}`), so this needs to pick `id` out by name and ignore the rest.
#[derive(Deserialize)]
struct TournamentId {
    id: Uuid,
}

/// Extracts the tournament addressed by the `{id}` path segment, 404-ing if it
/// doesn't exist. Usable as a handler argument or as a leading argument to an
/// `axum::middleware::from_fn` middleware.
pub(crate) struct TournamentCtx(pub Arc<TournamentInstance>);

impl<S> FromRequestParts<S> for TournamentCtx
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(TournamentId { id }) = Path::<TournamentId>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::NotFound("no tournament id in the request path".into()))?;
        let app_state = AppState::from_ref(state);
        app_state
            .registry
            .get(id)
            .map(TournamentCtx)
            .ok_or_else(|| ApiError::NotFound(format!("no tournament {id}")))
    }
}
