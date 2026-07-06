//! Tournament API: create, fetch, replace (load), player CRUD, and undo.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{post, put};
use axum::{Json, Router};
use osp_core::{Board, Handicap, NewPlayer, Tournament, Winner};
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
/// - `POST   /api/tournament/finalize-registration`  finalize registration
/// - `POST   /api/tournament/complete-round`         complete the current round
/// - `POST   /api/tournament/rounds/prepare`         begin drafting the next round
/// - `PUT    /api/tournament/draft`                  edit the draft
/// - `POST   /api/tournament/rounds`                 confirm the draft (pair & start)
/// - `POST   /api/tournament/rounds/{n}/boards/{i}/result`  toggle a board winner
/// - `POST   /api/tournament/rounds/{n}/boards/{i}/drawn`   set the draw flag
/// - `PUT    /api/tournament/rounds/{n}/boards/{i}/handicap` set/clear the handicap
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
        .route(
            "/api/tournament/finalize-registration",
            post(finalize_registration),
        )
        .route("/api/tournament/complete-round", post(complete_round))
        .route("/api/tournament/rounds/prepare", post(prepare_round))
        .route("/api/tournament/draft", put(update_draft))
        .route("/api/tournament/rounds", post(confirm_round))
        .route(
            "/api/tournament/rounds/{round_number}/boards/{board_index}/result",
            post(set_board_result),
        )
        .route(
            "/api/tournament/rounds/{round_number}/boards/{board_index}/drawn",
            post(set_board_drawn),
        )
        .route(
            "/api/tournament/rounds/{round_number}/boards/{board_index}/handicap",
            put(set_board_handicap),
        )
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

/// Finalize registration (prerequisite for starting the first round).
async fn finalize_registration(
    State(state): State<AppState>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.finalize_registration())?;
    view(&store)
}

/// Complete the current (in-progress) round.
async fn complete_round(
    State(state): State<AppState>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.complete_current_round())?;
    view(&store)
}

/// Begin drafting the next round (enters the round-draft state).
async fn prepare_round(
    State(state): State<AppState>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.prepare_round().map(|_| ()))?;
    view(&store)
}

/// Body of `PUT /api/tournament/draft`: the draft's customization.
#[derive(Debug, Deserialize)]
struct DraftUpdate {
    #[serde(default)]
    absent: Vec<Uuid>,
    /// Forced pairings; only `player1`/`player2` are read (result is ignored).
    #[serde(default)]
    forced_boards: Vec<Board>,
    #[serde(default)]
    forced_bye: Option<Uuid>,
}

/// Edit the current draft (absent set, forced pairings, forced bye).
async fn update_draft(
    State(state): State<AppState>,
    Json(req): Json<DraftUpdate>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| {
        t.update_draft(req.absent, req.forced_boards, req.forced_bye)
            .map(|_| ())
    })?;
    view(&store)
}

/// Confirm the draft: pair the remaining players and start the round.
async fn confirm_round(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<TournamentView>), ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.confirm_round().map(|_| ()))?;
    Ok((StatusCode::CREATED, view(&store)?))
}

/// Body of the board-result endpoint: which player the referee clicked.
#[derive(Debug, Deserialize)]
struct SetResultRequest {
    clicked: Winner,
}

/// Toggle a board's winner in response to a clicked player.
async fn set_board_result(
    State(state): State<AppState>,
    Path((round_number, board_index)): Path<(u32, usize)>,
    Json(req): Json<SetResultRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| {
        t.toggle_board_winner(round_number, board_index, req.clicked)
            .map(|_| ())
    })?;
    view(&store)
}

/// Body of the draw-flag endpoint: whether the game was drawn before resolving.
#[derive(Debug, Deserialize)]
struct SetDrawnRequest {
    drawn: bool,
}

/// Set (or clear) a board's "a draw occurred" flag.
async fn set_board_drawn(
    State(state): State<AppState>,
    Path((round_number, board_index)): Path<(u32, usize)>,
    Json(req): Json<SetDrawnRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| {
        t.set_board_drawn(round_number, board_index, req.drawn)
            .map(|_| ())
    })?;
    view(&store)
}

/// Body of the handicap endpoint: the handicap to set, or `null` to clear it.
#[derive(Debug, Deserialize)]
struct SetHandicapRequest {
    handicap: Option<Handicap>,
}

/// Set or clear a board's handicap. The giver is frozen server-side from the
/// players' current ratings.
async fn set_board_handicap(
    State(state): State<AppState>,
    Path((round_number, board_index)): Path<(u32, usize)>,
    Json(req): Json<SetHandicapRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| {
        t.set_board_handicap(round_number, board_index, req.handicap)
            .map(|_| ())
    })?;
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
