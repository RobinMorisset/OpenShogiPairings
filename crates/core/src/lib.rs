//! Core domain logic for OpenShogiPairings.
//!
//! This crate carries the data types shared between the server and its clients
//! ([`Player`], [`Tournament`], …), the tournament mutation logic, and the
//! pairing engine (Blossom matching over a weighted player graph, with an
//! ILP/CP-SAT backend still to come) — so all of it is reused unchanged by the
//! HTTP server, the simulator, and the Tauri desktop app.
//!
//! These docs are written to be read *in the source* as much as in rustdoc, so a
//! public item freely links to the private one that explains it (the rule table
//! behind a pairing, the estimator behind a setting). rustdoc renders those as
//! plain text and warns; `cargo doc --document-private-items` renders them as
//! links. The warning is silenced below because the links are deliberate — a link
//! to something that doesn't exist at all still warns, which is the case worth
//! hearing about.
//!
//! Two allow-by-default lints are turned on, bracketing one question from either
//! side: what is really part of this crate's API? `unreachable_pub` catches `pub`
//! that reaches nothing — the item is crate-visible either way, so the marker
//! only misleads about the answer, which is the `pub use` list below.
//! `unnameable_types` catches the reverse, a type a caller can reach through a
//! public signature but cannot write down. Both had drifted, in both directions
//! at once; with CI denying warnings the markers and the exports now stay in
//! step.
#![allow(rustdoc::private_intra_doc_links)]
#![warn(unnameable_types, unreachable_pub)]

mod american_grid;
mod csv_import;
mod cup;
mod elo;
mod fesa;
mod fesa_results;
mod handicap;
mod licence;
mod matching;
mod pairing;
mod player;
mod result_import;
mod round;
mod scoring;
mod settings;
pub mod sim;
mod standings;
mod team;
mod team_scoring;
mod tournament;
mod units;

pub use american_grid::to_grid as american_grid;
pub use csv_import::{parse_players_csv, CsvImportError};
pub use cup::{
    cup_field_size, knockout_champion, reconstruct_cup_from_final, BracketMatch, Cup,
    CupBracketView, CupFormat, CupMatch, CupPairings, CupPodium, CUP_SIZES,
};
pub use elo::estimate_elos;
pub use fesa::{decode_latin1, parse_rating_list, RatedPlayer};
pub use fesa_results::import_fesa_results;
pub use licence::{check_licences, LicenceCheck, UnlicensedPlayer};
pub use pairing::{
    AffectedCycle, BoardLedger, Counterfactual, CounterfactualMode, RoundExplanation,
    RuleContribution, RuleDelta, RuleId, RuleTotal, ScopeReason,
};
pub use player::{Grade, GradeKind, NewPlayer, Player, PointAdjustment};
pub use result_import::ResultImportError;
pub use round::{
    AbsenceKind, Board, CupStage, ForcedMatch, Forfeit, GameRecord, Handicap, HandicapGame,
    Outcome, PairingSource, Round, RoundDraft, Sitout, SitoutKind, SitoutValue, Winner,
};
pub use settings::{
    ClubProtection, DateError, EloEstimator, EloPriorShape, FloaterStyle, HandicapDisplay,
    HandicapPolicy, IsoDate, MacMahon, MacMahonSource, MacMahonThreshold, NationalityProtection,
    PairingMode, PlayerCategory, Ratio, RatioAtLeastOne, TeamModeConflict, TeamSettings,
    ThresholdCriterion, Tiebreak, TournamentDates, TournamentSettings, UnratedK,
};
pub use standings::{Standing, Tiebreaks};
pub use team::{pairing_rating, Team};
pub use team_scoring::{TeamMatchView, TeamStanding};
pub use tournament::{Tournament, TournamentError, TOURNAMENT_FORMAT_VERSION};
pub use units::{HalfPoints, TeamId, TournamentId, UnitKey, Wins};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Human-readable service identifier reported by the health endpoint.
pub const SERVICE_NAME: &str = "openshogipairings-server";

/// The crate version, surfaced to clients so they can detect a server upgrade.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Payload returned by the server's health check.
///
/// Defined here (rather than in the server) precisely so every client — the web
/// UI, the planned CLI, and the Tauri app — can depend on one canonical shape.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct HealthStatus {
    /// Always `"ok"` when the server is able to respond.
    pub status: String,
    /// Which service answered, useful once there are several backends.
    pub service: String,
    /// Semantic version of the running server.
    pub version: String,
}

impl HealthStatus {
    /// Build the status describing this running build.
    pub fn current() -> Self {
        Self {
            status: "ok".to_string(),
            service: SERVICE_NAME.to_string(),
            version: VERSION.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_status_reports_ok() {
        let status = HealthStatus::current();
        assert_eq!(status.status, "ok");
        assert_eq!(status.service, SERVICE_NAME);
        assert_eq!(status.version, VERSION);
    }
}
