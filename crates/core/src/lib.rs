//! Core domain logic for OpenShogiPairings.
//!
//! At this early stage this crate carries the data types shared between the
//! server and its clients ([`Player`], [`Tournament`], …) plus the tournament
//! mutation logic. The pairing engine (Blossom matching over a weighted player
//! graph, and later an ILP/CP-SAT backend) will live here too so that it can be
//! reused unchanged by the HTTP server, a future CLI client, and the Tauri
//! desktop app.

mod american_grid;
mod csv_import;
mod cup;
mod elo;
mod fesa;
mod fesa_results;
mod handicap;
mod matching;
mod pairing;
mod player;
mod result_import;
mod round;
mod scoring;
mod settings;
pub mod sim;
mod standings;
mod tournament;
mod units;

pub use american_grid::to_grid as american_grid;
pub use csv_import::{parse_players_csv, CsvImportError};
pub use cup::{
    cup_field_size, knockout_champion, reconstruct_cup_from_final, BracketMatch, Cup,
    CupBracketView, CupFormat, CupPodium, CUP_SIZES,
};
pub use elo::{estimate_elos, PROVISIONAL_GAMES_THRESHOLD};
pub use fesa::{decode_latin1, parse_rating_list, RatedPlayer};
pub use fesa_results::import_fesa_results;
pub use pairing::{
    AffectedCycle, BoardLedger, Counterfactual, CounterfactualMode, RoundExplanation,
    RuleContribution, RuleDelta, RuleId, RuleTotal, ScopeReason,
};
pub use player::{Grade, GradeKind, NewPlayer, Player, PointAdjustment};
pub use result_import::ResultImportError;
pub use round::{
    Board, CupStage, Handicap, HandicapGame, NoShow, PairingSource, Round, RoundDraft, Sitout,
    SitoutKind, SitoutValue, Winner,
};
pub use settings::{
    ClubProtection, EloEstimator, EloPriorShape, FloaterStyle, HandicapDisplay, HandicapPolicy,
    MacMahon, MacMahonSource, MacMahonThreshold, PairingMode, Ratio, RatioAtLeastOne,
    ThresholdCriterion, Tiebreak, TournamentSettings, UnratedK,
};
pub use standings::{compute_standings, Standing};
pub use tournament::{
    Tournament, TournamentError, MIN_PLAYERS_PER_ROUND, TOURNAMENT_FORMAT_VERSION,
};
pub use units::{HalfPoints, TournamentId, UnitKey, Wins};

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
