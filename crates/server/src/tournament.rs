//! Per-tournament API: fetch, replace (load), delete, player CRUD, rounds, and
//! undo — everything nested under `/api/tournaments/{id}`.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use osp_core::{
    Board, Counterfactual, CounterfactualMode, CupBracketView, CupPodium, Handicap, NewPlayer,
    NoShow, RoundExplanation, SitoutValue, Standing, Tournament, TournamentId, TournamentSettings,
    Winner,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backup;
use crate::error::ApiError;
use crate::live::ExpectedVersion;
use crate::ratings;
use crate::scope::TournamentCtx;
use crate::state::{AppState, TournamentStore};
use crate::{auth, live};

/// Build the router nested at `/api/tournaments/{id}`.
///
/// Split into a public group (login, the SSE stream — both need to resolve the
/// tournament but must work without a token already) and a protected group
/// (everything else): the protected group's `route_layer`s run auth before the
/// version check, and both need [`Path`]-param access to the `{id}` segment,
/// which requires `route_layer` (not `layer`) — see axum's docs on the
/// difference.
///
/// - `GET    /`               fetch the tournament
/// - `DELETE /`               delete the tournament
/// - `POST   /undo`           revert the last player change
/// - `GET    /american-grid` export the cross-table for ELO (text)
/// - `PUT    /american-grid` import a cross-table, rebuilding the tournament (text)
/// - `PUT    /settings`      update tournament settings (MacMahon, …)
/// - `POST   /cancel-round`           cancel the last round (or draft)
/// - `POST   /rounds/prepare`         begin drafting the next round
/// - `PUT    /draft`                  edit the draft
/// - `POST   /rounds`                 confirm the draft (pair & start)
/// - `GET    /rounds/{n}/explanation` explain a round's Swiss pairings
/// - `POST   /rounds/{n}/counterfactual` explain forcing/forbidding a pairing
/// - `POST   /rounds/force-pairing`      re-pair the round with a forced board
/// - `POST   /rounds/{n}/boards/{i}/result`  toggle a board winner
/// - `POST   /rounds/{n}/boards/{i}/drawn`   set the draw flag
/// - `POST   /rounds/{n}/boards/{i}/no-show` set/clear a board no-show
/// - `PUT    /rounds/{n}/boards/{i}/handicap` set/clear the handicap
/// - `POST   /rounds/{n}/boards/{i}/long`    flag/unflag a two-round long game
/// - `POST   /players`       register a player
/// - `POST   /players/batch` register many players as a single mutation (CSV import)
/// - `PUT    /players/{player_id}`  edit a player
/// - `DELETE /players/{player_id}`  remove a player
/// - `POST   /players/{player_id}/eligible`  set cup eligibility
/// - `POST   /players/{player_id}/category`  set membership in a player category
/// - `POST   /players/{player_id}/adjustments`             add a manual point bonus/malus
/// - `DELETE /players/{player_id}/adjustments/{adjustment_id}` remove one
/// - `GET    /backups`         list automatic backups, newest first
/// - `POST   /backups/{backup_id}/restore` restore a backup as the current tournament
/// - `POST   /login`  exchange this tournament's password for a session token
/// - `GET    /events` SSE stream of this tournament's change version
///
/// Every endpoint (except login/events/the text exports) returns a
/// [`TournamentView`] (the tournament plus whether an undo is available), so
/// clients can refresh their view and the undo button from a single response.
pub fn scope(state: AppState) -> Router<AppState> {
    let public = Router::new()
        .route("/login", post(auth::tournament_login))
        .route("/events", get(live::events));

    let protected = Router::new()
        .route("/", get(get_tournament).delete(delete_tournament))
        .route("/undo", post(undo))
        .route(
            "/american-grid",
            get(american_grid).put(import_american_grid),
        )
        .route("/settings", put(update_settings))
        .route("/cancel-round", post(cancel_round))
        .route("/rounds/prepare", post(prepare_round))
        .route("/draft", put(update_draft))
        .route("/rounds", post(confirm_round))
        .route("/rounds/{round_number}/explanation", get(round_explanation))
        .route(
            "/rounds/{round_number}/counterfactual",
            post(round_counterfactual),
        )
        .route("/rounds/force-pairing", post(force_pairing))
        .route(
            "/rounds/{round_number}/boards/{board_index}/result",
            post(set_board_result),
        )
        .route(
            "/rounds/{round_number}/boards/{board_index}/drawn",
            post(set_board_drawn),
        )
        .route(
            "/rounds/{round_number}/boards/{board_index}/no-show",
            post(set_board_no_show),
        )
        .route(
            "/rounds/{round_number}/boards/{board_index}/handicap",
            put(set_board_handicap),
        )
        .route(
            "/rounds/{round_number}/boards/{board_index}/long",
            post(set_board_long),
        )
        .route(
            "/rounds/{round_number}/sitouts/{player}",
            put(set_sitout_value),
        )
        .route("/players", post(add_player))
        .route("/players/batch", post(add_players_batch))
        .route("/players/import-csv", post(import_players_csv))
        .route(
            "/players/{player_id}",
            axum::routing::put(edit_player).delete(remove_player),
        )
        .route("/players/{player_id}/eligible", post(set_player_eligible))
        .route("/players/{player_id}/category", post(set_player_category))
        .route(
            "/players/{player_id}/adjustments",
            post(add_point_adjustment),
        )
        .route(
            "/players/{player_id}/adjustments/{adjustment_id}",
            axum::routing::delete(remove_point_adjustment),
        )
        .route("/backups", get(list_backups))
        .route("/backups/{backup_id}/restore", post(restore_backup))
        // `route_layer` (not `layer`): needed so the `{id}` path param is
        // available to these middlewares' own `TournamentCtx` extraction.
        // Layers wrap outermost-last, so auth runs before the version check.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            live::check_version,
        ))
        .route_layer(middleware::from_fn_with_state(
            state,
            auth::require_tournament_auth,
        ));

    public.merge(protected)
}

/// API response: the current tournament, undo availability, and the derived
/// standings.
///
/// Deliberately *not* the bare `Tournament` (which is what save/load uses) — the
/// `can_undo` flag is server session state and `standings` is derived, both kept
/// out of the persisted shape. Standings are computed server-side so every
/// client (and the future American grid) shares one ranking.
#[derive(Serialize, ts_rs::TS)]
#[ts(
    export,
    rename = "TournamentResponse",
    export_to = "../../../frontend/src/lib/generated/"
)]
struct TournamentView {
    tournament: Tournament,
    can_undo: bool,
    /// Monotonic change version. Clients echo it in the `X-Tournament-Version`
    /// header so a stale edit is rejected (409), and use it to ignore the SSE
    /// echo of their own change (see [`crate::live`]).
    version: u32,
    standings: Vec<Standing>,
    /// The cup podium once decided (champion / runner-up / third / fourth), for the
    /// Results-tab medals. `None` when there is no cup or the final isn't finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    cup_podium: Option<CupPodium>,
    /// The full cup bracket (structure + results), derived server-side so the Cup
    /// tab renders it directly. `None` when there is no cup.
    #[serde(skip_serializing_if = "Option::is_none")]
    cup_bracket: Option<CupBracketView>,
    /// Players the cup will pair in the round being drafted, so the draft UI can
    /// keep them out of the Swiss customization. Empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    draft_cup_players: Vec<TournamentId>,
    /// Suggested handicap per board, indexed like `tournament.rounds[i].boards[j]`.
    /// Computed from current ratings regardless of `handicap_policy` — the
    /// frontend decides how to surface it. `None` = no suggestion (near-equal
    /// strength, an unrated player, or a cup board).
    suggested_handicaps: Vec<Vec<Option<Handicap>>>,
    /// The winner that counts for standings/pairing per board (see
    /// [`osp_core::round::Board::effective_winner`]), indexed like
    /// `tournament.rounds[i].boards[j]`. Computed here — using the tournament's
    /// `handicap_wiel_rule` setting — so the frontend never has to re-derive it.
    /// `None` while a board is undecided.
    effective_winners: Vec<Vec<Option<Winner>>>,
}

/// Build the view from the store, or 404 if no tournament exists.
fn view(store: &TournamentStore) -> Result<Json<TournamentView>, ApiError> {
    let tournament = store.current().cloned().ok_or(ApiError::NoTournament)?;
    let standings = tournament.standings();
    let cup_podium = tournament.cup_podium();
    let cup_bracket = tournament.cup_bracket();
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
    let effective_winners = tournament
        .rounds
        .iter()
        .map(|round| {
            round
                .boards
                .iter()
                .map(|board| board.effective_winner(tournament.settings.handicap_wiel_rule()))
                .collect()
        })
        .collect();
    Ok(Json(TournamentView {
        tournament,
        can_undo: store.can_undo(),
        version: store.version(),
        standings,
        cup_podium,
        cup_bracket,
        draft_cup_players,
        suggested_handicaps,
        effective_winners,
    }))
}

/// Fetch the tournament.
async fn get_tournament(
    TournamentCtx(instance): TournamentCtx,
) -> Result<Json<TournamentView>, ApiError> {
    let store = instance.read();
    view(&store)
}

/// Delete the tournament: its registry entry, persisted file, and backups.
async fn delete_tournament(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    if state.registry.remove(id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("no tournament {id}")))
    }
}

/// Revert the last player change.
async fn undo(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    if store.current().is_none() {
        return Err(ApiError::NoTournament);
    }
    store.ensure_current_version(expected)?;
    store.undo();
    view(&store)
}

/// List automatic backups for the tournament, newest first (see [`backup`]).
async fn list_backups(
    TournamentCtx(instance): TournamentCtx,
) -> Result<Json<Vec<backup::BackupInfo>>, ApiError> {
    let store = instance.read();
    let tournament = store.current().ok_or(ApiError::NoTournament)?;
    Ok(Json(backup::list(tournament.id)))
}

/// The backup's own id from the path. Named (not positional/tuple) so this
/// tolerates the outer `{id}` (tournament) segment also present in the
/// matched route — [`Path`]'s struct form ignores fields it doesn't declare.
#[derive(Deserialize)]
struct BackupParams {
    backup_id: String,
}

/// Restore a backup as the current tournament — like "load", but from the
/// server's own backup store rather than an uploaded file. Resets undo
/// history.
async fn restore_backup(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<BackupParams>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.ensure_current_version(expected)?;
    let tournament_id = store.current().ok_or(ApiError::NoTournament)?.id;
    let restored = backup::load(tournament_id, &params.backup_id)
        .ok_or_else(|| ApiError::NotFound(format!("no backup {}", params.backup_id)))?;
    store.set_current(restored);
    view(&store)
}

/// Export the tournament as an American Grid document (`text/plain`).
///
/// Built from the same server-computed standings as [`view`], so the grid's
/// final-rank ordering matches the Results tab. Unlike the other endpoints this
/// returns the raw grid text rather than a [`TournamentView`].
async fn american_grid(
    TournamentCtx(instance): TournamentCtx,
) -> Result<impl IntoResponse, ApiError> {
    let store = instance.read();
    let tournament = store.current().ok_or(ApiError::NoTournament)?;
    let grid = osp_core::american_grid(tournament, &tournament.standings());
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], grid))
}

/// Import an American Grid document (raw text body), rebuilding it into the
/// tournament — replacing whatever was there (keeping this instance's id).
/// Meant for quickly seeding a non-trivial tournament state in tests and
/// simulations. Returns the rebuilt [`TournamentView`].
async fn import_american_grid(
    TournamentCtx(instance): TournamentCtx,
    Path(id): Path<Uuid>,
    ExpectedVersion(expected): ExpectedVersion,
    body: String,
) -> Result<Json<TournamentView>, ApiError> {
    let mut tournament =
        osp_core::import_american_grid(&body).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    tournament.id = id;
    let mut store = instance.write();
    store.ensure_current_version(expected)?;
    store.set_current(tournament);
    view(&store)
}

/// Update the tournament settings. Takes the whole [`TournamentSettings`] object
/// (MacMahon thresholds, degressive schedule, …) and stores it normalized so the
/// surface grows without changing the endpoint shape.
async fn update_settings(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Json(settings): Json<TournamentSettings>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, move |t| {
        t.update_settings(settings)?;
        Ok(())
    })?;
    view(&store)
}

/// Body of the round-preparation endpoint: the chosen cup size, if the cup is
/// enabled (only meaningful for round 1, which finalizes registration).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FinalizeRequest {
    #[serde(default)]
    cup_size: Option<u32>,
}

/// Cancel the last round (or the open draft), stepping the tournament back one
/// stage. Undoable like any other mutation.
async fn cancel_round(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| t.cancel_last_round())?;
    backup_after(&store, "round cancelled");
    view(&store)
}

/// Begin drafting the next round (enters the round-draft state).
///
/// For the very first round, registration may not be finalized yet: rather than
/// force the referee through a separate "finalize" step, we finalize (using the
/// cup size from the body, if the cup is enabled) and prepare in the *same*
/// mutation, so the pair is a single undo step.
async fn prepare_round(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    body: Option<Json<FinalizeRequest>>,
) -> Result<Json<TournamentView>, ApiError> {
    let cup_size = body.and_then(|Json(b)| b.cup_size);
    let mut store = instance.write();
    store.mutate(expected, |t| {
        if !t.registration_finalized {
            t.finalize_registration_with(cup_size)?;
        }
        t.prepare_round().map(|_| ())
    })?;
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
    // A store tombstoned by a concurrent delete must not re-create its backups
    // directory after `remove` cleared it.
    if store.is_deleted() {
        return;
    }
    if let Some(tournament) = store.current() {
        backup::take(tournament, label);
    }
}

/// Body of `PUT /draft`: the draft's customization.
#[derive(Debug, Deserialize)]
struct DraftUpdate {
    #[serde(default)]
    absent: Vec<TournamentId>,
    /// Forced pairings; only `player1`/`player2` are read (result is ignored).
    #[serde(default)]
    forced_boards: Vec<Board>,
    #[serde(default)]
    forced_byes: Vec<TournamentId>,
}

/// Edit the current draft (absent set, forced pairings, forced byes).
async fn update_draft(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Json(req): Json<DraftUpdate>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| {
        t.update_draft(req.absent, req.forced_boards, req.forced_byes)
            .map(|_| ())
    })?;
    view(&store)
}

/// Confirm the draft: pair the remaining players and start the round.
async fn confirm_round(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
) -> Result<(StatusCode, Json<TournamentView>), ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| t.confirm_round().map(|_| ()))?;
    let label = round_label(&store, "round", "started");
    backup_after(&store, &label);
    Ok((StatusCode::CREATED, view(&store)?))
}

/// The round number from the path (see [`BackupParams`] on why this is a
/// named struct rather than a bare `Path<u32>`).
#[derive(Deserialize)]
struct RoundParams {
    round_number: u32,
}

/// Explain a round's Swiss pairings: per-board rule ledger and round report.
/// Read-only, so no backup is taken.
async fn round_explanation(
    TournamentCtx(instance): TournamentCtx,
    Path(params): Path<RoundParams>,
) -> Result<Json<RoundExplanation>, ApiError> {
    let store = instance.read();
    let tournament = store.current().ok_or(ApiError::NoTournament)?;
    let explanation = tournament.explain_round(params.round_number)?;
    Ok(Json(explanation))
}

/// Body of the counterfactual endpoint: the two players to probe and the mode —
/// `"force"` ("why aren't these two paired?") or `"forbid"` ("why did you pair
/// them?"). Defaults to force.
#[derive(Debug, Deserialize)]
struct CounterfactualRequest {
    #[serde(default = "default_counterfactual_mode")]
    mode: CounterfactualMode,
    a: TournamentId,
    b: TournamentId,
}

fn default_counterfactual_mode() -> CounterfactualMode {
    CounterfactualMode::Force
}

/// Explain what forcing or forbidding the pairing `a`–`b` in this round would
/// cost. Read-only.
async fn round_counterfactual(
    TournamentCtx(instance): TournamentCtx,
    Path(params): Path<RoundParams>,
    Json(req): Json<CounterfactualRequest>,
) -> Result<Json<Counterfactual>, ApiError> {
    if req.a == req.b {
        return Err(ApiError::BadRequest(
            "a counterfactual needs two different players".into(),
        ));
    }
    let store = instance.read();
    let tournament = store.current().ok_or(ApiError::NoTournament)?;
    let result = tournament.explain_counterfactual(params.round_number, req.a, req.b, req.mode)?;
    Ok(Json(result))
}

/// Body of the force-pairing endpoint: the two players to pair.
#[derive(Debug, Deserialize)]
struct ForcePairingRequest {
    a: TournamentId,
    b: TournamentId,
}

/// Force the pairing `a`–`b` onto the current round (re-pairs it with that board
/// fixed). Mutates, so a backup is taken.
async fn force_pairing(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Json(req): Json<ForcePairingRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    if req.a == req.b {
        return Err(ApiError::BadRequest(
            "a forced pairing needs two different players".into(),
        ));
    }
    let mut store = instance.write();
    store.mutate(expected, |t| t.force_pairing(req.a, req.b).map(|_| ()))?;
    let label = round_label(&store, "round", "re-paired");
    backup_after(&store, &label);
    view(&store)
}

/// The round number and board index from the path (see [`BackupParams`] on
/// why this is a named struct).
#[derive(Deserialize)]
struct BoardParams {
    round_number: u32,
    board_index: usize,
}

/// Body of the board-result endpoint: which player the referee clicked.
#[derive(Debug, Deserialize)]
struct SetResultRequest {
    clicked: Winner,
}

/// Toggle a board's winner in response to a clicked player.
///
/// Recording a board result can complete the round (once every board has one).
/// A round no longer has an explicit "complete" step, so this is where the
/// automatic "round N completed" backup is taken — but only on the transition
/// into completed, so re-editing an already-complete round doesn't spam backups.
async fn set_board_result(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<BoardParams>,
    Json(req): Json<SetResultRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let was_completed = round_completed(&store, params.round_number);
    store.mutate(expected, |t| {
        t.toggle_board_winner(params.round_number, params.board_index, req.clicked)
            .map(|_| ())
    })?;
    if !was_completed && round_completed(&store, params.round_number) {
        backup_after(&store, &format!("round {} completed", params.round_number));
    }
    view(&store)
}

/// Whether the numbered round exists and is currently completed.
fn round_completed(store: &TournamentStore, round_number: u32) -> bool {
    store
        .current()
        .and_then(|t| t.rounds.iter().find(|r| r.number == round_number))
        .is_some_and(|r| r.completed)
}

/// Body of the draw-flag endpoint: whether the game was drawn before resolving.
#[derive(Debug, Deserialize)]
struct SetDrawnRequest {
    drawn: bool,
}

/// Set (or clear) a board's "a draw occurred" flag.
async fn set_board_drawn(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<BoardParams>,
    Json(req): Json<SetDrawnRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| {
        t.set_board_drawn(params.round_number, params.board_index, req.drawn)
            .map(|_| ())
    })?;
    view(&store)
}

/// Body of the no-show endpoint: which side(s) failed to appear (one player or
/// `both`), or `null` to clear the flag back to a normal unplayed board.
#[derive(Debug, Deserialize)]
struct SetNoShowRequest {
    absent: Option<NoShow>,
}

/// Mark (or clear) a board as a no-show.
///
/// Like recording a winner, a no-show can complete the round (once every board
/// is decided), so the automatic "round N completed" backup is taken here too,
/// only on the transition into completed.
async fn set_board_no_show(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<BoardParams>,
    Json(req): Json<SetNoShowRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let was_completed = round_completed(&store, params.round_number);
    store.mutate(expected, |t| {
        t.set_board_no_show(params.round_number, params.board_index, req.absent)
            .map(|_| ())
    })?;
    if !was_completed && round_completed(&store, params.round_number) {
        backup_after(&store, &format!("round {} completed", params.round_number));
    }
    view(&store)
}

/// The round and player addressed by the sit-out endpoint (see [`BackupParams`]
/// on why this is a named struct rather than a bare tuple `Path`).
#[derive(Deserialize)]
struct SitoutParams {
    round_number: u32,
    player: TournamentId,
}

/// Body of the sit-out endpoint: what the round is worth to that player.
#[derive(Debug, Deserialize)]
struct SetSitoutValueRequest {
    value: SitoutValue,
}

/// Set what a round scored a player who sat it out (`0+` / `0=` / `0−`).
///
/// Allowed on completed rounds — re-scoring a past round is the point — and it
/// never changes whether the round is complete, since sit-outs don't gate that.
async fn set_sitout_value(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<SitoutParams>,
    Json(req): Json<SetSitoutValueRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| {
        t.set_sitout_value(params.round_number, params.player, req.value)
            .map(|_| ())
    })?;
    view(&store)
}

/// Body of the long-game endpoint: whether the board is a two-round long game.
#[derive(Debug, Deserialize)]
struct SetLongRequest {
    long: bool,
}

/// Flag (or unflag) a board as a two-round "long game" (see
/// `docs/two-round-boards.md`).
///
/// Like recording a winner, flagging the last-undecided board long can complete
/// the round, so the automatic "round N completed" backup is taken here too, only
/// on the transition into completed.
async fn set_board_long(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<BoardParams>,
    Json(req): Json<SetLongRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let was_completed = round_completed(&store, params.round_number);
    store.mutate(expected, |t| {
        t.set_board_long(params.round_number, params.board_index, req.long)
            .map(|_| ())
    })?;
    if !was_completed && round_completed(&store, params.round_number) {
        backup_after(&store, &format!("round {} completed", params.round_number));
    }
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
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<BoardParams>,
    Json(req): Json<SetHandicapRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| {
        t.set_board_handicap(params.round_number, params.board_index, req.handicap)
            .map(|_| ())
    })?;
    view(&store)
}

/// Register a player in the tournament.
async fn add_player(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Json(new_player): Json<NewPlayer>,
) -> Result<(StatusCode, Json<TournamentView>), ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| t.add_player(new_player).map(|_| ()))?;
    Ok((StatusCode::CREATED, view(&store)?))
}

/// Register many players in one request (CSV import). All-or-nothing: the
/// whole batch is one `mutate` call, so it lands as a single history entry —
/// one undo reverts the entire import rather than one player at a time.
async fn add_players_batch(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Json(new_players): Json<Vec<NewPlayer>>,
) -> Result<(StatusCode, Json<TournamentView>), ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| {
        for new_player in new_players {
            t.add_player(new_player)?;
        }
        Ok(())
    })?;
    Ok((StatusCode::CREATED, view(&store)?))
}

/// Register players from a raw CSV file (text body), filling missing
/// ELO/grade/nationality from the server's cached FESA list.
///
/// The CSV is parsed by `osp-core` ([`osp_core::parse_players_csv`]) so the
/// column/format rules have a single tested implementation shared by every
/// client. Like [`add_players_batch`] the whole roster lands as one
/// all-or-nothing mutation (one undo reverts the entire import). Returns 400 on
/// a malformed file (empty, missing name columns, or any row with no last name);
/// enrichment is best-effort against whatever FESA list is cached.
async fn import_players_csv(
    State(state): State<AppState>,
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    body: String,
) -> Result<(StatusCode, Json<TournamentView>), ApiError> {
    let ratings = ratings::cached_ratings(&state);
    // A parse failure carries a machine code so the client can localize it.
    let new_players = osp_core::parse_players_csv(&body, &ratings).map_err(ApiError::CsvImport)?;
    let mut store = instance.write();
    store.mutate(expected, |t| {
        for new_player in new_players {
            t.add_player(new_player)?;
        }
        Ok(())
    })?;
    Ok((StatusCode::CREATED, view(&store)?))
}

/// The player's own id from the path (see [`BackupParams`] on why this is a
/// named struct).
#[derive(Deserialize)]
struct PlayerParams {
    player_id: Uuid,
}

/// Edit an existing player's fields (in-place cell editing).
async fn edit_player(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<PlayerParams>,
    Json(new_player): Json<NewPlayer>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| {
        t.edit_player(params.player_id, new_player).map(|_| ())
    })?;
    view(&store)
}

/// Remove a player from the tournament by id.
async fn remove_player(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<PlayerParams>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| t.remove_player(params.player_id))?;
    view(&store)
}

/// Body of the eligibility endpoint: the new cup-eligibility flag.
#[derive(Debug, Deserialize)]
struct SetEligibleRequest {
    eligible: bool,
}

/// Set whether a player is eligible for the direct-elimination cup.
async fn set_player_eligible(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<PlayerParams>,
    Json(req): Json<SetEligibleRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| {
        t.set_player_eligible(params.player_id, req.eligible)
            .map(|_| ())
    })?;
    view(&store)
}

/// Body of the category-membership endpoint: which category, and whether the
/// player belongs to it.
#[derive(Debug, Deserialize)]
struct SetCategoryRequest {
    category_id: Uuid,
    member: bool,
}

/// Add or remove a player's membership in a referee-defined category.
async fn set_player_category(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<PlayerParams>,
    Json(req): Json<SetCategoryRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| {
        t.set_player_category(params.player_id, req.category_id, req.member)
            .map(|_| ())
    })?;
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
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<PlayerParams>,
    Json(req): Json<AddAdjustmentRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| {
        t.add_point_adjustment(params.player_id, req.delta, req.reason)
            .map(|_| ())
    })?;
    view(&store)
}

/// The player id and adjustment id from the path (see [`BackupParams`] on why
/// this is a named struct).
#[derive(Deserialize)]
struct AdjustmentParams {
    player_id: Uuid,
    adjustment_id: Uuid,
}

/// Remove a previously applied point adjustment.
async fn remove_point_adjustment(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<AdjustmentParams>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, |t| {
        t.remove_point_adjustment(params.player_id, params.adjustment_id)
            .map(|_| ())
    })?;
    view(&store)
}
