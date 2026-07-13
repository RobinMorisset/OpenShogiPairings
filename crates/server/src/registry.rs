//! The tournament registry's own top-level routes: list and create.
//!
//! Unlike everything under `/api/tournaments/{id}/...` (see [`crate::tournament`]),
//! these aren't scoped to one tournament — `GET` has to work before you've
//! picked one, and `POST` is what mints a new one.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::{AppState, TournamentSummary};

/// `GET /api/tournaments`: list every known tournament. Public — needed to
/// render the picker before anyone has logged in anywhere.
pub fn public_routes() -> Router<AppState> {
    Router::new().route("/api/tournaments", get(list_tournaments))
}

/// `POST /api/tournaments`: create a new, empty tournament. Callers are
/// expected to wrap this with [`crate::auth::require_admin_auth`] (see
/// `lib.rs`) — creation is gated by the admin password, if one is configured.
pub fn admin_routes() -> Router<AppState> {
    Router::new().route("/api/tournaments", post(create_tournament))
}

async fn list_tournaments(State(state): State<AppState>) -> Json<Vec<TournamentSummary>> {
    Json(state.registry.list())
}

/// Body of `POST /api/tournaments`.
#[derive(Debug, Deserialize)]
struct CreateTournamentRequest {
    name: String,
    /// This tournament's own password; `None`/absent leaves it open.
    #[serde(default)]
    password: Option<String>,
}

/// Response of `POST /api/tournaments`.
#[derive(Serialize)]
struct CreateTournamentResponse {
    id: Uuid,
    /// The session token for the tournament just created, if it has a
    /// password — so the creator doesn't have to immediately log in to their
    /// own tournament.
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}

async fn create_tournament(
    State(state): State<AppState>,
    Json(req): Json<CreateTournamentRequest>,
) -> Result<(StatusCode, Json<CreateTournamentResponse>), ApiError> {
    let (id, token) = state
        .registry
        .create(&req.name, req.password)
        .map_err(ApiError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(CreateTournamentResponse { id, token }),
    ))
}
