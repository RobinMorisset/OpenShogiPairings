//! HTTP error mapping.
//!
//! Handlers return `Result<_, ApiError>`; this module turns domain errors and a
//! few HTTP-specific conditions into JSON responses with the right status code.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use osp_core::TournamentError;
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
    /// An upstream dependency (e.g. FESA) failed and no cache is available (502).
    Upstream(String),
}

/// JSON body sent for every error: `{ "error": "..." }`.
#[derive(Serialize)]
struct ErrorBody {
    error: String,
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
            | TournamentError::NoRoundToComplete
            | TournamentError::NoRoundToCancel
            | TournamentError::RoundHasUnplayedGames
            | TournamentError::HandicapNeedsRatingDifference
            | TournamentError::UnsupportedFormatVersion { .. }
            | TournamentError::CupSizeRequired
            | TournamentError::InvalidCupSize { .. }
            | TournamentError::NotEnoughEligiblePlayers { .. }
            | TournamentError::CannotRemoveCupPlayer
            | TournamentError::CupBracketInconsistent
            | TournamentError::EmptyAdjustmentReason
            | TournamentError::ZeroPointAdjustment => ApiError::BadRequest(err.to_string()),
            TournamentError::PlayerNotFound(_)
            | TournamentError::RoundNotFound(_)
            | TournamentError::BoardNotFound { .. }
            | TournamentError::AdjustmentNotFound { .. } => ApiError::NotFound(err.to_string()),
        }
    }
}

impl From<MutateError> for ApiError {
    fn from(err: MutateError) -> Self {
        match err {
            MutateError::NoTournament => ApiError::NoTournament,
            MutateError::Domain(e) => ApiError::from(e),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NoTournament => (
                StatusCode::NOT_FOUND,
                "no tournament exists; create one first".to_string(),
            ),
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, message),
            ApiError::Upstream(message) => (StatusCode::BAD_GATEWAY, message),
        };
        (status, Json(ErrorBody { error: message })).into_response()
    }
}
