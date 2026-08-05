// Turning a server error into text the referee can act on.
//
// The server tags the errors a referee can actually cause with a stable machine
// `code` plus language-neutral interpolation `values` (see
// `crates/server/src/error_codes.rs`); this maps those codes to translation
// keys. Errors it leaves untagged are the ones only a client bug can reach, and
// their English `message` is shown verbatim — deliberately, because it reads as
// the bug report it is rather than as ordinary UI.

import { ApiError } from "./api";

/** A translate function, e.g. the `$_` store from svelte-i18n (its exact
 * parameter type isn't exported, so this is intentionally loose). */
type Translate = (id: string, opts?: Record<string, unknown>) => string;

/**
 * Server error `code` → translation key.
 *
 * Every code in `LOCALIZED_ERROR_CODES` must appear here, and every key here
 * must exist in the locale catalogues; `errorCodes.test.ts` checks both, because
 * a mismatch fails silently — the referee just sees English again.
 */
export const ERROR_CODE_KEYS: Record<string, string> = {
  no_tournament: "serverError.noTournament",

  csv_empty: "playerRegistration.csvErrorEmpty",
  csv_missing_name_columns: "playerRegistration.csvErrorMissingColumns",
  csv_rows_missing_last_name: "playerRegistration.csvErrorRowsMissingLastName",

  empty_tournament_name: "serverError.emptyTournamentName",
  empty_player_name: "serverError.emptyPlayerName",
  registration_already_finalized: "serverError.registrationAlreadyFinalized",
  registration_not_finalized: "serverError.registrationNotFinalized",
  cannot_remove_cup_player: "serverError.cannotRemoveCupPlayer",
  cannot_remove_played_player: "serverError.cannotRemovePlayedPlayer",

  previous_round_not_complete: "serverError.previousRoundNotComplete",
  not_enough_present_players: "serverError.notEnoughPresentPlayers",
  no_round_to_cancel: "serverError.noRoundToCancel",
  round_has_results: "serverError.roundHasResults",
  unresolved_long_game: "serverError.unresolvedLongGame",

  handicap_needs_rating_difference: "serverError.handicapNeedsRatingDifference",
  handicap_not_allowed_for_cup: "serverError.handicapNotAllowedForCup",

  cup_size_required: "serverError.cupSizeRequired",
  invalid_cup_size: "serverError.invalidCupSize",
  not_enough_eligible_players: "serverError.notEnoughEligiblePlayers",

  empty_adjustment_reason: "serverError.emptyAdjustmentReason",
  zero_point_adjustment: "serverError.zeroPointAdjustment",

  unsupported_format_version: "serverError.unsupportedFormatVersion",
  malformed_save: "serverError.malformedSave",
  elo_estimate_unanchored: "serverError.eloEstimateUnanchored",
};

/**
 * An error carrying one of the stable codes above. Matched structurally rather
 * than by class, because the client can reject something the server would have
 * rejected the same way — a save file whose format version this build cannot
 * read (see `TournamentFileError` in `tournamentFile.ts`) — and the referee
 * should read the same sentence either way.
 */
interface Coded {
  code?: unknown;
  values?: unknown;
}

/** Message to show for a failed request, translated where it was tagged. */
export function describeApiError(err: unknown, t: Translate): string {
  if (err instanceof ApiError && err.status === 0) {
    return t("app.cannotReachServer");
  }
  if (err instanceof Error) {
    const { code, values } = err as Coded;
    if (typeof code === "string" && ERROR_CODE_KEYS[code]) {
      return t(ERROR_CODE_KEYS[code], {
        values: (values as Record<string, string> | undefined) ?? {},
      });
    }
  }
  return err instanceof Error ? err.message : String(err);
}
