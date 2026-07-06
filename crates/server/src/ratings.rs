//! FESA rating list: fetch, parse, and cache.
//!
//! The list is shared reference data, identical for every referee and slow to
//! change, so the server owns the cache rather than each client: it fetches from
//! FESA at most once per TTL, keeps the parsed list in memory, and persists it to
//! a per-user cache file so restarts (and offline use) don't re-hit FESA. Clients
//! pull the list once and autocomplete locally.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::Json;
use osp_core::{decode_latin1, parse_rating_list, RatedPlayer};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

/// The rating list to fetch. The URL encodes the list's publication date; FESA
/// publishes a new one periodically. Hardcoded for now — make it configurable
/// (or discover the latest) when list rotation matters.
const FESA_URL: &str = "https://fesashogi.eu/old/ratinglists/2026-06-01.txt";

/// How long a cached list is considered fresh before we re-fetch.
const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60); // 7 days

/// The cached rating list plus when it was fetched (Unix seconds).
#[derive(Clone, Serialize, Deserialize)]
pub struct CachedRatings {
    fetched_at: u64,
    players: Vec<RatedPlayer>,
}

impl CachedRatings {
    fn is_fresh(&self) -> bool {
        now_secs().saturating_sub(self.fetched_at) < CACHE_TTL.as_secs()
    }
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

fn save_to_disk(cached: &CachedRatings) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_vec_pretty(cached) {
        let _ = std::fs::write(&path, json);
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

/// Return the rating list, using the freshest available source.
///
/// Order: fresh in-memory cache → fresh disk cache → fetch from FESA. If the
/// fetch fails, fall back to any stale cache so autocomplete keeps working
/// offline; only error if there is nothing cached at all.
async fn get_ratings(state: &AppState) -> Result<Vec<RatedPlayer>, ApiError> {
    // 1. Fresh in memory?
    if let Some(cached) = state.ratings.read().expect("ratings lock poisoned").as_ref() {
        if cached.is_fresh() {
            return Ok(cached.players.clone());
        }
    }

    // 2. Fresh on disk? Promote into memory.
    if let Some(disk) = load_from_disk() {
        if disk.is_fresh() {
            let players = disk.players.clone();
            *state.ratings.write().expect("ratings lock poisoned") = Some(disk);
            return Ok(players);
        }
    }

    // 3. Fetch fresh from FESA.
    match fetch_from_fesa().await {
        Ok(players) => {
            let cached = CachedRatings {
                fetched_at: now_secs(),
                players: players.clone(),
            };
            save_to_disk(&cached);
            *state.ratings.write().expect("ratings lock poisoned") = Some(cached);
            Ok(players)
        }
        Err(err) => {
            // 4. Serve stale rather than nothing.
            if let Some(cached) = state.ratings.read().expect("ratings lock poisoned").as_ref() {
                tracing::warn!("serving stale in-memory ratings after fetch error: {err:?}");
                return Ok(cached.players.clone());
            }
            if let Some(disk) = load_from_disk() {
                tracing::warn!("serving stale on-disk ratings after fetch error: {err:?}");
                let players = disk.players.clone();
                *state.ratings.write().expect("ratings lock poisoned") = Some(disk);
                return Ok(players);
            }
            Err(err)
        }
    }
}

/// `GET /api/ratings` — the FESA rating list for client-side autocomplete.
pub async fn ratings_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<RatedPlayer>>, ApiError> {
    get_ratings(&state).await.map(Json)
}
