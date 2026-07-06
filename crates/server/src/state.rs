//! Shared server state.

use std::sync::{Arc, RwLock};

use osp_core::Tournament;

/// State shared across all requests.
///
/// The server currently holds a single "current tournament" in memory — this is
/// the single source of truth that every connected referee reads and mutates.
/// It is wrapped in an [`RwLock`] so concurrent reads don't block each other,
/// and [`Arc`]-shared so axum can clone the state cheaply per request.
///
/// Persistence is explicit for now: clients save by downloading the tournament
/// as JSON and load by uploading it back (which replaces this value). A proper
/// datastore can slot in behind the same handlers later.
#[derive(Clone, Default)]
pub struct AppState {
    pub tournament: Arc<RwLock<Option<Tournament>>>,
}
