//! Tournament API: create, fetch, replace (load), player CRUD, and undo.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use osp_core::{NewPlayer, Tournament};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::{AppState, TournamentStore};

/// Register the tournament routes onto a router.
///
/// - `POST   /api/tournament`               create a new (empty) tournament
/// - `GET    /api/tournament`               fetch the current tournament
/// - `PUT    /api/tournament`               replace the current tournament (load)
/// - `POST   /api/tournament/undo`          revert the last player change
/// - `POST   /api/tournament/players`       register a player
/// - `PUT    /api/tournament/players/{id}`  edit a player
/// - `DELETE /api/tournament/players/{id}`  remove a player
///
/// Every endpoint returns a [`TournamentView`] (the tournament plus whether an
/// undo is available), so clients can refresh their view and the undo button
/// from a single response.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/tournament",
            post(create_tournament)
                .get(get_tournament)
                .put(replace_tournament),
        )
        .route("/api/tournament/undo", post(undo))
        .route("/api/tournament/players", post(add_player))
        .route(
            "/api/tournament/players/{id}",
            axum::routing::put(edit_player).delete(remove_player),
        )
}

/// API response: the current tournament plus undo availability.
///
/// Deliberately *not* the bare `Tournament` (which is what save/load uses) — the
/// `can_undo` flag is server session state, kept out of the persisted shape.
#[derive(Serialize)]
struct TournamentView {
    tournament: Tournament,
    can_undo: bool,
}

/// Build the view from the store, or 404 if no tournament exists.
fn view(store: &TournamentStore) -> Result<Json<TournamentView>, ApiError> {
    let tournament = store.current().cloned().ok_or(ApiError::NoTournament)?;
    Ok(Json(TournamentView {
        tournament,
        can_undo: store.can_undo(),
    }))
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
) -> Result<(StatusCode, Json<TournamentView>), ApiError> {
    let tournament = Tournament::new(&req.name)?;
    let mut store = state.store.write().expect("store lock poisoned");
    store.set_current(tournament);
    Ok((StatusCode::CREATED, view(&store)?))
}

/// Fetch the current tournament, or 404 if none exists.
async fn get_tournament(State(state): State<AppState>) -> Result<Json<TournamentView>, ApiError> {
    let store = state.store.read().expect("store lock poisoned");
    view(&store)
}

/// Replace the current tournament wholesale (used by "load"). Resets undo history.
async fn replace_tournament(
    State(state): State<AppState>,
    Json(tournament): Json<Tournament>,
) -> Result<Json<TournamentView>, ApiError> {
    tournament.validate_loaded()?;
    let mut store = state.store.write().expect("store lock poisoned");
    store.set_current(tournament);
    view(&store)
}

/// Revert the last player change.
async fn undo(State(state): State<AppState>) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    if store.current().is_none() {
        return Err(ApiError::NoTournament);
    }
    store.undo();
    view(&store)
}

/// Register a player in the current tournament.
async fn add_player(
    State(state): State<AppState>,
    Json(new_player): Json<NewPlayer>,
) -> Result<(StatusCode, Json<TournamentView>), ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.add_player(new_player).map(|_| ()))?;
    Ok((StatusCode::CREATED, view(&store)?))
}

/// Edit an existing player's fields (in-place cell editing).
async fn edit_player(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(new_player): Json<NewPlayer>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.edit_player(id, new_player).map(|_| ()))?;
    view(&store)
}

/// Remove a player from the current tournament by id.
async fn remove_player(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.remove_player(id))?;
    view(&store)
}
