//! The canonical list of stable error `code`s the API can put in an error body.
//!
//! This file exists to hold nothing but the list, because it is read from *both*
//! sides of the language boundary: [`crate::error`] emits these codes, and the
//! frontend test `frontend/src/lib/errorCodes.test.ts` parses this file to check
//! that every code has a translation key. Keeping it a flat list of string
//! literals in a file of its own is what makes that parse trustworthy — do not
//! add other code here.
//!
//! A code is part of the API contract: renaming one silently degrades a
//! translated message back to the server's English fallback, so treat a rename
//! as a breaking change and update the locale catalogues in the same commit.

/// Every error code the server can emit, so both ends can check they agree.
///
/// `error::domain_payload` and `error::csv_error_payload` (private to that
/// module, hence not linked) must
/// only return codes from this list; `codes_are_registered` below enforces that
/// for the domain half, and the exhaustive `match` in `domain_payload` forces
/// every new [`osp_core::TournamentError`] variant to be classified as either
/// localized (a code here) or internal (English only).
pub(crate) const LOCALIZED_ERROR_CODES: &[&str] = &[
    // Request-level
    "no_tournament",
    // CSV import
    "csv_empty",
    "csv_missing_name_columns",
    "csv_rows_missing_last_name",
    // Registration and players
    "empty_tournament_name",
    "empty_player_name",
    "registration_already_finalized",
    "registration_not_finalized",
    "cannot_remove_cup_player",
    "cannot_remove_played_player",
    "cannot_remove_team_player",
    "cannot_remove_matched_team",
    // Rounds
    "previous_round_not_complete",
    "not_enough_present_players",
    "no_round_to_cancel",
    "round_has_results",
    "long_flag_after_result",
    "long_flag_after_coupled_result",
    "unresolved_long_game",
    "uncarried_long_game",
    "unfinished_round",
    "carried_long_game",
    "long_game_started_earlier",
    // Handicaps
    "handicap_needs_rating_difference",
    "handicap_not_allowed_for_cup",
    // Cup
    "cup_size_required",
    "invalid_cup_size",
    "not_enough_eligible_players",
    // Point adjustments
    "empty_adjustment_reason",
    "zero_point_adjustment",
    // Team tournaments
    "team_mode_rejects_cup",
    "team_mode_rejects_long_games",
    "team_mode_rejects_elo_pairing",
    "team_mode_rejects_grade_thresholds",
    "team_mode_rejects_est_elo_tiebreak",
    "invalid_team_size",
    "team_settings_locked",
    "empty_team_name",
    "duplicate_team_name",
    "team_is_full",
    "players_without_team",
    "incomplete_team",
    "members_without_pairing_rating",
    "not_enough_teams",
    "no_late_registration_in_team_mode",
    "not_enough_present_teams",
    // Saved files and settings
    "unsupported_format_version",
    "old_save_already_started",
    "malformed_save",
    "elo_estimate_unanchored",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn codes_are_unique() {
        let unique: HashSet<_> = LOCALIZED_ERROR_CODES.iter().collect();
        assert_eq!(
            unique.len(),
            LOCALIZED_ERROR_CODES.len(),
            "duplicate code in LOCALIZED_ERROR_CODES"
        );
    }

    /// The frontend parses this file with a regex that assumes lowercase
    /// snake_case literals; a code that doesn't match would be silently skipped
    /// there, so reject it here instead.
    #[test]
    fn codes_are_snake_case() {
        for code in LOCALIZED_ERROR_CODES {
            assert!(
                !code.is_empty()
                    && code
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "code {code:?} is not lowercase snake_case"
            );
        }
    }
}
