//! Shared server state.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use osp_core::{Tournament, TournamentError};
use tokio::sync::broadcast;

use crate::auth::AuthConfig;
use crate::ratings::CachedRatings;

/// Capacity of the change-notification channel. Small on purpose: subscribers
/// only care about the *latest* version, and one that lags past the buffer is
/// simply told to resync (see [`crate::live`]).
const NOTIFY_CAPACITY: usize = 16;

/// The current tournament plus a linear undo history.
///
/// The history is a stack of full tournament snapshots taken *before* each
/// player mutation; undo pops the most recent one. Snapshots are cheap (a
/// tournament is just a list of players), which is why we snapshot rather than
/// track inverse commands. Creating or loading a tournament resets the history,
/// so undo is scoped to edits of the current tournament.
///
/// When [`persist_path`](Self::persist_path) is set (the hosted server passes a
/// data file), the *current* tournament is written through to disk after every
/// change, and reloaded on boot — so an always-on server survives a restart. The
/// undo history is session state and deliberately not persisted.
///
/// A monotonic [`version`](Self::version) is bumped on every change and pushed to
/// subscribers of [`notifier`](Self::notifier), which drives both live sync (the
/// SSE stream) and optimistic-concurrency conflict detection — see
/// [`crate::live`].
pub struct TournamentStore {
    current: Option<Tournament>,
    history: Vec<Tournament>,
    /// Where to write the current tournament, or `None` for in-memory only
    /// (embedded desktop / dev / tests).
    persist_path: Option<PathBuf>,
    /// Bumped on every state change; echoed by clients to detect stale edits.
    version: u32,
    /// Broadcasts the new version to connected clients after each change.
    notifier: broadcast::Sender<u32>,
}

impl Default for TournamentStore {
    fn default() -> Self {
        let (notifier, _) = broadcast::channel(NOTIFY_CAPACITY);
        Self {
            current: None,
            history: Vec::new(),
            persist_path: None,
            version: 0,
            notifier,
        }
    }
}

impl TournamentStore {
    /// Build a store backed by `path`: load any tournament already saved there,
    /// and write the current one through to it on every change.
    pub fn with_persistence(path: PathBuf) -> Self {
        let current = Self::load_from_disk(&path);
        if current.is_some() {
            tracing::info!("loaded the saved tournament from {}", path.display());
        }
        Self {
            current,
            persist_path: Some(path),
            ..Default::default()
        }
    }

    /// The current change version. Bumped on every state change.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Subscribe to change notifications: each message is the new [`version`].
    pub fn subscribe(&self) -> broadcast::Receiver<u32> {
        self.notifier.subscribe()
    }

    /// Advance the version and notify subscribers. Called after every change so
    /// connected clients can refetch (live sync). A send error just means no one
    /// is currently listening, which is fine.
    fn bump_and_notify(&mut self) {
        self.version = self.version.wrapping_add(1);
        let _ = self.notifier.send(self.version);
    }

    /// Read and parse the persisted tournament, or `None` if the file is absent
    /// or unreadable/corrupt (in which case the server just starts empty).
    fn load_from_disk(path: &Path) -> Option<Tournament> {
        let bytes = fs::read(path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(tournament) => Some(tournament),
            Err(e) => {
                tracing::warn!("could not parse {}: {e}; starting empty", path.display());
                None
            }
        }
    }

    /// Write the current tournament through to disk, if a path is configured.
    ///
    /// Best-effort and atomic (temp file + rename): a persistence failure is
    /// logged, never propagated — it must not break the referee's action, just
    /// as backups don't (see [`crate::backup`]).
    fn persist(&self) {
        let (Some(path), Some(current)) = (&self.persist_path, &self.current) else {
            return;
        };
        let bytes = match serde_json::to_vec_pretty(current) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!("persist: could not serialize the tournament: {e}");
                return;
            }
        };
        let tmp = path.with_extension("tmp");
        if let Err(e) = fs::write(&tmp, &bytes) {
            tracing::warn!("persist: could not write {}: {e}", tmp.display());
            return;
        }
        // `rename` replaces the destination atomically on both Unix and Windows.
        if let Err(e) = fs::rename(&tmp, path) {
            tracing::warn!("persist: could not replace {}: {e}", path.display());
        }
    }

    /// The current tournament, if one exists.
    pub fn current(&self) -> Option<&Tournament> {
        self.current.as_ref()
    }

    /// Whether there is anything to undo.
    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    /// Set the current tournament and clear the undo history.
    ///
    /// Used by create and load — these establish a new baseline, so prior
    /// history no longer applies.
    pub fn set_current(&mut self, tournament: Tournament) {
        self.current = Some(tournament);
        self.history.clear();
        self.persist();
        self.bump_and_notify();
    }

    /// Apply a mutation to the current tournament, checkpointing first.
    ///
    /// The mutation runs against a clone; only if it succeeds do we snapshot the
    /// previous state onto the history and swap in the new one. A failed
    /// mutation (e.g. validation error) leaves state and history untouched.
    pub fn mutate<F>(&mut self, f: F) -> Result<(), MutateError>
    where
        F: FnOnce(&mut Tournament) -> Result<(), TournamentError>,
    {
        let mut next = match &self.current {
            Some(current) => current.clone(),
            None => return Err(MutateError::NoTournament),
        };
        f(&mut next).map_err(MutateError::Domain)?;
        let previous = self.current.replace(next).expect("current was Some");
        self.history.push(previous);
        self.persist();
        self.bump_and_notify();
        Ok(())
    }

    /// Revert to the previous state, if any. No-op when history is empty.
    pub fn undo(&mut self) {
        if let Some(previous) = self.history.pop() {
            self.current = Some(previous);
            self.persist();
            self.bump_and_notify();
        }
    }
}

/// Failure of [`TournamentStore::mutate`].
#[derive(Debug)]
pub enum MutateError {
    /// No tournament exists to mutate.
    NoTournament,
    /// The mutation itself violated a domain rule.
    Domain(TournamentError),
}

/// State shared across all requests.
///
/// The tournament store is the single source of truth every connected referee
/// reads and mutates; it and the ratings cache are `RwLock`-guarded and
/// `Arc`-shared so axum can clone the state cheaply per request.
#[derive(Clone, Default)]
pub struct AppState {
    pub store: Arc<RwLock<TournamentStore>>,
    /// Cached FESA rating list (see [`crate::ratings`]).
    pub ratings: Arc<RwLock<Option<CachedRatings>>>,
    /// Shared-password auth, or `None` in local/embedded mode where the API is
    /// loopback-only and needs no gate (see [`crate::auth`]).
    pub auth: Option<AuthConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use osp_core::NewPlayer;

    #[test]
    fn persists_current_and_reloads_from_disk() {
        let path = std::env::temp_dir().join(format!("osp-persist-{}.json", uuid::Uuid::new_v4()));

        // Write through a create + a player mutation.
        {
            let mut store = TournamentStore::with_persistence(path.clone());
            store.set_current(Tournament::new("Persisted Cup").unwrap());
            store
                .mutate(|t| {
                    t.add_player(NewPlayer {
                        last_name: "Bob".into(),
                        ..Default::default()
                    })
                    .map(|_| ())
                })
                .unwrap();
        }

        // A fresh store over the same path resumes exactly where we left off.
        let reloaded = TournamentStore::with_persistence(path.clone());
        let tournament = reloaded.current().expect("tournament was persisted");
        assert_eq!(tournament.name, "Persisted Cup");
        assert_eq!(tournament.players.len(), 1);
        assert_eq!(tournament.players[0].last_name, "Bob");

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn changes_bump_the_version_and_notify_subscribers() {
        let mut store = TournamentStore::default();
        assert_eq!(store.version(), 0);

        let mut rx = store.subscribe();
        store.set_current(Tournament::new("Cup").unwrap());

        // The version advanced and the new value was broadcast.
        assert_eq!(store.version(), 1);
        assert_eq!(rx.recv().await.unwrap(), 1);

        store
            .mutate(|t| {
                t.add_player(NewPlayer {
                    last_name: "Bob".into(),
                    ..Default::default()
                })
                .map(|_| ())
            })
            .unwrap();
        assert_eq!(store.version(), 2);
        assert_eq!(rx.recv().await.unwrap(), 2);
    }

    #[test]
    fn undo_is_written_through_to_disk() {
        let path = std::env::temp_dir().join(format!("osp-undo-{}.json", uuid::Uuid::new_v4()));

        let mut store = TournamentStore::with_persistence(path.clone());
        store.set_current(Tournament::new("Cup").unwrap());
        store
            .mutate(|t| {
                t.add_player(NewPlayer {
                    last_name: "Alice".into(),
                    ..Default::default()
                })
                .map(|_| ())
            })
            .unwrap();
        store.undo();

        // The on-disk copy reflects the undo (no players), not the pre-undo state.
        let reloaded = TournamentStore::with_persistence(path.clone());
        assert!(reloaded.current().unwrap().players.is_empty());

        std::fs::remove_file(&path).ok();
    }
}
