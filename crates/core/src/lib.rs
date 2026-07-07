//! Core domain logic for OpenShogiPairings.
//!
//! At this early stage this crate carries the data types shared between the
//! server and its clients ([`Player`], [`Tournament`], …) plus the tournament
//! mutation logic. The pairing engine (Blossom matching over a weighted player
//! graph, and later an ILP/CP-SAT backend) will live here too so that it can be
//! reused unchanged by the HTTP server, a future CLI client, and the Tauri
//! desktop app.

mod american_grid;
mod cup;
mod elo;
mod fesa;
mod grid_import;
mod handicap;
mod matching;
mod pairing;
mod player;
mod round;
mod scoring;
mod settings;
mod standings;
mod tournament;

pub use american_grid::to_grid as american_grid;
pub use cup::{Cup, CupPodium, CUP_SIZES};
pub use elo::estimate_elos;
pub use fesa::{decode_latin1, parse_rating_list, RatedPlayer};
pub use grid_import::{import_american_grid, GridImportError};
pub use handicap::suggested_handicap;
pub use pairing::{pair_round, pair_round_constrained, pair_round_weighted};
pub use player::{NewPlayer, Player, PointAdjustment};
pub use round::{Board, CupStage, Handicap, HandicapGame, PairingSource, Round, RoundDraft, Winner};
pub use settings::{FloaterStyle, HandicapPolicy, Tiebreak, TournamentSettings};
pub use standings::{compute_standings, Standing};
pub use tournament::{
    Tournament, TournamentError, MIN_PLAYERS_PER_ROUND, TOURNAMENT_FORMAT_VERSION,
};

use serde::{Deserialize, Serialize};

/// Human-readable service identifier reported by the health endpoint.
pub const SERVICE_NAME: &str = "openshogipairings-server";

/// The crate version, surfaced to clients so they can detect a server upgrade.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Payload returned by the server's health check.
///
/// Defined here (rather than in the server) precisely so every client — the web
/// UI, the planned CLI, and the Tauri app — can depend on one canonical shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
