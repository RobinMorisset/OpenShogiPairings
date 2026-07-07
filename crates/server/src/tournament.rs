//! Tournament API: create, fetch, replace (load), player CRUD, and undo.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use osp_core::{
    Board, Counterfactual, CounterfactualMode, CupPodium, Handicap, NewPlayer, RoundExplanation,
    Standing, Tournament, TournamentSettings, Winner,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backup;
use crate::error::ApiError;
use crate::state::{AppState, TournamentStore};

/// Register the tournament routes onto a router.
///
/// - `POST   /api/tournament`               create a new (empty) tournament
/// - `GET    /api/tournament`               fetch the current tournament
/// - `PUT    /api/tournament`               replace the current tournament (load)
/// - `POST   /api/tournament/undo`          revert the last player change
/// - `GET    /api/tournament/american-grid` export the cross-table for ELO (text)
/// - `PUT    /api/tournament/american-grid` import a cross-table, rebuilding it (text)
/// - `PUT    /api/tournament/settings`      update tournament settings (MacMahon, …)
/// - `POST   /api/tournament/finalize-registration`  finalize registration
/// - `POST   /api/tournament/complete-round`         complete the current round
/// - `POST   /api/tournament/cancel-round`           cancel the last round (or draft)
/// - `POST   /api/tournament/rounds/prepare`         begin drafting the next round
/// - `PUT    /api/tournament/draft`                  edit the draft
/// - `POST   /api/tournament/rounds`                 confirm the draft (pair & start)
/// - `GET    /api/tournament/rounds/{n}/explanation` explain a round's Swiss pairings
/// - `POST   /api/tournament/rounds/{n}/counterfactual` explain forcing/forbidding a pairing
/// - `POST   /api/tournament/rounds/force-pairing`      re-pair the round with a forced board
/// - `POST   /api/tournament/rounds/{n}/boards/{i}/result`  toggle a board winner
/// - `POST   /api/tournament/rounds/{n}/boards/{i}/drawn`   set the draw flag
/// - `PUT    /api/tournament/rounds/{n}/boards/{i}/handicap` set/clear the handicap
/// - `POST   /api/tournament/players`       register a player
/// - `PUT    /api/tournament/players/{id}`  edit a player
/// - `DELETE /api/tournament/players/{id}`  remove a player
/// - `POST   /api/tournament/players/{id}/eligible`  set cup eligibility
/// - `POST   /api/tournament/players/{id}/adjustments`             add a manual point bonus/malus
/// - `DELETE /api/tournament/players/{id}/adjustments/{adjustment_id}` remove one
/// - `GET    /api/tournament/backups`         list automatic backups, newest first
/// - `POST   /api/tournament/backups/{id}/restore` restore a backup as the current tournament
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
            "/api/tournament/american-grid",
            get(american_grid).put(import_american_grid),
        )
        .route("/api/tournament/settings", put(update_settings))
        .route(
            "/api/tournament/finalize-registration",
            post(finalize_registration),
        )
        .route("/api/tournament/complete-round", post(complete_round))
        .route("/api/tournament/cancel-round", post(cancel_round))
        .route("/api/tournament/rounds/prepare", post(prepare_round))
        .route("/api/tournament/draft", put(update_draft))
        .route("/api/tournament/rounds", post(confirm_round))
        .route(
            "/api/tournament/rounds/{round_number}/explanation",
            get(round_explanation),
        )
        .route(
            "/api/tournament/rounds/{round_number}/counterfactual",
            post(round_counterfactual),
        )
        .route("/api/tournament/rounds/force-pairing", post(force_pairing))
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
        .route(
            "/api/tournament/players/{id}/eligible",
            post(set_player_eligible),
        )
        .route(
            "/api/tournament/players/{id}/adjustments",
            post(add_point_adjustment),
        )
        .route(
            "/api/tournament/players/{id}/adjustments/{adjustment_id}",
            axum::routing::delete(remove_point_adjustment),
        )
        .route("/api/tournament/backups", get(list_backups))
        .route("/api/tournament/backups/{id}/restore", post(restore_backup))
}

/// API response: the current tournament, undo availability, and the derived
/// standings.
///
/// Deliberately *not* the bare `Tournament` (which is what save/load uses) — the
/// `can_undo` flag is server session state and `standings` is derived, both kept
/// out of the persisted shape. Standings are computed server-side so every
/// client (and the future American grid) shares one ranking.
#[derive(Serialize)]
struct TournamentView {
    tournament: Tournament,
    can_undo: bool,
    standings: Vec<Standing>,
    /// The cup podium once decided (champion / runner-up / third / fourth), for the
    /// Results-tab medals. `None` when there is no cup or the final isn't finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    cup_podium: Option<CupPodium>,
    /// Players the cup will pair in the round being drafted, so the draft UI can
    /// keep them out of the Swiss customization. Empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    draft_cup_players: Vec<uuid::Uuid>,
    /// Suggested handicap per board, indexed like `tournament.rounds[i].boards[j]`.
    /// Computed from current ratings regardless of `handicap_policy` — the
    /// frontend decides how to surface it. `None` = no suggestion (near-equal
    /// strength, an unrated player, or a cup board).
    suggested_handicaps: Vec<Vec<Option<Handicap>>>,
}

/// Build the view from the store, or 404 if no tournament exists.
fn view(store: &TournamentStore) -> Result<Json<TournamentView>, ApiError> {
    let tournament = store.current().cloned().ok_or(ApiError::NoTournament)?;
    let standings = tournament.standings();
    let cup_podium = tournament.cup_podium();
    let draft_cup_players = tournament.draft_cup_players();
    let suggested_handicaps = tournament
        .rounds
        .iter()
        .map(|round| {
            round
                .boards
                .iter()
                .map(|board| tournament.suggested_handicap_for_board(board))
                .collect()
        })
        .collect();
    Ok(Json(TournamentView {
        tournament,
        can_undo: store.can_undo(),
        standings,
        cup_podium,
        draft_cup_players,
        suggested_handicaps,
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

/// List automatic backups for the current tournament, newest first (see
/// [`backup`]). 404 if there is no current tournament (there is nothing to
/// scope the list to).
async fn list_backups(
    State(state): State<AppState>,
) -> Result<Json<Vec<backup::BackupInfo>>, ApiError> {
    let store = state.store.read().expect("store lock poisoned");
    let tournament = store.current().ok_or(ApiError::NoTournament)?;
    Ok(Json(backup::list(tournament.id)))
}

/// Restore a backup as the current tournament — like "load", but from the
/// server's own backup store rather than an uploaded file. Resets undo
/// history.
async fn restore_backup(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    let tournament_id = store.current().ok_or(ApiError::NoTournament)?.id;
    let restored = backup::load(tournament_id, &id)
        .ok_or_else(|| ApiError::NotFound(format!("no backup {id}")))?;
    store.set_current(restored);
    view(&store)
}

/// Export the tournament as an American Grid document (`text/plain`).
///
/// Built from the same server-computed standings as [`view`], so the grid's
/// final-rank ordering matches the Results tab. Unlike the other endpoints this
/// returns the raw grid text rather than a [`TournamentView`].
async fn american_grid(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let store = state.store.read().expect("store lock poisoned");
    let tournament = store.current().ok_or(ApiError::NoTournament)?;
    let grid = osp_core::american_grid(tournament, &tournament.standings());
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], grid))
}

/// Import an American Grid document (raw text body), rebuilding it into the
/// current tournament — replacing whatever was there. Meant for quickly seeding a
/// non-trivial tournament state in tests and simulations. Returns the rebuilt
/// [`TournamentView`].
async fn import_american_grid(
    State(state): State<AppState>,
    body: String,
) -> Result<Json<TournamentView>, ApiError> {
    let tournament = osp_core::import_american_grid(&body)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let mut store = state.store.write().expect("store lock poisoned");
    store.set_current(tournament);
    view(&store)
}

/// Update the tournament settings. Takes the whole [`TournamentSettings`] object
/// (MacMahon thresholds, degressive schedule, …) and stores it normalized so the
/// surface grows without changing the endpoint shape.
async fn update_settings(
    State(state): State<AppState>,
    Json(settings): Json<TournamentSettings>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(move |t| {
        t.update_settings(settings);
        Ok(())
    })?;
    view(&store)
}

/// Body of the finalize endpoint: the chosen cup size, if the cup is enabled.
#[derive(Debug, Default, Deserialize)]
struct FinalizeRequest {
    #[serde(default)]
    cup_size: Option<u32>,
}

/// Finalize registration (prerequisite for starting the first round). When the
/// cup is enabled the body carries the chosen size; otherwise the body is empty.
async fn finalize_registration(
    State(state): State<AppState>,
    body: Option<Json<FinalizeRequest>>,
) -> Result<Json<TournamentView>, ApiError> {
    let cup_size = body.and_then(|Json(b)| b.cup_size);
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.finalize_registration_with(cup_size))?;
    backup_after(&store, "registration finalized");
    view(&store)
}

/// Complete the current (in-progress) round.
async fn complete_round(
    State(state): State<AppState>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.complete_current_round())?;
    let label = round_label(&store, "round", "completed");
    backup_after(&store, &label);
    view(&store)
}

/// Cancel the last round (or the open draft), stepping the tournament back one
/// stage. Undoable like any other mutation.
async fn cancel_round(
    State(state): State<AppState>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.cancel_last_round())?;
    backup_after(&store, "round cancelled");
    view(&store)
}

/// Begin drafting the next round (enters the round-draft state).
async fn prepare_round(
    State(state): State<AppState>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.prepare_round().map(|_| ()))?;
    let label = store
        .current()
        .and_then(|t| t.draft.as_ref())
        .map(|d| format!("round {} drafting", d.number))
        .unwrap_or_else(|| "round drafting".to_string());
    backup_after(&store, &label);
    view(&store)
}

/// A round-numbered backup label, e.g. "round 3 completed" — falls back to
/// `noun` alone if there is (unexpectedly) no last round to number.
fn round_label(store: &TournamentStore, noun: &str, verb: &str) -> String {
    match store.current().and_then(|t| t.rounds.last()) {
        Some(round) => format!("{noun} {} {verb}", round.number),
        None => format!("{noun} {verb}"),
    }
}

/// Take a backup of the current tournament, if any. Best-effort and silent on
/// failure (logged inside [`backup::take`]) — a backup problem must never
/// surface as an API error.
fn backup_after(store: &TournamentStore, label: &str) {
    if let Some(tournament) = store.current() {
        backup::take(tournament, label);
    }
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
    let label = round_label(&store, "round", "started");
    backup_after(&store, &label);
    Ok((StatusCode::CREATED, view(&store)?))
}

/// Explain a round's Swiss pairings: per-board rule ledger and round report.
/// Read-only, so no backup is taken.
async fn round_explanation(
    State(state): State<AppState>,
    Path(round_number): Path<u32>,
) -> Result<Json<RoundExplanation>, ApiError> {
    let store = state.store.read().expect("store lock poisoned");
    let tournament = store.current().ok_or(ApiError::NoTournament)?;
    let explanation = tournament.explain_round(round_number)?;
    Ok(Json(explanation))
}

/// Body of the counterfactual endpoint: the two players to probe and the mode —
/// `"force"` ("why aren't these two paired?") or `"forbid"` ("why did you pair
/// them?"). Defaults to force.
#[derive(Debug, Deserialize)]
struct CounterfactualRequest {
    #[serde(default = "default_counterfactual_mode")]
    mode: CounterfactualMode,
    a: Uuid,
    b: Uuid,
}

fn default_counterfactual_mode() -> CounterfactualMode {
    CounterfactualMode::Force
}

/// Explain what forcing or forbidding the pairing `a`–`b` in this round would
/// cost. Read-only.
async fn round_counterfactual(
    State(state): State<AppState>,
    Path(round_number): Path<u32>,
    Json(req): Json<CounterfactualRequest>,
) -> Result<Json<Counterfactual>, ApiError> {
    if req.a == req.b {
        return Err(ApiError::BadRequest(
            "a counterfactual needs two different players".into(),
        ));
    }
    let store = state.store.read().expect("store lock poisoned");
    let tournament = store.current().ok_or(ApiError::NoTournament)?;
    let result = tournament.explain_counterfactual(round_number, req.a, req.b, req.mode)?;
    Ok(Json(result))
}

/// Body of the force-pairing endpoint: the two players to pair.
#[derive(Debug, Deserialize)]
struct ForcePairingRequest {
    a: Uuid,
    b: Uuid,
}

/// Force the pairing `a`–`b` onto the current round (re-pairs it with that board
/// fixed). Mutates, so a backup is taken.
async fn force_pairing(
    State(state): State<AppState>,
    Json(req): Json<ForcePairingRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    if req.a == req.b {
        return Err(ApiError::BadRequest(
            "a forced pairing needs two different players".into(),
        ));
    }
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.force_pairing(req.a, req.b).map(|_| ()))?;
    let label = round_label(&store, "round", "re-paired");
    backup_after(&store, &label);
    view(&store)
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

/// Body of the eligibility endpoint: the new cup-eligibility flag.
#[derive(Debug, Deserialize)]
struct SetEligibleRequest {
    eligible: bool,
}

/// Set whether a player is eligible for the direct-elimination cup.
async fn set_player_eligible(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SetEligibleRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.set_player_eligible(id, req.eligible).map(|_| ()))?;
    view(&store)
}

/// Body of the point-adjustment endpoint: the delta and its mandatory reason.
#[derive(Debug, Deserialize)]
struct AddAdjustmentRequest {
    delta: i32,
    reason: String,
}

/// Apply a manual point bonus/malus to a player.
async fn add_point_adjustment(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddAdjustmentRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.add_point_adjustment(id, req.delta, req.reason).map(|_| ()))?;
    view(&store)
}

/// Remove a previously applied point adjustment.
async fn remove_point_adjustment(
    State(state): State<AppState>,
    Path((id, adjustment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = state.store.write().expect("store lock poisoned");
    store.mutate(|t| t.remove_point_adjustment(id, adjustment_id).map(|_| ()))?;
    view(&store)
}
