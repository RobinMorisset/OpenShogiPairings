//! Per-tournament API: fetch, replace (load), delete, player CRUD, rounds, and
//! undo — everything nested under `/api/tournaments/{id}`.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use osp_core::{
    Board, Counterfactual, CounterfactualMode, CupBracketView, CupPodium, ForcedMatch, Forfeit,
    Handicap, LicenceCheck, NewPlayer, SitoutValue, Standing, TeamId, TeamMatchView, TeamStanding,
    Tournament, TournamentId, TournamentSettings, UnitKey, Winner,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backup;
use crate::error::ApiError;
use crate::live::ExpectedVersion;
use crate::ratings;
use crate::scope::TournamentCtx;
use crate::state::{AppState, TournamentInstance, TournamentStore};
use crate::undo::UndoLabel;
use crate::{auth, live, public, undo};

/// Build the router nested at `/api/tournaments/{id}`.
///
/// Split into **three** groups that share the `{id}` prefix and nothing else:
///
/// - a public group (login, the change-version SSE stream — both need to
///   resolve the tournament but must work without a token already);
/// - the **reader** group ([`crate::public`]): the public projection and its
///   own stream, reached with a capability key instead of a password, and
///   containing no mutating handler at all;
/// - a protected group (everything else), whose `route_layer`s run auth before
///   the version check. Both middlewares need [`Path`]-param access to the
///   `{id}` segment, which requires `route_layer` (not `layer`) — see axum's
///   docs on the difference.
///
/// The property worth preserving as this grows: **no handler is reachable from
/// more than one group.** No bug in token handling can then escalate a reader
/// into a writer, because there is no writer to reach (`docs/archive/public-access.md`
/// §3).
///
/// - `GET    /`               fetch the tournament
/// - `DELETE /`               delete the tournament
/// - `POST   /undo`           revert the last player change (409 if there is none)
/// - `GET    /american-grid` export the cross-table for ELO (text)
/// - `PUT    /settings`      update tournament settings (MacMahon, …)
/// - `POST   /cancel-round`           cancel the last round (or draft)
/// - `POST   /rounds/prepare`         begin drafting the next round
/// - `PUT    /draft`                  edit the draft
/// - `POST   /rounds`                 confirm the draft (pair & start)
/// - `POST   /rounds/{n}/counterfactual` explain forcing/forbidding a pairing
/// - `POST   /rounds/force-pairing`      re-pair the round with a forced board
/// - `POST   /rounds/{n}/boards/{i}/result`  toggle a board winner
/// - `POST   /rounds/{n}/boards/{i}/drawn`   set the draw flag
/// - `POST   /rounds/{n}/boards/{i}/no-show` set/clear a board no-show
/// - `PUT    /rounds/{n}/boards/{i}/handicap` set/clear the handicap
/// - `POST   /rounds/{n}/boards/{i}/long`    flag/unflag a two-round long game
/// - `PUT    /rounds/{n}/sitouts/{player}`   re-score what a sat-out round was worth
/// - `PUT    /rounds/{n}/team-sitouts/{team_id}` the same for a whole team
/// - `POST   /players`       register a player
/// - `POST   /players/import-csv` register a roster from a raw CSV body
/// - `POST   /players/licence-check` who of one nationality is missing from a licence list
/// - `PUT    /players/{player_id}`  edit a player
/// - `DELETE /players/{player_id}`  remove a player
/// - `POST   /players/eligible`  set cup eligibility for a list of players, atomically
/// - `POST   /players/{player_id}/eligible`  set cup eligibility
/// - `POST   /players/{player_id}/category`  set membership in a player category
/// - `POST   /players/{player_id}/adjustments`             add a manual point bonus/malus
/// - `DELETE /players/{player_id}/adjustments/{adjustment_id}` remove one
/// - `PUT    /players/{player_id}/pairing-rating` set/clear the pairing ELO (team mode)
/// - `POST   /teams`                     create a team
/// - `PUT    /teams/{team_id}`           rename a team
/// - `DELETE /teams/{team_id}`           delete a team
/// - `POST   /teams/{team_id}/members`   add a player to a team
/// - `DELETE /teams/{team_id}/members/{player_id}` take a player out of a team
/// - `PUT    /teams/{team_id}/board-order`     reorder a team's boards
/// - `POST   /teams/{team_id}/sort-by-rating`  reset that order by rating
/// - `POST   /teams/{team_id}/adjustments`     add a manual point bonus/malus
/// - `DELETE /teams/{team_id}/adjustments/{adjustment_id}` remove one
/// - `GET    /backups`         where the automatic backups live, and the list
/// - `POST   /backups/{backup_id}/restore` restore a backup as the current tournament
/// - `POST   /login`  exchange this tournament's password for a session token
/// - `GET    /events` SSE stream of this tournament's change version
/// - `GET    /publication` whether this tournament is published, and its key
/// - `PUT    /publication` publish / rotate the key / unpublish
/// - `GET    /public-snapshot` the public projection, for the static HTML export
/// - `GET    /public`        the public projection (capability key, no password)
/// - `GET    /public/events` the same projection, pushed on every change
///
/// Every endpoint (except login/events/the text exports) returns a
/// [`TournamentView`] (the tournament plus what an undo would revert), so
/// clients can refresh their view and the undo button from a single response.
pub(crate) fn scope(state: AppState) -> Router<AppState> {
    let public = Router::new()
        .route("/login", post(auth::tournament_login))
        .route("/events", get(live::events));

    let protected = Router::new()
        .route("/", get(get_tournament).delete(delete_tournament))
        .route(
            "/publication",
            get(public::get_publication).put(public::set_publication),
        )
        .route("/public-snapshot", get(public::get_public_snapshot))
        .route("/undo", post(undo))
        .route("/american-grid", get(american_grid))
        .route("/settings", put(update_settings))
        .route("/cancel-round", post(cancel_round))
        .route("/rounds/prepare", post(prepare_round))
        .route("/draft", put(update_draft))
        .route("/rounds", post(confirm_round))
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
        .route(
            "/rounds/{round_number}/team-sitouts/{team_id}",
            put(set_team_sitout_value),
        )
        .route("/players", post(add_player))
        .route("/players/import-csv", post(import_players_csv))
        .route("/players/licence-check", post(check_licences))
        .route("/players/eligible", post(set_players_eligible))
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
        .route(
            "/players/{player_id}/pairing-rating",
            put(set_pairing_rating),
        )
        .route("/teams", post(add_team))
        .route("/teams/{team_id}", put(rename_team).delete(remove_team))
        .route("/teams/{team_id}/members", post(add_team_member))
        .route(
            "/teams/{team_id}/members/{player_id}",
            axum::routing::delete(remove_team_member),
        )
        .route("/teams/{team_id}/board-order", put(set_team_board_order))
        .route("/teams/{team_id}/sort-by-rating", post(sort_team_by_rating))
        .route("/teams/{team_id}/adjustments", post(add_team_adjustment))
        .route(
            "/teams/{team_id}/adjustments/{adjustment_id}",
            axum::routing::delete(remove_team_adjustment),
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

    public.merge(public::scope()).merge(protected)
}

/// Why an action the UI offers would be refused right now, in the shape the
/// client already renders: the stable machine `code` and its interpolation
/// `values`, exactly as an error response carries them.
///
/// Shipped so a button can be disabled *and explain itself* without the frontend
/// re-deriving the rule. Every rule this project has mirrored into TypeScript has
/// eventually disagreed with the Rust it copied — the long-game predicates most
/// recently — so the decision travels rather than its inputs.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub(crate) struct BlockedReason {
    pub(crate) code: String,
    pub(crate) values: std::collections::BTreeMap<String, String>,
}

impl BlockedReason {
    /// `None` when the action would go ahead. An error with no localized code
    /// still blocks, carrying its English message as the `code`: better a bare
    /// string than a button that lies about being available.
    fn of(blocker: Option<osp_core::TournamentError>) -> Option<Self> {
        let err = blocker?;
        Some(match crate::error::domain_payload(&err) {
            Some((code, values)) => BlockedReason {
                code: code.to_string(),
                values,
            },
            None => BlockedReason {
                code: err.to_string(),
                values: Default::default(),
            },
        })
    }
}

/// API response: the current tournament, what an undo would revert, and the
/// derived standings.
///
/// Deliberately *not* the bare `Tournament` (which is what save/load uses) — the
/// `undo_label` is server session state and `standings` is derived, both kept
/// out of the persisted shape. Standings are computed server-side so every
/// client (and the future American grid) shares one ranking.
#[derive(Serialize, ts_rs::TS)]
#[ts(
    export,
    rename = "TournamentResponse",
    export_to = "../../../frontend/src/lib/generated/"
)]
pub(crate) struct TournamentView {
    pub(crate) tournament: Tournament,
    /// What the undo button would revert, or absent when there is nothing to
    /// undo — which is also how the client knows to disable it (see
    /// [`crate::undo`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) undo_label: Option<UndoLabel>,
    /// `false` when the server could not write this state to disk: the edit was
    /// applied and answered `200`, but it lives only in memory and a restart
    /// would lose it. The client shows a standing banner — on a full disk every
    /// later edit is a `200` too, so nothing else would ever say so. Always
    /// `true` where there is no persistence to begin with (the desktop app).
    pub(crate) persisted: bool,
    /// Why starting the next round would be refused, or absent if it would go
    /// ahead — [`Tournament::next_round_blocker`], rendered by the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) next_round_blocked: Option<BlockedReason>,
    /// The same for the American Grid export
    /// ([`Tournament::grid_export_blocker`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) grid_export_blocked: Option<BlockedReason>,
    /// Monotonic change version. Clients echo it in the `X-Tournament-Version`
    /// header so a stale edit is rejected (409), and use it to ignore the SSE
    /// echo of their own change (see [`crate::live`]).
    pub(crate) version: u32,
    pub(crate) standings: Vec<Standing>,
    /// The ranked team standings, in team mode only — the table the Standings
    /// tab shows there, with the per-player figures staying in `standings` for
    /// the breakdown rows. `None` for an individual tournament.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) team_standings: Option<Vec<TeamStanding>>,
    /// The team matches of each round, scored, indexed like `tournament.rounds`
    /// — the grouping the round view and the standings' round columns render
    /// (see [`osp_core::TeamMatchView`]). Empty outside team mode.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) team_matches: Vec<Vec<TeamMatchView>>,
    /// The cup podium once decided (champion / runner-up / third / fourth), for the
    /// Results-tab medals. `None` when there is no cup or the final isn't finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cup_podium: Option<CupPodium>,
    /// The full cup bracket (structure + results), derived server-side so the Cup
    /// tab renders it directly. `None` when there is no cup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cup_bracket: Option<CupBracketView>,
    /// Players the cup will pair in the round being drafted, so the draft UI can
    /// keep them out of the Swiss customization. Empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) draft_cup_players: Vec<TournamentId>,
    /// Players a long game started in the previous round keeps out of the round
    /// being drafted, for the same reason as `draft_cup_players` — and shipped
    /// rather than mirrored because the frontend's own copy of the rule carried
    /// the same bug the Rust one did. Empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) draft_long_players: Vec<TournamentId>,
    /// Suggested handicap per board, indexed like `tournament.rounds[i].boards[j]`.
    /// Computed from current ratings regardless of `handicap_policy` — the
    /// frontend decides how to surface it. `None` = no suggestion (near-equal
    /// strength, an unrated player, or a cup board).
    pub(crate) suggested_handicaps: Vec<Vec<Option<Handicap>>>,
    /// The winner that counts for standings/pairing per board (see
    /// [`osp_core::Board::effective_winner`]), indexed like
    /// `tournament.rounds[i].boards[j]`. Computed here — using the tournament's
    /// `handicap_wiel_rule` setting — so the frontend never has to re-derive it.
    /// `None` while a board is undecided.
    pub(crate) effective_winners: Vec<Vec<Option<Winner>>>,
}

/// What a CSV roster import answers: the usual tournament view, plus the entries
/// the file named that were already registered and so were not added again.
///
/// The view is flattened in, so this stays a [`TournamentView`] as far as every
/// client-side state update is concerned and only the extra field has to be read
/// specially. `skipped_duplicates` is empty on the ordinary import — the referee
/// is only told about rows that were left out.
#[derive(Serialize, ts_rs::TS)]
#[ts(
    export,
    rename = "ImportCsvResponse",
    export_to = "../../../frontend/src/lib/generated/"
)]
pub(crate) struct ImportCsvView {
    #[serde(flatten)]
    #[ts(flatten)]
    pub(crate) view: TournamentView,
    /// Names skipped because a player with that name was already registered, in
    /// file order and spelled as the file spelled them.
    pub(crate) skipped_duplicates: Vec<String>,
}

/// Build the view from the store, or 404 if no tournament exists.
///
/// Shared with [`crate::public`], which projects it down to what a reader may
/// see — so the public standings are the referee's own numbers, never a second
/// computation that could disagree with them.
pub(crate) fn build_view(store: &TournamentStore) -> Result<TournamentView, ApiError> {
    let tournament = store.current().cloned().ok_or(ApiError::NoTournament)?;
    let standings = tournament.standings();
    let team_standings = tournament.team_standings();
    let team_matches = tournament.team_matches();
    let cup_podium = tournament.cup_podium();
    let cup_bracket = tournament.cup_bracket();
    let draft_cup_players = tournament.draft_cup_players();
    let draft_long_players = tournament.draft_long_players();
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
    // Read before `tournament` is moved into the response.
    let next_round_blocked = BlockedReason::of(tournament.next_round_blocker());
    let grid_export_blocked = BlockedReason::of(tournament.grid_export_blocker());
    Ok(TournamentView {
        tournament,
        undo_label: store.undo_label().cloned(),
        persisted: store.persisted(),
        next_round_blocked,
        grid_export_blocked,
        version: store.version(),
        standings,
        team_standings,
        team_matches,
        cup_podium,
        cup_bracket,
        draft_cup_players,
        draft_long_players,
        suggested_handicaps,
        effective_winners,
    })
}

/// [`build_view`], wrapped as the JSON body every referee endpoint returns.
fn view(store: &TournamentStore) -> Result<Json<TournamentView>, ApiError> {
    build_view(store).map(Json)
}

/// Fetch the tournament.
async fn get_tournament(
    TournamentCtx(instance): TournamentCtx,
) -> Result<Json<TournamentView>, ApiError> {
    let store = instance.read();
    view(&store)
}

/// Delete the tournament: its registry entry and persisted file. Its backups
/// are kept for the retention period (see [`crate::state::TournamentRegistry::remove`]).
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
///
/// `409` when there is nothing left to undo, rather than a `200` reporting a
/// change that never happened — see [`TournamentStore::undo`]. The client's own
/// button is disabled whenever `undo_label` is absent, so this answers a stale
/// or third-party client, which is worth a line in the log.
async fn undo(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    if store.current().is_none() {
        return Err(ApiError::NoTournament);
    }
    store.ensure_current_version(expected)?;
    store.undo().inspect_err(|err| {
        tracing::warn!("refusing an undo: {err}");
    })?;
    view(&store)
}

/// The automatic backups of one tournament: where they are kept, and what is
/// in there. The directory travels with the list because the referee has to be
/// able to find these files outside the app — to copy them off the machine, or
/// to hand one to someone else — and a rotating store they can't locate is one
/// they can't rely on.
#[derive(Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub(crate) struct BackupList {
    /// Absolute path of the directory holding this tournament's backups, as
    /// the server sees it. `None` when no backups directory could be resolved
    /// at all — nothing is being backed up, which the client says outright
    /// rather than showing an empty list that looks merely new.
    directory: Option<String>,
    /// The backups themselves, newest first.
    backups: Vec<backup::BackupInfo>,
}

/// List automatic backups for the tournament, newest first (see [`backup`]).
async fn list_backups(
    TournamentCtx(instance): TournamentCtx,
) -> Result<Json<BackupList>, ApiError> {
    let dir = instance.backups_dir.as_deref();
    Ok(Json(BackupList {
        directory: dir.map(|d| d.display().to_string()),
        backups: backup::list(dir),
    }))
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
    // The backup replaces a tournament, it doesn't conjure one: a store with
    // nothing in it (a concurrent delete) must 404 rather than be revived.
    store.current().ok_or(ApiError::NoTournament)?;
    let restored =
        backup::load(instance.backups_dir.as_deref(), &params.backup_id).map_err(|e| match e {
            backup::LoadError::NotFound => {
                ApiError::NotFound(format!("no backup {}", params.backup_id))
            }
            // A backup from before a format bump: it is right there in the list,
            // so say what is wrong with it rather than claim it isn't there.
            backup::LoadError::Unreadable(e) => ApiError::Domain(e),
        })?;
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
    // Refused outright while a long game is unresolved — an unfinished two-round
    // board has no result to put in its column, and rendering it anyway writes a
    // loss for both players into a document destined for a rating body.
    let grid = osp_core::american_grid(tournament, &tournament.standings())?;
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], grid))
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
    store.mutate(expected, UndoLabel::settings(), move |t| {
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
    store.mutate(expected, UndoLabel::cancel_round(), |t| {
        t.cancel_last_round()
    })?;
    backup_after(&instance, &store, "round cancelled");
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
    let undo_label = UndoLabel::prepare_round(next_round_number(&store));
    store.mutate(expected, undo_label, |t| {
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
    backup_after(&instance, &store, &label);
    view(&store)
}

/// The number the round being drafted carries, or would: the open draft's own,
/// else one past the rounds already played. Read *before* a mutation, to name
/// what that mutation is about to do.
fn next_round_number(store: &TournamentStore) -> u32 {
    match store.current() {
        Some(t) => t
            .draft
            .as_ref()
            .map_or(t.rounds.len() as u32 + 1, |draft| draft.number),
        None => 1,
    }
}

/// The number of the round in progress. `0` when there is none, which only a
/// mutation about to be refused for that very reason can reach — and a refused
/// mutation discards its label unread.
fn current_round_number(store: &TournamentStore) -> u32 {
    store
        .current()
        .and_then(|t| t.rounds.last())
        .map_or(0, |round| round.number)
}

/// A round-numbered backup label, e.g. "round 3 completed" — falls back to
/// `noun` alone if there is (unexpectedly) no last round to number.
fn round_label(store: &TournamentStore, noun: &str, verb: &str) -> String {
    match store.current().and_then(|t| t.rounds.last()) {
        Some(round) => format!("{noun} {} {verb}", round.number),
        None => format!("{noun} {verb}"),
    }
}

/// Take a backup of the current tournament, if any, into the instance's own
/// backups directory. Best-effort and silent on failure (logged inside
/// [`backup::take`]) — a backup problem must never surface as an API error.
///
/// Takes the instance *and* the store guard the caller already holds, rather
/// than re-locking: every call site is mid-mutation with the write guard in
/// hand, and reaching back through the instance for the store here would
/// deadlock.
fn backup_after(instance: &TournamentInstance, store: &TournamentStore, label: &str) {
    // A store tombstoned by a concurrent delete must not re-create its backups
    // directory after `remove` cleared it.
    if store.is_deleted() {
        return;
    }
    if let Some(tournament) = store.current() {
        backup::take(instance.backups_dir.as_deref(), tournament, label);
    }
}

/// Body of `PUT /draft`: the draft's customization.
///
/// The absent set is per player in either mode — in a team tournament a member
/// can be absent without their team being — while the *forced* halves come in a
/// pair each: boards and byes name players, matches and team byes name teams.
/// Which pair applies follows the tournament's mode, and sending the other
/// mode's is a domain error rather than a silently ignored field.
#[derive(Debug, Deserialize)]
struct DraftUpdate {
    #[serde(default)]
    absent: Vec<TournamentId>,
    /// Forced pairings; only `player1`/`player2` are read (result is ignored).
    #[serde(default)]
    forced_boards: Vec<Board>,
    #[serde(default)]
    forced_byes: Vec<TournamentId>,
    /// Forced team matches (team mode).
    #[serde(default)]
    forced_matches: Vec<ForcedMatch>,
    /// Teams forced onto a bye (team mode).
    #[serde(default)]
    forced_team_byes: Vec<TeamId>,
}

/// Edit the current draft (absent set, forced pairings, forced byes).
async fn update_draft(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Json(req): Json<DraftUpdate>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::edit_draft(next_round_number(&store));
    store.mutate(expected, undo_label, |t| {
        if t.settings.team_mode() {
            // The core rejects the player-level halves in team mode, so hand it
            // those too rather than dropping them here: a client that sent them
            // should be told, not quietly obeyed in part.
            if !req.forced_boards.is_empty() || !req.forced_byes.is_empty() {
                return t
                    .update_draft(
                        req.absent.clone(),
                        req.forced_boards.clone(),
                        req.forced_byes.clone(),
                    )
                    .map(|_| ());
            }
            t.update_team_draft(
                req.absent.clone(),
                req.forced_matches.clone(),
                req.forced_team_byes.clone(),
            )
            .map(|_| ())
        } else {
            t.update_draft(
                req.absent.clone(),
                req.forced_boards.clone(),
                req.forced_byes.clone(),
            )
            .map(|_| ())
        }
    })?;
    view(&store)
}

/// Confirm the draft: pair the remaining players and start the round.
async fn confirm_round(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
) -> Result<(StatusCode, Json<TournamentView>), ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::start_round(next_round_number(&store));
    store.mutate(expected, undo_label, |t| t.confirm_round().map(|_| ()))?;
    let label = round_label(&store, "round", "started");
    backup_after(&instance, &store, &label);
    Ok((StatusCode::CREATED, view(&store)?))
}

/// The round number from the path (see [`BackupParams`] on why this is a
/// named struct rather than a bare `Path<u32>`).
#[derive(Deserialize)]
struct RoundParams {
    round_number: u32,
}

/// Body of the counterfactual endpoint: the two players to probe and the mode —
/// `"force"` ("why aren't these two paired?") or `"forbid"` ("why did you pair
/// them?"). Defaults to force.
#[derive(Debug, Deserialize)]
struct CounterfactualRequest {
    #[serde(default = "default_counterfactual_mode")]
    mode: CounterfactualMode,
    /// The two units to probe: player numbers in an individual tournament, team
    /// numbers in a team one — whichever the engine paired. `0` means the bye.
    a: UnitKey,
    b: UnitKey,
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
    /// As for the probe: player numbers, or team numbers in team mode.
    a: UnitKey,
    b: UnitKey,
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
    let undo_label = UndoLabel::force_pairing(current_round_number(&store));
    store.mutate(expected, undo_label, |t| {
        t.force_pairing(req.a, req.b).map(|_| ())
    })?;
    let label = round_label(&store, "round", "re-paired");
    backup_after(&instance, &store, &label);
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
    let undo_label = UndoLabel::board_result(params.round_number, params.board_index);
    store.mutate(expected, undo_label, |t| {
        t.toggle_board_winner(params.round_number, params.board_index, req.clicked)
            .map(|_| ())
    })?;
    if !was_completed && round_completed(&store, params.round_number) {
        backup_after(
            &instance,
            &store,
            &format!("round {} completed", params.round_number),
        );
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
    let undo_label = UndoLabel::board_result(params.round_number, params.board_index);
    store.mutate(expected, undo_label, |t| {
        t.set_board_drawn(params.round_number, params.board_index, req.drawn)
            .map(|_| ())
    })?;
    view(&store)
}

/// Body of the no-show endpoint: which side(s) failed to appear (one player or
/// `both`), or `null` to clear the flag back to a normal unplayed board.
#[derive(Debug, Deserialize)]
struct SetNoShowRequest {
    absent: Option<Forfeit>,
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
    let undo_label = UndoLabel::board_result(params.round_number, params.board_index);
    store.mutate(expected, undo_label, |t| {
        t.set_board_no_show(params.round_number, params.board_index, req.absent)
            .map(|_| ())
    })?;
    if !was_completed && round_completed(&store, params.round_number) {
        backup_after(
            &instance,
            &store,
            &format!("round {} completed", params.round_number),
        );
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
    let undo_label = UndoLabel::sitout_value(params.round_number);
    store.mutate(expected, undo_label, |t| {
        t.set_sitout_value(params.round_number, params.player, req.value)
            .map(|_| ())
    })?;
    view(&store)
}

/// The round and team addressed by the team sit-out endpoint.
#[derive(Deserialize)]
struct TeamSitoutParams {
    round_number: u32,
    team_id: Uuid,
}

/// Set what a round scored a **team** that sat it out, writing the value to
/// every member's entry at once.
///
/// A team sits out together and its score for the round is read from those
/// entries, which have to agree — so this is the team-mode way to re-score such
/// a round; the per-player endpoint above would leave them disagreeing.
async fn set_team_sitout_value(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<TeamSitoutParams>,
    Json(req): Json<SetSitoutValueRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::sitout_value(params.round_number);
    store.mutate(expected, undo_label, |t| {
        t.set_team_sitout_value(params.round_number, params.team_id, req.value)
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
/// `docs/reference/two-round-boards.md`).
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
    let undo_label = UndoLabel::board_result(params.round_number, params.board_index);
    store.mutate(expected, undo_label, |t| {
        t.set_board_long(params.round_number, params.board_index, req.long)
            .map(|_| ())
    })?;
    if !was_completed && round_completed(&store, params.round_number) {
        backup_after(
            &instance,
            &store,
            &format!("round {} completed", params.round_number),
        );
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
    let undo_label = UndoLabel::board_handicap(params.round_number, params.board_index);
    store.mutate(expected, undo_label, |t| {
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
    let undo_label = UndoLabel::register_player(&new_player);
    store.mutate(expected, undo_label, |t| {
        t.add_player(new_player).map(|_| ())
    })?;
    Ok((StatusCode::CREATED, view(&store)?))
}

/// Register players from a raw CSV file (text body), filling missing
/// ELO/grade/nationality from the server's cached FESA list.
///
/// The CSV is parsed by `osp-core` ([`osp_core::parse_players_csv`]) so the
/// column/format rules have a single tested implementation shared by every
/// client. The whole roster lands as one all-or-nothing mutation, so a single
/// undo reverts the entire import rather than one player at a time. Returns 400 on
/// a malformed file (empty, missing name columns, or any row with no last name);
/// enrichment is best-effort against whatever FESA list is cached.
///
/// A file naming players who are already registered — the same file loaded
/// twice, or a corrected export of it — registers only the ones missing; the
/// rest come back in `skipped_duplicates` so the referee is told rather than
/// left to spot the homonyms (see
/// [`osp_core::Tournament::add_players_skipping_duplicates`]).
async fn import_players_csv(
    State(state): State<AppState>,
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    body: String,
) -> Result<(StatusCode, Json<ImportCsvView>), ApiError> {
    let ratings = ratings::cached_ratings(&state);
    // A parse failure carries a machine code so the client can localize it.
    let new_players = osp_core::parse_players_csv(&body, &ratings).map_err(ApiError::CsvImport)?;
    let mut store = instance.write();
    let mut skipped_duplicates = Vec::new();
    store.mutate(expected, UndoLabel::import_players(), |t| {
        skipped_duplicates = t.add_players_skipping_duplicates(new_players)?;
        Ok(())
    })?;
    Ok((
        StatusCode::CREATED,
        Json(ImportCsvView {
            view: build_view(&store)?,
            skipped_duplicates,
        }),
    ))
}

/// Body of the licence-check endpoint: which nationality to check, and the
/// federation's list to check it against.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LicenceCheckRequest {
    /// The nationality to check, as registered (e.g. `FR`); matched case- and
    /// accent-insensitively. Required and non-blank — "the players with no
    /// nationality" is not something this endpoint answers.
    nationality: String,
    /// The raw text of the licence-list CSV.
    csv: String,
}

/// Report which registered players of one nationality are **not** on a
/// federation's licence list — the ones who have not paid their yearly fee.
///
/// Read-only: nothing is registered, edited or removed, so no version guard and
/// no backup. The list is parsed by `osp-core` ([`osp_core::check_licences`]) with
/// the same rules as a roster import, and 400 says the file was unusable (empty,
/// no name columns, a row with no last name) rather than that everyone is
/// licensed. Each missing player carries any list entry one edit from their name
/// (`near_misses`), for the misspellings these exports are full of.
async fn check_licences(
    TournamentCtx(instance): TournamentCtx,
    Json(req): Json<LicenceCheckRequest>,
) -> Result<Json<LicenceCheck>, ApiError> {
    let nationality = req.nationality.trim();
    if nationality.is_empty() {
        return Err(ApiError::BadRequest(
            "a licence check needs a nationality to check".into(),
        ));
    }
    let store = instance.read();
    let tournament = store.current().ok_or(ApiError::NoTournament)?;
    // A parse failure carries a machine code so the client can localize it.
    let check = osp_core::check_licences(&req.csv, &tournament.players, nationality)
        .map_err(ApiError::CsvImport)?;
    Ok(Json(check))
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
    let undo_label = UndoLabel::edit_player(store.current(), params.player_id);
    store.mutate(expected, undo_label, |t| {
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
    let undo_label = UndoLabel::remove_player(store.current(), params.player_id);
    store.mutate(expected, undo_label, |t| t.remove_player(params.player_id))?;
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
    let undo_label = UndoLabel::edit_player(store.current(), params.player_id);
    store.mutate(expected, undo_label, |t| {
        t.set_player_eligible(params.player_id, req.eligible)
            .map(|_| ())
    })?;
    view(&store)
}

/// Body of the bulk eligibility endpoint: which players, and the flag to give
/// all of them.
#[derive(Debug, Deserialize)]
struct SetManyEligibleRequest {
    player_ids: Vec<Uuid>,
    eligible: bool,
}

/// Set cup eligibility for a list of players in one mutation.
///
/// The client used to do this by looping over the single-player endpoint, which
/// meant a request that failed halfway — an id that is no longer registered,
/// another referee's edit landing in between — left the roster half-changed,
/// with no version at which the referee's instruction was carried out.
///
/// The loop lives inside the `mutate` closure instead, which runs against a
/// clone and only swaps it in if the whole closure returns `Ok`. So this is
/// all-or-nothing, and it costs one version bump and one undo step for the whole
/// bulk rather than one per player.
///
/// Which players is the caller's business: the UI selects them by nationality,
/// but the rule for that ("no nationality" is a group too) lives in the
/// registration form and has no business being restated here.
async fn set_players_eligible(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Json(req): Json<SetManyEligibleRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    store.mutate(expected, UndoLabel::edit_players(), |t| {
        for id in &req.player_ids {
            t.set_player_eligible(*id, req.eligible)?;
        }
        Ok(())
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
    let undo_label = UndoLabel::edit_player(store.current(), params.player_id);
    store.mutate(expected, undo_label, |t| {
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
    let undo_label = UndoLabel::adjust_points(undo::player_name(store.current(), params.player_id));
    store.mutate(expected, undo_label, |t| {
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
    let undo_label = UndoLabel::adjust_points(undo::player_name(store.current(), params.player_id));
    store.mutate(expected, undo_label, |t| {
        t.remove_point_adjustment(params.player_id, params.adjustment_id)
            .map(|_| ())
    })?;
    view(&store)
}

// --- Teams -----------------------------------------------------------------
//
// Registration-time roster editing for a team tournament. Every handler is a
// one-line delegation to a `Tournament` method, which is where the rules live
// (team mode only, registration still open, names unique, a player in exactly
// one team, …) — the store stays dumb.

/// A team's id from the path (see [`BackupParams`] on why this is a named
/// struct).
#[derive(Deserialize)]
struct TeamParams {
    team_id: Uuid,
}

/// A team and one of its members, for the member-removal path.
#[derive(Deserialize)]
struct TeamMemberParams {
    team_id: Uuid,
    player_id: Uuid,
}

/// Body of team creation and renaming: the team's name.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamNameRequest {
    name: String,
}

/// Body of the add-member endpoint: which registered player joins.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddTeamMemberRequest {
    player_id: Uuid,
}

/// Body of the board-order endpoint: the team's members in their new order.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoardOrderRequest {
    order: Vec<Uuid>,
}

/// Body of the pairing-ELO endpoint: the referee-assigned rating, or `null` to
/// clear it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairingRatingRequest {
    #[serde(default)]
    pairing_rating: Option<u32>,
}

/// Register a new team.
async fn add_team(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Json(req): Json<TeamNameRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::add_team(&req.name);
    store.mutate(expected, undo_label, |t| t.add_team(&req.name).map(|_| ()))?;
    view(&store)
}

/// Rename a team.
async fn rename_team(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<TeamParams>,
    Json(req): Json<TeamNameRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::rename_team(store.current(), params.team_id);
    store.mutate(expected, undo_label, |t| {
        t.rename_team(params.team_id, &req.name).map(|_| ())
    })?;
    view(&store)
}

/// Delete a team; its members return to the unassigned pool.
async fn remove_team(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<TeamParams>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::remove_team(store.current(), params.team_id);
    store.mutate(expected, undo_label, |t| t.remove_team(params.team_id))?;
    view(&store)
}

/// Add a registered player to a team, at the end of its board order.
async fn add_team_member(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<TeamParams>,
    Json(req): Json<AddTeamMemberRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::edit_team(store.current(), params.team_id);
    store.mutate(expected, undo_label, |t| {
        t.add_team_member(params.team_id, req.player_id).map(|_| ())
    })?;
    view(&store)
}

/// Take a player out of a team, back into the unassigned pool.
async fn remove_team_member(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<TeamMemberParams>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::edit_team(store.current(), params.team_id);
    store.mutate(expected, undo_label, |t| {
        t.remove_team_member(params.team_id, params.player_id)
            .map(|_| ())
    })?;
    view(&store)
}

/// Set a team's board order (index 0 = board 1). The body must be a permutation
/// of the team's current members.
async fn set_team_board_order(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<TeamParams>,
    Json(req): Json<BoardOrderRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::edit_team(store.current(), params.team_id);
    store.mutate(expected, undo_label, |t| {
        t.set_team_board_order(params.team_id, req.order.clone())
            .map(|_| ())
    })?;
    view(&store)
}

/// Re-sort a team's board order by descending pairing rating — the default
/// order, and the reset after editing ratings.
async fn sort_team_by_rating(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<TeamParams>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::edit_team(store.current(), params.team_id);
    store.mutate(expected, undo_label, |t| {
        t.sort_team_by_rating(params.team_id).map(|_| ())
    })?;
    view(&store)
}

/// A team and one of its adjustments, for the removal path.
#[derive(Deserialize)]
struct TeamAdjustmentParams {
    team_id: Uuid,
    adjustment_id: Uuid,
}

/// Body of the team-adjustment endpoint: the delta and its mandatory reason.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamAdjustmentRequest {
    delta: i32,
    reason: String,
}

/// Apply a manual point bonus/malus to a team. Available after finalization —
/// that is when a referee needs it.
async fn add_team_adjustment(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<TeamParams>,
    Json(req): Json<TeamAdjustmentRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::adjust_points(undo::team_name(store.current(), params.team_id));
    store.mutate(expected, undo_label, |t| {
        t.add_team_adjustment(params.team_id, req.delta, req.reason.clone())
            .map(|_| ())
    })?;
    view(&store)
}

/// Remove a previously applied team adjustment.
async fn remove_team_adjustment(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<TeamAdjustmentParams>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::adjust_points(undo::team_name(store.current(), params.team_id));
    store.mutate(expected, undo_label, |t| {
        t.remove_team_adjustment(params.team_id, params.adjustment_id)
            .map(|_| ())
    })?;
    view(&store)
}

/// Set (or clear) a player's referee-assigned pairing ELO — team mode with
/// MacMahon starting points only.
async fn set_pairing_rating(
    TournamentCtx(instance): TournamentCtx,
    ExpectedVersion(expected): ExpectedVersion,
    Path(params): Path<PlayerParams>,
    Json(req): Json<PairingRatingRequest>,
) -> Result<Json<TournamentView>, ApiError> {
    let mut store = instance.write();
    let undo_label = UndoLabel::edit_player(store.current(), params.player_id);
    store.mutate(expected, undo_label, |t| {
        t.set_pairing_rating(params.player_id, req.pairing_rating)
            .map(|_| ())
    })?;
    view(&store)
}
