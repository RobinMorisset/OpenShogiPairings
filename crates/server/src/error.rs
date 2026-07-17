//! HTTP error mapping.
//!
//! Handlers return `Result<_, ApiError>`; this module turns domain errors and a
//! few HTTP-specific conditions into JSON responses with the right status code.

use std::collections::BTreeMap;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use osp_core::{CsvImportError, TournamentError};
use serde::Serialize;

use crate::state::MutateError;

/// An error that can be returned from an API handler.
#[derive(Debug)]
pub enum ApiError {
    /// A request needs a tournament to exist, but none has been created yet.
    NoTournament,
    /// The request was malformed or violated a domain rule (400).
    BadRequest(String),
    /// A referenced resource (e.g. a player) does not exist (404).
    NotFound(String),
    /// The request lacked a valid session token (401). See [`crate::auth`].
    Unauthorized,
    /// A mutating request was based on a stale tournament version (409). See
    /// [`crate::live`].
    VersionConflict,
    /// An upstream dependency (e.g. FESA) failed and no cache is available (502).
    Upstream(String),
    /// A CSV import failed to parse (400). Carries the domain error so the
    /// response can add a stable machine `code` (and interpolation `values`) the
    /// client localizes — the English `error` string is only a fallback.
    CsvImport(CsvImportError),
}

/// JSON body sent for every error: always an `error` message, plus — for errors
/// the client localizes — a stable machine `code` and any interpolation
/// `values` (e.g. the offending CSV rows). Both are omitted when absent, so the
/// common shape stays `{ "error": "..." }`.
#[derive(Serialize)]
struct ErrorBody {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    values: BTreeMap<String, String>,
}

impl ErrorBody {
    /// A plain message-only body (no localization code).
    fn message(error: String) -> Self {
        ErrorBody {
            error,
            code: None,
            values: BTreeMap::new(),
        }
    }
}

/// The stable machine `code` and interpolation `values` for a CSV import error,
/// so the client can render a localized message (the row numbers are language-
/// neutral, passed through as a joined string).
fn csv_error_payload(err: &CsvImportError) -> (&'static str, BTreeMap<String, String>) {
    match err {
        CsvImportError::Empty => ("csv_empty", BTreeMap::new()),
        CsvImportError::MissingNameColumns => ("csv_missing_name_columns", BTreeMap::new()),
        CsvImportError::RowsMissingLastName { rows } => {
            let joined = rows
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            (
                "csv_rows_missing_last_name",
                BTreeMap::from([("rows".to_string(), joined)]),
            )
        }
    }
}

impl From<TournamentError> for ApiError {
    fn from(err: TournamentError) -> Self {
        match err {
            TournamentError::EmptyTournamentName
            | TournamentError::EmptyPlayerName
            | TournamentError::NotEnoughPresentPlayers { .. }
            | TournamentError::RegistrationAlreadyFinalized
            | TournamentError::RegistrationNotFinalized
            | TournamentError::PreviousRoundNotComplete
            | TournamentError::DraftAlreadyExists
            | TournamentError::NoDraft
            | TournamentError::InvalidDraft(_)
            | TournamentError::NoRoundToCancel
            | TournamentError::NoCurrentRound
            | TournamentError::RoundHasResults
            | TournamentError::NotCurrentRound
            | TournamentError::UnresolvedLongGame { .. }
            | TournamentError::HandicapNeedsRatingDifference
            | TournamentError::HandicapNotAllowedForCup
            | TournamentError::UnsupportedFormatVersion { .. }
            | TournamentError::MalformedSave(_)
            | TournamentError::EloEstimateUnanchored
            | TournamentError::CupSizeRequired
            | TournamentError::InvalidCupSize { .. }
            | TournamentError::NotEnoughEligiblePlayers { .. }
            | TournamentError::CannotRemoveCupPlayer
            | TournamentError::CannotRemovePlayedPlayer
            | TournamentError::CupBracketInconsistent
            | TournamentError::EmptyAdjustmentReason
            | TournamentError::ZeroPointAdjustment => ApiError::BadRequest(err.to_string()),
            TournamentError::PlayerNotFound(_)
            | TournamentError::CategoryNotFound(_)
            | TournamentError::RoundNotFound(_)
            | TournamentError::BoardNotFound { .. }
            // The sit-out addressed by the route doesn't exist: that player
            // played a board that round, or wasn't in it.
            | TournamentError::PlayerNotSittingOut { .. }
            | TournamentError::AdjustmentNotFound { .. } => ApiError::NotFound(err.to_string()),
        }
    }
}

impl From<MutateError> for ApiError {
    fn from(err: MutateError) -> Self {
        match err {
            MutateError::NoTournament => ApiError::NoTournament,
            MutateError::VersionConflict => ApiError::VersionConflict,
            MutateError::Domain(e) => ApiError::from(e),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // CSV import errors carry a localization code + values, not just a message.
        if let ApiError::CsvImport(err) = &self {
            let (code, values) = csv_error_payload(err);
            let body = ErrorBody {
                error: err.to_string(),
                code: Some(code),
                values,
            };
            return (StatusCode::BAD_REQUEST, Json(body)).into_response();
        }
        let (status, message) = match self {
            ApiError::NoTournament => (
                StatusCode::NOT_FOUND,
                "no tournament exists; create one first".to_string(),
            ),
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, message),
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "authentication required".to_string(),
            ),
            ApiError::VersionConflict => (
                StatusCode::CONFLICT,
                "the tournament changed since your last view; reload and retry".to_string(),
            ),
            ApiError::Upstream(message) => (StatusCode::BAD_GATEWAY, message),
            // Handled above (needs the code/values payload, not just a message).
            ApiError::CsvImport(_) => unreachable!("CsvImport is handled before this match"),
        };
        (status, Json(ErrorBody::message(message))).into_response()
    }
}
