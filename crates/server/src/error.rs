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

use crate::error_codes::LOCALIZED_ERROR_CODES;
use crate::state::MutateError;

/// An error that can be returned from an API handler.
#[derive(Debug)]
pub(crate) enum ApiError {
    /// A request needs a tournament to exist, but none has been created yet.
    NoTournament,
    /// The request was malformed (400) in a way no domain error describes —
    /// plumbing, not a rule the referee broke, so it stays English.
    BadRequest(String),
    /// A referenced resource (e.g. a player) does not exist (404).
    NotFound(String),
    /// The request lacked a valid session token (401). See [`crate::auth`].
    Unauthorized,
    /// The caller is authenticated, but lacks a *per-resource* password: today,
    /// restoring a deleted tournament that had one (403). Deliberately not a
    /// 401, which the client answers by clearing its token and showing the
    /// sign-in prompt — the wrong move when the token was fine and it is the
    /// tournament's own password that was wrong.
    Forbidden(String),
    /// A mutating request was based on a stale tournament version (409). See
    /// [`crate::live`].
    VersionConflict,
    /// The request contradicts the current state (409) in a way rereading fixes
    /// — today, restoring a tournament that is not deleted any more. Distinct
    /// from [`VersionConflict`](Self::VersionConflict), which the client answers
    /// by replaying its edit against the new version.
    Conflict(String),
    /// An upstream dependency (e.g. FESA) failed and no cache is available (502).
    Upstream(String),
    /// A resource anyone may open is already at its cap (503) — today, the
    /// public reader streams (see [`crate::public`]). Distinct from a 429: the
    /// caller isn't asking too often, the server is simply full, and retrying
    /// later is the right response.
    Overloaded(String),
    /// A CSV import failed to parse (400). Carries the domain error so the
    /// response can add a stable machine `code` (and interpolation `values`) the
    /// client localizes — the English `error` string is only a fallback.
    CsvImport(CsvImportError),
    /// A domain rule was violated. Carries the domain error rather than its
    /// message, so the response can add the `code` and `values` the client
    /// localizes; [`domain_status`] decides 400 vs 404.
    Domain(TournamentError),
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

/// The HTTP status for a domain error: 404 when the request named something
/// that doesn't exist, 400 when it broke a rule.
fn domain_status(err: &TournamentError) -> StatusCode {
    match err {
        TournamentError::PlayerNotFound(_)
        | TournamentError::CategoryNotFound(_)
        | TournamentError::RoundNotFound(_)
        | TournamentError::BoardNotFound { .. }
        // The sit-out addressed by the route doesn't exist: that player
        // played a board that round, or wasn't in it.
        | TournamentError::PlayerNotSittingOut { .. }
        | TournamentError::TeamNotFound(_)
        | TournamentError::AdjustmentNotFound { .. } => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// The stable machine `code` and interpolation `values` for a domain error, or
/// `None` for errors that stay English.
///
/// The match is deliberately exhaustive rather than ending in a `_` arm: a new
/// [`TournamentError`] variant must not compile until someone decides which half
/// it belongs to.
///
/// `None` is the right answer for a variant the referee can only reach if the
/// client is buggy (a route naming a board that isn't there, a draft that the UI
/// should never have submitted). Translating those would dress up a broken
/// invariant as ordinary UI; leaving them in English marks them as what they
/// are — a bug report worth copying verbatim into an issue.
///
/// Values must be language-neutral (numbers, ids); the catalogues interpolate
/// them into the translated sentence.
pub(crate) fn domain_payload(
    err: &TournamentError,
) -> Option<(&'static str, BTreeMap<String, String>)> {
    /// Shorthand for a code with no interpolation values.
    fn bare(code: &'static str) -> Option<(&'static str, BTreeMap<String, String>)> {
        Some((code, BTreeMap::new()))
    }
    /// Shorthand for a code with interpolation values.
    fn with(
        code: &'static str,
        values: impl IntoIterator<Item = (&'static str, String)>,
    ) -> Option<(&'static str, BTreeMap<String, String>)> {
        Some((
            code,
            values
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        ))
    }

    match err {
        // Registration and players.
        TournamentError::EmptyTournamentName => bare("empty_tournament_name"),
        TournamentError::EmptyPlayerName => bare("empty_player_name"),
        TournamentError::RegistrationAlreadyFinalized => bare("registration_already_finalized"),
        TournamentError::RegistrationNotFinalized => bare("registration_not_finalized"),
        TournamentError::CannotRemoveCupPlayer => bare("cannot_remove_cup_player"),
        TournamentError::CannotRemovePlayedPlayer => bare("cannot_remove_played_player"),

        // Rounds.
        TournamentError::PreviousRoundNotComplete => bare("previous_round_not_complete"),
        TournamentError::NotEnoughPresentPlayers { needed, have } => with(
            "not_enough_present_players",
            [
                ("needed", needed.to_string()),
                ("have", have.to_string()),
            ],
        ),
        TournamentError::NoRoundToCancel => bare("no_round_to_cancel"),
        TournamentError::RoundHasResults => bare("round_has_results"),
        TournamentError::UnresolvedLongGame { round } => {
            with("unresolved_long_game", [("round", round.to_string())])
        }

        // Handicaps.
        TournamentError::HandicapNeedsRatingDifference => bare("handicap_needs_rating_difference"),
        TournamentError::HandicapNotAllowedForCup => bare("handicap_not_allowed_for_cup"),

        // Cup.
        TournamentError::CupSizeRequired => bare("cup_size_required"),
        TournamentError::InvalidCupSize { size } => {
            with("invalid_cup_size", [("size", size.to_string())])
        }
        TournamentError::NotEnoughEligiblePlayers { needed, have } => with(
            "not_enough_eligible_players",
            [
                ("needed", needed.to_string()),
                ("have", have.to_string()),
            ],
        ),

        // Team tournaments. The conflict code comes from the conflict itself, so a
        // new `TeamModeConflict` variant cannot be added without one.
        TournamentError::TeamModeConflict(conflict) => bare(conflict.code()),
        TournamentError::InvalidTeamSize { size } => {
            with("invalid_team_size", [("size", size.to_string())])
        }
        TournamentError::TeamSettingsLocked => bare("team_settings_locked"),
        TournamentError::EmptyTeamName => bare("empty_team_name"),
        TournamentError::DuplicateTeamName(name) => {
            with("duplicate_team_name", [("name", name.clone())])
        }
        TournamentError::TeamIsFull { size } => with("team_is_full", [("size", size.to_string())]),
        TournamentError::PlayersWithoutTeam { count } => {
            with("players_without_team", [("count", count.to_string())])
        }
        TournamentError::IncompleteTeam { name, have, need } => with(
            "incomplete_team",
            [
                ("name", name.clone()),
                ("have", have.to_string()),
                ("need", need.to_string()),
            ],
        ),
        TournamentError::MembersWithoutPairingRating { count } => with(
            "members_without_pairing_rating",
            [("count", count.to_string())],
        ),
        TournamentError::NotEnoughTeams { have } => {
            with("not_enough_teams", [("have", have.to_string())])
        }
        TournamentError::NoLateRegistrationInTeamMode => bare("no_late_registration_in_team_mode"),
        TournamentError::NotEnoughPresentTeams { needed, have } => with(
            "not_enough_present_teams",
            [
                ("needed", needed.to_string()),
                ("have", have.to_string()),
            ],
        ),

        // Point adjustments.
        TournamentError::EmptyAdjustmentReason => bare("empty_adjustment_reason"),
        TournamentError::ZeroPointAdjustment => bare("zero_point_adjustment"),

        // Saved files and settings.
        TournamentError::UnsupportedFormatVersion { found, supported } => with(
            "unsupported_format_version",
            [
                ("found", found.to_string()),
                ("supported", supported.to_string()),
            ],
        ),
        // The inner text comes from the deserializer, so it can't be translated;
        // the catalogues wrap it in a translated sentence and show it as detail.
        TournamentError::MalformedSave(details) => {
            with("malformed_save", [("details", details.clone())])
        }
        TournamentError::EloEstimateUnanchored => bare("elo_estimate_unanchored"),

        // English only: reachable only through a client bug or a race the UI
        // already handles by reloading (see the 409 path in `live`).
        TournamentError::PlayerNotFound(_)
        | TournamentError::CategoryNotFound(_)
        | TournamentError::RoundNotFound(_)
        | TournamentError::BoardNotFound { .. }
        | TournamentError::DrawnOnForfeitedBoard { .. }
        | TournamentError::PlayerNotSittingOut { .. }
        | TournamentError::AdjustmentNotFound { .. }
        | TournamentError::DraftAlreadyExists
        | TournamentError::NoDraft
        | TournamentError::NoCurrentRound
        | TournamentError::NotCurrentRound
        | TournamentError::CupBracketInconsistent
        // Team-mode operations the UI never offers where they don't apply: a
        // roster edit on an individual tournament, a team id that isn't there, a
        // reorder that doesn't name the team's own members.
        | TournamentError::NotATeamTournament
        | TournamentError::TeamNotFound(_)
        | TournamentError::PlayerAlreadyInATeam(_)
        | TournamentError::NotATeamMember { .. }
        | TournamentError::InvalidBoardOrder
        | TournamentError::PairingRatingNotApplicable
        // The team draft UI only offers whole-team absences and team-level
        // forcing, so these two mean a client sent something it never renders.
        | TournamentError::PlayerLevelDraftInTeamMode
        | TournamentError::PlayerAdjustmentInTeamMode
        // The UI only offers a justified absence in team mode, so this means a
        // client sent a kind it never renders there.
        | TournamentError::JustifiedAbsenceOutsideTeamMode
        // Frozen-explanation invariants nothing this program writes can break —
        // a file reaching these was hand-edited, not merely old (an old one is
        // caught by its format version, which *is* translated).
        | TournamentError::MisplacedExplanation { .. }
        | TournamentError::WatermarkPastLastRound { .. }
        // Same class for the cup: its bracket size and seed list are frozen
        // together at finalization, so a file where they disagree (or that seeds
        // a player it doesn't contain) was hand-edited.
        | TournamentError::CupSeedCountMismatch { .. }
        | TournamentError::DuplicateCupSeed { .. }
        | TournamentError::UnknownCupSeed { .. }
        // Deliberately English: unfinished scaffolding (the UI offers no team
        // probe yet), not a state the product is meant to have.
        // TODO: `InvalidDraft` carries a free-form English string built at ~8
        // call sites; it needs to become a structured enum before it can be
        // translated. Referee-visible, so worth doing.
        | TournamentError::InvalidDraft(_) => None,
    }
}

impl From<TournamentError> for ApiError {
    fn from(err: TournamentError) -> Self {
        ApiError::Domain(err)
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
        // Errors carrying a localization code + values, not just a message. The
        // `error` string stays English throughout: it is the fallback the client
        // shows for an untranslated code, and what turns up in logs and `curl`.
        let (status, body) = match self {
            ApiError::CsvImport(err) => {
                let (code, values) = csv_error_payload(&err);
                (
                    StatusCode::BAD_REQUEST,
                    ErrorBody {
                        error: err.to_string(),
                        code: Some(code),
                        values,
                    },
                )
            }
            ApiError::Domain(err) => {
                let status = domain_status(&err);
                let (code, values) = match domain_payload(&err) {
                    Some((code, values)) => (Some(code), values),
                    None => (None, BTreeMap::new()),
                };
                (
                    status,
                    ErrorBody {
                        error: err.to_string(),
                        code,
                        values,
                    },
                )
            }
            ApiError::NoTournament => (
                StatusCode::NOT_FOUND,
                ErrorBody {
                    error: "no tournament exists; create one first".to_string(),
                    code: Some("no_tournament"),
                    values: BTreeMap::new(),
                },
            ),
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, ErrorBody::message(message)),
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, ErrorBody::message(message)),
            // The client turns 401 into the login screen and 409 into a reload,
            // both by status code, so these messages are never shown as-is.
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                ErrorBody::message("authentication required".to_string()),
            ),
            ApiError::Forbidden(message) => (StatusCode::FORBIDDEN, ErrorBody::message(message)),
            ApiError::Conflict(message) => (StatusCode::CONFLICT, ErrorBody::message(message)),
            ApiError::VersionConflict => (
                StatusCode::CONFLICT,
                ErrorBody::message(
                    "the tournament changed since your last view; reload and retry".to_string(),
                ),
            ),
            ApiError::Upstream(message) => (StatusCode::BAD_GATEWAY, ErrorBody::message(message)),
            ApiError::Overloaded(message) => {
                (StatusCode::SERVICE_UNAVAILABLE, ErrorBody::message(message))
            }
        };
        debug_assert!(
            body.code
                .is_none_or(|code| LOCALIZED_ERROR_CODES.contains(&code)),
            "error code {:?} is missing from LOCALIZED_ERROR_CODES, so the frontend \
             cannot have a translation for it",
            body.code,
        );
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osp_core::{TeamModeConflict, TournamentId};
    use uuid::Uuid;

    /// One instance of every [`TournamentError`] variant.
    ///
    /// The exhaustive `match` in [`domain_payload`] makes the compiler demand a
    /// decision about each new variant; this list is what lets the tests below
    /// check that decision was carried through to a registered code. Adding a
    /// variant without adding it here weakens those tests, so keep them in step.
    fn every_domain_error() -> Vec<TournamentError> {
        let uuid = Uuid::nil();
        vec![
            TournamentError::EmptyTournamentName,
            TournamentError::EmptyPlayerName,
            TournamentError::PlayerNotFound(uuid),
            TournamentError::CategoryNotFound(uuid),
            TournamentError::RegistrationAlreadyFinalized,
            TournamentError::RegistrationNotFinalized,
            TournamentError::PreviousRoundNotComplete,
            TournamentError::DraftAlreadyExists,
            TournamentError::NoDraft,
            TournamentError::NotEnoughPresentPlayers { needed: 2, have: 1 },
            TournamentError::InvalidDraft("whatever".to_string()),
            TournamentError::NoRoundToCancel,
            TournamentError::NoCurrentRound,
            TournamentError::RoundHasResults,
            TournamentError::NotCurrentRound,
            TournamentError::UnresolvedLongGame { round: 3 },
            TournamentError::RoundNotFound(7),
            TournamentError::BoardNotFound { round: 2, board: 5 },
            TournamentError::PlayerNotSittingOut {
                round: 2,
                player: TournamentId(4),
            },
            TournamentError::HandicapNeedsRatingDifference,
            TournamentError::HandicapNotAllowedForCup,
            TournamentError::UnsupportedFormatVersion {
                found: 9,
                supported: 3,
            },
            TournamentError::MalformedSave("trailing comma".to_string()),
            TournamentError::EloEstimateUnanchored,
            TournamentError::CupSizeRequired,
            TournamentError::InvalidCupSize { size: 12 },
            TournamentError::NotEnoughEligiblePlayers { needed: 8, have: 5 },
            TournamentError::CupSeedCountMismatch {
                size: 8,
                expected: 8,
                found: 3,
            },
            TournamentError::DuplicateCupSeed {
                seed: TournamentId(4),
            },
            TournamentError::UnknownCupSeed {
                seed: TournamentId(9),
            },
            TournamentError::CannotRemoveCupPlayer,
            TournamentError::CannotRemovePlayedPlayer,
            TournamentError::CupBracketInconsistent,
            TournamentError::EmptyAdjustmentReason,
            TournamentError::ZeroPointAdjustment,
            TournamentError::AdjustmentNotFound {
                player: uuid,
                adjustment: uuid,
            },
            TournamentError::TeamModeConflict(TeamModeConflict::Cup),
            TournamentError::TeamModeConflict(TeamModeConflict::LongGames),
            TournamentError::TeamModeConflict(TeamModeConflict::EloPairing),
            TournamentError::TeamModeConflict(TeamModeConflict::GradeThresholds),
            TournamentError::TeamModeConflict(TeamModeConflict::EstEloTiebreak),
            TournamentError::InvalidTeamSize { size: 12 },
            TournamentError::TeamSettingsLocked,
            TournamentError::NotATeamTournament,
            TournamentError::TeamNotFound(uuid),
            TournamentError::EmptyTeamName,
            TournamentError::DuplicateTeamName("Paris".to_string()),
            TournamentError::PlayerAlreadyInATeam(uuid),
            TournamentError::TeamIsFull { size: 3 },
            TournamentError::NotATeamMember {
                team: uuid,
                player: uuid,
            },
            TournamentError::InvalidBoardOrder,
            TournamentError::PlayersWithoutTeam { count: 2 },
            TournamentError::IncompleteTeam {
                name: "Paris".to_string(),
                have: 2,
                need: 3,
            },
            TournamentError::MembersWithoutPairingRating { count: 1 },
            TournamentError::NotEnoughTeams { have: 1 },
            TournamentError::PairingRatingNotApplicable,
            TournamentError::NoLateRegistrationInTeamMode,
            TournamentError::NotEnoughPresentTeams { needed: 2, have: 1 },
            TournamentError::PlayerLevelDraftInTeamMode,
            TournamentError::PlayerAdjustmentInTeamMode,
            TournamentError::JustifiedAbsenceOutsideTeamMode,
            TournamentError::DrawnOnForfeitedBoard { round: 1, board: 0 },
        ]
    }

    /// A code the frontend has never heard of degrades that message back to the
    /// server's English fallback — silently, which is exactly the failure this
    /// registry exists to prevent.
    #[test]
    fn every_emitted_code_is_registered() {
        for err in every_domain_error() {
            let Some((code, _)) = domain_payload(&err) else {
                continue;
            };
            assert!(
                LOCALIZED_ERROR_CODES.contains(&code),
                "domain_payload returns {code:?} for {err:?}, but it is not in \
                 LOCALIZED_ERROR_CODES, so no locale can translate it"
            );
        }
        for err in [
            CsvImportError::Empty,
            CsvImportError::MissingNameColumns,
            CsvImportError::RowsMissingLastName { rows: vec![2, 5] },
        ] {
            let (code, _) = csv_error_payload(&err);
            assert!(
                LOCALIZED_ERROR_CODES.contains(&code),
                "csv_error_payload returns {code:?}, which is not in LOCALIZED_ERROR_CODES"
            );
        }
    }

    /// The inverse: a registered code nothing emits is dead weight that would
    /// keep an unused key alive in every catalogue.
    #[test]
    fn every_registered_code_is_emitted() {
        let mut emitted: Vec<&str> = every_domain_error()
            .iter()
            .filter_map(|err| domain_payload(err).map(|(code, _)| code))
            .collect();
        emitted.extend([
            "csv_empty",
            "csv_missing_name_columns",
            "csv_rows_missing_last_name",
        ]);
        // Not produced by a domain error: built directly in `into_response`.
        emitted.push("no_tournament");

        for code in LOCALIZED_ERROR_CODES {
            assert!(
                emitted.contains(code),
                "{code:?} is registered in LOCALIZED_ERROR_CODES but nothing emits it"
            );
        }
    }

    /// Interpolation values are what the translated sentence is built from, so a
    /// missing one leaves a literal `{needed}` in the referee's face.
    #[test]
    fn payload_values_cover_the_error() {
        let err = TournamentError::NotEnoughPresentPlayers { needed: 2, have: 1 };
        let (code, values) = domain_payload(&err).expect("localized");
        assert_eq!(code, "not_enough_present_players");
        assert_eq!(values.get("needed").map(String::as_str), Some("2"));
        assert_eq!(values.get("have").map(String::as_str), Some("1"));
    }

    /// Internal errors keep their English message and carry no code, so the
    /// client shows the text verbatim.
    #[test]
    fn internal_errors_are_not_localized() {
        assert!(domain_payload(&TournamentError::CupBracketInconsistent).is_none());
        assert!(domain_payload(&TournamentError::BoardNotFound { round: 1, board: 0 }).is_none());
    }

    /// The 400/404 split has to survive the move out of the `From` impl.
    #[test]
    fn domain_status_splits_missing_from_invalid() {
        assert_eq!(
            domain_status(&TournamentError::PlayerNotFound(Uuid::nil())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            domain_status(&TournamentError::RoundHasResults),
            StatusCode::BAD_REQUEST
        );
    }
}
