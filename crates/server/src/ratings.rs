//! FESA rating list: fetch, parse, and cache.
//!
//! The list is shared reference data, identical for every referee and slow to
//! change, so the server owns the cache rather than each client. It fetches from
//! FESA **only once**, when nothing is cached yet, then keeps the parsed list in
//! memory and persists it to a per-user cache file so restarts (and offline use)
//! don't re-hit FESA. Updating the list is an explicit, user-driven action
//! (the refresh button → [`refresh_from_fesa`]); it never re-downloads on its
//! own. Clients pull the list once and autocomplete locally.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::Json;
use osp_core::{decode_latin1, parse_rating_list, RatedPlayer};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

/// The rating list to fetch. FESA always serves the current list at this path.
const FESA_URL: &str = "https://fesashogi.eu/old/ratinglists/latest.txt";

/// The cached rating list plus when it was fetched (Unix seconds, informational).
#[derive(Clone, Serialize, Deserialize)]
pub struct CachedRatings {
    fetched_at: u64,
    players: Vec<RatedPlayer>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("openshogipairings").join("fesa-ratings.json"))
}

fn load_from_disk() -> Option<CachedRatings> {
    let bytes = std::fs::read(cache_path()?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Write the freshly-fetched list to the on-disk cache. Best-effort — the list
/// is already in memory, so a failure costs a re-download at the next start, not
/// this session — but it is logged: a cache that silently never writes looks
/// exactly like one that is working, until the referee is offline at a
/// tournament and the ratings aren't there.
fn save_to_disk(cached: &CachedRatings) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!("ratings: could not create {}: {e}", parent.display());
            return;
        }
    }
    match serde_json::to_vec_pretty(cached) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!("ratings: could not cache to {}: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("ratings: could not serialize the cached list: {e}"),
    }
}

/// Download and parse the list from FESA.
async fn fetch_from_fesa() -> Result<Vec<RatedPlayer>, ApiError> {
    let response = reqwest::get(FESA_URL)
        .await
        .map_err(|e| ApiError::Upstream(format!("FESA request failed: {e}")))?;
    if !response.status().is_success() {
        return Err(ApiError::Upstream(format!(
            "FESA returned status {}",
            response.status()
        )));
    }
    // Use raw bytes, not `.text()`: the file is Latin-1, not UTF-8.
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ApiError::Upstream(format!("reading FESA response failed: {e}")))?;

    let players = parse_rating_list(&decode_latin1(&bytes));
    if players.is_empty() {
        return Err(ApiError::Upstream(
            "FESA list parsed to zero players (format may have changed)".into(),
        ));
    }
    Ok(players)
}

/// Return the cached rating list, fetching once from FESA only if nothing is
/// cached yet.
///
/// Lookup order: in-memory → on-disk (promoted into memory) → fetch from FESA
/// (first run only). Once a list is cached it is *never* re-fetched
/// automatically — use [`refresh_from_fesa`] (the refresh button) to update it.
async fn get_ratings(state: &AppState) -> Result<Vec<RatedPlayer>, ApiError> {
    // 1. Already in memory?
    if let Some(cached) = state
        .ratings
        .read()
        .expect("ratings lock poisoned")
        .as_ref()
    {
        return Ok(cached.players.clone());
    }

    // 2. On disk? Promote into memory.
    if let Some(disk) = load_from_disk() {
        let players = disk.players.clone();
        *state.ratings.write().expect("ratings lock poisoned") = Some(disk);
        return Ok(players);
    }

    // 3. Nothing cached yet: do the one-time initial fetch.
    refresh_from_fesa(state).await
}

/// Re-download the list from FESA and replace the cache.
///
/// On failure the existing cache is left untouched and the error propagates, so
/// a failed manual refresh reports the problem without losing the current list.
async fn refresh_from_fesa(state: &AppState) -> Result<Vec<RatedPlayer>, ApiError> {
    let players = fetch_from_fesa().await?;
    let cached = CachedRatings {
        fetched_at: now_secs(),
        players: players.clone(),
    };
    save_to_disk(&cached);
    *state.ratings.write().expect("ratings lock poisoned") = Some(cached);
    Ok(players)
}

/// The cached FESA list **without** ever fetching — in-memory, else on-disk
/// (promoted into memory), else empty.
///
/// Used by CSV import to fill in missing ratings/grades: enrichment is
/// best-effort, so a cold cache simply means no enrichment (the same as when the
/// client's autocomplete list failed to load), never a blocking network round-trip
/// or a failed import.
pub(crate) fn cached_ratings(state: &AppState) -> Vec<RatedPlayer> {
    if let Some(cached) = state
        .ratings
        .read()
        .expect("ratings lock poisoned")
        .as_ref()
    {
        return cached.players.clone();
    }
    if let Some(disk) = load_from_disk() {
        let players = disk.players.clone();
        *state.ratings.write().expect("ratings lock poisoned") = Some(disk);
        return players;
    }
    Vec::new()
}

/// `GET /api/ratings` — cached FESA list (fetched once if nothing is cached yet).
pub async fn ratings_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<RatedPlayer>>, ApiError> {
    get_ratings(&state).await.map(Json)
}

/// `POST /api/ratings/refresh` — re-download the list from FESA now.
pub async fn refresh_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<RatedPlayer>>, ApiError> {
    refresh_from_fesa(&state).await.map(Json)
}
