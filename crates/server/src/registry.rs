//! The tournament registry's own top-level routes: list, create, and import.
//!
//! Unlike everything under `/api/tournaments/{id}/...` (see [`crate::tournament`]),
//! these aren't scoped to one tournament — `GET` has to work before you've
//! picked one, and the two `POST`s are what mint new ones.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use osp_core::Tournament;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::{AppState, TournamentSummary};

/// `GET /api/tournaments`: list every known tournament. Public — needed to
/// render the picker before anyone has logged in anywhere.
pub fn public_routes() -> Router<AppState> {
    Router::new().route("/api/tournaments", get(list_tournaments))
}

/// `POST /api/tournaments` (create a new, empty tournament) and
/// `POST /api/tournaments/import` (create one from a save file). Callers are
/// expected to wrap these with [`crate::auth::require_admin_auth`] (see
/// `lib.rs`) — minting a tournament, by either route, is gated by the admin
/// password if one is configured.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/api/tournaments", post(create_tournament))
        .route("/api/tournaments/import", post(import_tournament))
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

/// Body of `POST /api/tournaments/import`.
///
/// The save file is held as a [`RawValue`] rather than a `Tournament` so its
/// bytes survive the wrapper's deserialize untouched, letting
/// [`check_format_version`](crate::state::check_format_version) probe them
/// *before* anything tries to parse the tournament itself. That ordering is the
/// entire point of the probe: a format bump can reshape a field, so a full
/// parse of an old file fails deep inside that field with an error naming
/// anything but the real cause.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportTournamentRequest {
    /// The save file, verbatim.
    tournament: Box<RawValue>,
    /// The imported tournament's own password; `None`/absent leaves it open.
    #[serde(default)]
    password: Option<String>,
}

/// `POST /api/tournaments/import`: create a tournament from an uploaded save
/// file, in one step.
///
/// Deliberately atomic. Importing used to be create-then-upload from the
/// client, which meant a file this build cannot read (the common case: a save
/// from an older version) was only rejected *after* an empty tournament had
/// already been registered, leaving the referee with an orphan to delete by
/// hand. Nothing is registered here unless the file is accepted whole.
async fn import_tournament(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<CreateTournamentResponse>), ApiError> {
    let req: ImportTournamentRequest = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("could not parse import request: {e}")))?;
    // Version first, for the reason spelled out on `ImportTournamentRequest`.
    crate::state::check_format_version(req.tournament.get().as_bytes())?;
    let tournament: Tournament = serde_json::from_str(req.tournament.get())
        .map_err(|e| ApiError::BadRequest(format!("could not parse tournament: {e}")))?;
    // The only gate between an untrusted file and the registry.
    tournament.validate_loaded()?;
    let (id, token) = state.registry.insert(tournament, req.password);
    Ok((
        StatusCode::CREATED,
        Json(CreateTournamentResponse { id, token }),
    ))
}
