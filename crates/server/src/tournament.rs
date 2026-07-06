//! Tournament API: create, fetch, replace (load), and player registration.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use osp_core::{NewPlayer, Tournament};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

/// Register the tournament routes onto a router.
///
/// - `POST   /api/tournament`            create a new (empty) tournament
/// - `GET    /api/tournament`            fetch the current tournament
/// - `PUT    /api/tournament`            replace the current tournament (load)
/// - `POST   /api/tournament/players`    register a player
/// - `DELETE /api/tournament/players/{id}` remove a player
///
/// Every mutating endpoint returns the full updated [`Tournament`] so clients
/// can refresh their view from a single response.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/tournament",
            post(create_tournament)
                .get(get_tournament)
                .put(replace_tournament),
        )
        .route("/api/tournament/players", post(add_player))
        .route("/api/tournament/players/{id}", axum::routing::delete(remove_player))
}

/// Body of `POST /api/tournament`.
#[derive(Debug, Deserialize)]
struct CreateTournamentRequest {
    name: String,
}

/// Create a new, empty tournament, replacing any existing one.
async fn create_tournament(
    State(state): State<AppState>,
    Json(req): Json<CreateTournamentRequest>,
) -> Result<(StatusCode, Json<Tournament>), ApiError> {
    let tournament = Tournament::new(&req.name)?;
    let mut guard = state.tournament.write().expect("tournament lock poisoned");
    *guard = Some(tournament.clone());
    Ok((StatusCode::CREATED, Json(tournament)))
}

/// Fetch the current tournament, or 404 if none exists.
async fn get_tournament(State(state): State<AppState>) -> Result<Json<Tournament>, ApiError> {
    let guard = state.tournament.read().expect("tournament lock poisoned");
    guard.clone().map(Json).ok_or(ApiError::NoTournament)
}

/// Replace the current tournament wholesale. Used by "load" (file upload):
/// the client uploads a previously-saved tournament and it becomes current.
async fn replace_tournament(
    State(state): State<AppState>,
    Json(tournament): Json<Tournament>,
) -> Result<Json<Tournament>, ApiError> {
    tournament.validate_loaded()?;
    let mut guard = state.tournament.write().expect("tournament lock poisoned");
    *guard = Some(tournament.clone());
    Ok(Json(tournament))
}

/// Register a player in the current tournament.
async fn add_player(
    State(state): State<AppState>,
    Json(new_player): Json<NewPlayer>,
) -> Result<(StatusCode, Json<Tournament>), ApiError> {
    let mut guard = state.tournament.write().expect("tournament lock poisoned");
    let tournament = guard.as_mut().ok_or(ApiError::NoTournament)?;
    tournament.add_player(new_player)?;
    Ok((StatusCode::CREATED, Json(tournament.clone())))
}

/// Remove a player from the current tournament by id.
async fn remove_player(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Tournament>, ApiError> {
    let mut guard = state.tournament.write().expect("tournament lock poisoned");
    let tournament = guard.as_mut().ok_or(ApiError::NoTournament)?;
    tournament.remove_player(id)?;
    Ok(Json(tournament.clone()))
}
