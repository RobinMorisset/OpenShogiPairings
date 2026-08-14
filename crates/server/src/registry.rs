//! The tournament registry's own top-level routes: list, create, and import.
//!
//! Unlike everything under `/api/tournaments/{id}/...` (see [`crate::tournament`]),
//! these aren't scoped to one tournament — `GET` has to work before you've
//! picked one, and the two `POST`s are what mint new ones.

use axum::body::Bytes;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use uuid::Uuid;

use crate::backup::DeletedTournament;
use crate::error::ApiError;
use crate::state::{AppState, RestoreError, TournamentSummary};

/// `GET /api/tournaments`: list tournaments. Public — needed to render the
/// picker before anyone has logged in anywhere — but see [`list_tournaments`]
/// for what "every" means to a caller with no admin token.
pub(crate) fn public_routes() -> Router<AppState> {
    Router::new().route("/api/tournaments", get(list_tournaments))
}

/// `POST /api/tournaments` (create a new, empty tournament),
/// `POST /api/tournaments/import` (create one from a save file), and the two
/// deleted-tournament routes (list the bin, restore out of it — which mints a
/// tournament like the other two, and whose listing names tournaments somebody
/// deliberately deleted). Callers are expected to wrap these with
/// [`crate::auth::require_admin_auth`] (see `lib.rs`) — minting a tournament, by
/// any of these routes, is gated by the admin password if one is configured.
///
/// `deleted` is a literal path segment where `/api/tournaments/{id}` takes an
/// id; axum matches the static segment first, so no tournament can shadow these.
pub(crate) fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/api/tournaments", post(create_tournament))
        .route("/api/tournaments/import", post(import_tournament))
        .route("/api/tournaments/deleted", get(list_deleted))
        .route(
            "/api/tournaments/deleted/{id}/restore",
            post(restore_deleted),
        )
}

/// What `GET /api/tournaments` answers.
///
/// `restricted` is why the envelope exists: without it, a referee arriving at a
/// hosted server would see a short list and no hint that there is more, which
/// is exactly the kind of silent wrong answer that costs trust. It says "an
/// admin password is configured and you did not present it, so this list shows
/// only the published tournaments", and the picker turns it into an
/// admin-password prompt.
#[derive(Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
struct TournamentListing {
    tournaments: Vec<TournamentSummary>,
    restricted: bool,
}

/// List tournaments for the picker.
///
/// The picker is the one thing a stranger who finds the URL can enumerate, and
/// once there is a public reader UI it would become that UI's front door for
/// tournaments that never opted into being public. So on a server that has an
/// admin password — the marker of "this host is reachable by people who are not
/// its referees" — a caller without a valid admin token sees only the published
/// tournaments. A server deliberately run open (no admin password, which is
/// every local and embedded one) is unchanged: it lists everything, as before.
async fn list_tournaments(State(state): State<AppState>, req: Request) -> Json<TournamentListing> {
    let restricted = match &state.admin_auth {
        None => false,
        Some(auth) => !crate::auth::has_valid_bearer(auth, &req),
    };
    Json(TournamentListing {
        tournaments: state.registry.list(restricted),
        restricted,
    })
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
/// bytes survive the wrapper's deserialize untouched and can be handed to
/// [`crate::save::load`] whole — which needs them to read the format version
/// before anything parses the tournament, and to upgrade an older save in place
/// if it is one this build still reads.
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
    // The only gate between an untrusted file and the registry.
    let tournament = crate::save::load(req.tournament.get().as_bytes())?;
    let (id, token) = state.registry.insert(tournament, req.password);
    Ok((
        StatusCode::CREATED,
        Json(CreateTournamentResponse { id, token }),
    ))
}

/// `GET /api/tournaments/deleted`: the tournaments in the bin, most recently
/// deleted first — what the picker's "recently deleted" section lists. See
/// [`crate::state::TournamentRegistry::remove`] for how they get there.
async fn list_deleted(State(state): State<AppState>) -> Json<Vec<DeletedTournament>> {
    Json(state.registry.list_deleted())
}

/// Body of `POST /api/tournaments/deleted/{id}/restore`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreRequest {
    /// Which backup to restore; absent takes the newest, the one taken as the
    /// tournament was deleted.
    #[serde(default)]
    backup_id: Option<String>,
    /// The password the tournament had, required if it had one — and given to
    /// the restored tournament, so it comes back as protected as it was.
    #[serde(default)]
    password: Option<String>,
}

/// `POST /api/tournaments/deleted/{id}/restore`: bring a deleted tournament back
/// as a new one.
///
/// The backups are left where they are, so this can be repeated (with a
/// different `backup_id`) if the first attempt restored the wrong moment.
async fn restore_deleted(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RestoreRequest>,
) -> Result<(StatusCode, Json<CreateTournamentResponse>), ApiError> {
    let (id, token) = state
        .registry
        .restore_deleted(id, req.backup_id.as_deref(), req.password.as_deref())
        .map_err(|e| match e {
            RestoreError::NotFound => {
                ApiError::NotFound(format!("no deleted tournament {id} to restore"))
            }
            RestoreError::AlreadyLive => ApiError::Conflict(format!(
                "tournament {id} is not deleted — it has already been restored"
            )),
            RestoreError::NoBackups => {
                ApiError::NotFound(format!("deleted tournament {id} has no backup left"))
            }
            RestoreError::NoSuchBackup => ApiError::NotFound("no such backup".into()),
            // 403, not 401: the caller's admin token (if any) was fine, so the
            // client must ask for *this tournament's* password rather than
            // dropping its session and showing the admin sign-in.
            RestoreError::WrongPassword => {
                ApiError::Forbidden("wrong password for that tournament".into())
            }
            RestoreError::PasswordNotNeeded => {
                ApiError::BadRequest("that tournament had no password".into())
            }
            RestoreError::Invalid(e) => ApiError::Domain(e),
        })?;
    Ok((
        StatusCode::CREATED,
        Json(CreateTournamentResponse { id, token }),
    ))
}
