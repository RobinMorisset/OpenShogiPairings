//! Shared server state: a registry of tournament instances.
//!
//! See `docs/multi-tournament.md`. One process can now hold several
//! tournaments; [`TournamentRegistry`] is the map from each tournament's own
//! id (already a stable `Uuid` on [`Tournament`]) to its
//! [`TournamentInstance`] (live state + its own optional password). Handlers
//! resolve the instance for a request via [`crate::scope::TournamentCtx`].

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use osp_core::{Tournament, TournamentError};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use ts_rs::TS;
use uuid::Uuid;

use crate::auth::AuthConfig;
use crate::ratings::CachedRatings;

/// Capacity of the change-notification channel. Small on purpose: subscribers
/// only care about the *latest* version, and one that lags past the buffer is
/// simply told to resync (see [`crate::live`]).
const NOTIFY_CAPACITY: usize = 16;

/// Peek at only the `format_version` of a serialized tournament and reject an
/// incompatible one *before* a full deserialize is attempted.
///
/// A format bump can reshape a field's serde representation (v5, for instance,
/// made `handicap_policy` an internally-tagged enum, so an old bare-string value
/// no longer deserializes at all). A full `from_slice::<Tournament>` of such a
/// file therefore fails deep inside a changed field with an opaque, misleading
/// error — hiding the real cause, the format version, which the version field
/// exists precisely to surface. Every path that loads an untrusted save (server
/// startup, the "load file" endpoint) runs this first, so an old file is
/// rejected loudly with a clear version message rather than mis-parsed or
/// silently dropped. Lives in the server (not `osp-core`) because it is the
/// server that parses JSON — core has no runtime JSON dependency.
pub fn check_format_version(bytes: &[u8]) -> Result<(), TournamentError> {
    #[derive(Deserialize)]
    struct VersionProbe {
        format_version: Option<u32>,
    }
    let probe: VersionProbe =
        serde_json::from_slice(bytes).map_err(|e| TournamentError::MalformedSave(e.to_string()))?;
    // A missing field means "current" (matches the `Tournament` field's own
    // serde default), so only a present, mismatched version is rejected.
    let found = probe
        .format_version
        .unwrap_or(osp_core::TOURNAMENT_FORMAT_VERSION);
    if found != osp_core::TOURNAMENT_FORMAT_VERSION {
        return Err(TournamentError::UnsupportedFormatVersion {
            found,
            supported: osp_core::TOURNAMENT_FORMAT_VERSION,
        });
    }
    Ok(())
}

/// The current tournament plus a linear undo history.
///
/// The history is a stack of full tournament snapshots taken *before* each
/// player mutation; undo pops the most recent one. Snapshots are cheap (a
/// tournament is just a list of players), which is why we snapshot rather than
/// track inverse commands. Loading a tournament resets the history, so undo is
/// scoped to edits made since.
///
/// When [`persist_path`](Self::persist_path) is set (a hosted server passes a
/// per-tournament file, see [`TournamentRegistry`]), the *current* tournament
/// is written through to disk after every change, and reloaded on boot — so an
/// always-on server survives a restart. The undo history is session state and
/// deliberately not persisted.
///
/// A monotonic [`version`](Self::version) is bumped on every change and pushed
/// to subscribers of [`notifier`](Self::notifier), which drives both live sync
/// (the SSE stream) and optimistic-concurrency conflict detection — see
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
    /// Set once the tournament has been removed from the registry. A store can
    /// still be reached by an in-flight request holding an `Arc` to its
    /// instance; the flag makes [`persist`](Self::persist) and [`mutate`](Self::mutate)
    /// no-op/refuse so a late write can't resurrect the deleted files on disk.
    deleted: bool,
}

impl TournamentStore {
    /// A store with no tournament yet, optionally backed by `persist_path` for
    /// when one is set later via [`set_current`](Self::set_current) (used when
    /// *creating* a brand-new tournament — there is nothing to load).
    fn empty(persist_path: Option<PathBuf>) -> Self {
        let (notifier, _) = broadcast::channel(NOTIFY_CAPACITY);
        Self {
            current: None,
            history: Vec::new(),
            persist_path,
            version: 0,
            notifier,
            deleted: false,
        }
    }

    /// Build a store backed by `path`: load any tournament already saved there,
    /// and write the current one through to it on every change.
    pub fn with_persistence(path: PathBuf) -> Self {
        let current = Self::load_from_disk(&path);
        if current.is_some() {
            tracing::info!("loaded the saved tournament from {}", path.display());
        }
        Self {
            current,
            ..Self::empty(Some(path))
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
    /// or can't be loaded.
    ///
    /// A file that is present but unreadable — a wrong/old format version, or
    /// corrupt bytes — is reported loudly at `error` level and the file is left
    /// **untouched** (the caller drops the instance instead of inserting an empty
    /// one, so nothing ever persists over it). This is deliberate: an old file
    /// from before an incompatible format bump must be rejected, not silently
    /// discarded — losing an in-progress event on a routine binary upgrade would
    /// be far worse than refusing to serve it.
    fn load_from_disk(path: &Path) -> Option<Tournament> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                // Genuinely absent is the normal empty-start case; anything else
                // (permissions, I/O) is worth surfacing.
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::error!("could not read {}: {e}", path.display());
                }
                return None;
            }
        };
        // Check the format version before the full parse, so an incompatible old
        // save fails with a clear version message rather than an opaque
        // field-level serde error (see [`check_format_version`]).
        if let Err(e) = check_format_version(&bytes) {
            tracing::error!(
                "refusing to load {}: {e}. The file is left untouched — load it \
                 with a matching build, or remove it to start fresh.",
                path.display()
            );
            return None;
        }
        match serde_json::from_slice(&bytes) {
            Ok(tournament) => Some(tournament),
            Err(e) => {
                tracing::error!(
                    "could not parse {}: {e}. The file is left untouched.",
                    path.display()
                );
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
        // A tournament removed from the registry must never be written back —
        // otherwise a mutation still in flight when it was deleted would recreate
        // its file after `remove` deleted it (see [`TournamentRegistry::remove`]).
        if self.deleted {
            return;
        }
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
    ///
    /// `expected` is the tournament version the client's edit was based on, or
    /// `None` to skip the check. Comparing it here — under the same write lock the
    /// mutation runs in — is what makes optimistic-concurrency detection actually
    /// atomic: the [`crate::live::check_version`] middleware only pre-screens (it
    /// reads the version and releases the lock before the handler runs), so two
    /// edits racing from the same base version must be separated *here*, or the
    /// later one would silently clobber the earlier one with no `409`.
    pub fn mutate<F>(&mut self, expected: Option<u32>, f: F) -> Result<(), MutateError>
    where
        F: FnOnce(&mut Tournament) -> Result<(), TournamentError>,
    {
        // A store removed from the registry is gone; refuse rather than mutate a
        // clone whose `persist` would no-op anyway.
        if self.deleted {
            return Err(MutateError::NoTournament);
        }
        if let Some(expected) = expected {
            if self.version != expected {
                return Err(MutateError::VersionConflict);
            }
        }
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

    /// Whether this store has been tombstoned by a registry [`remove`](TournamentRegistry::remove).
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// Tombstone the store: it has been removed from the registry, so no further
    /// mutation, persistence, or backup should touch it.
    fn mark_deleted(&mut self) {
        self.deleted = true;
    }

    /// Confirm the caller's edit was based on the current version, for the
    /// state-replacing mutations (`set_current` via load/restore/import, and
    /// `undo`) that don't go through [`mutate`]'s built-in check.
    ///
    /// Call it **under the write lock**, immediately before the mutation: that is
    /// what makes optimistic concurrency atomic here. [`crate::live::check_version`]
    /// only pre-screens (it reads the version and releases the lock before the
    /// handler runs), so without this in-lock re-check two edits racing from the
    /// same base version would both pass the pre-screen and the later one would
    /// silently clobber the earlier — a `200` where the client is owed a `409`.
    /// `expected` is `None` when the client opts out of concurrency checking.
    pub fn ensure_current_version(&self, expected: Option<u32>) -> Result<(), MutateError> {
        match expected {
            Some(expected) if self.version != expected => Err(MutateError::VersionConflict),
            _ => Ok(()),
        }
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
    /// The edit was based on a tournament version that is no longer current —
    /// another writer changed it first (optimistic-concurrency conflict, `409`).
    VersionConflict,
    /// The mutation itself violated a domain rule.
    Domain(TournamentError),
}

/// One tournament's live state plus its own access control.
///
/// `auth` is `None` for an open tournament (no password was set at creation —
/// the normal case in local/embedded mode) and `Some` for a password-protected
/// one; unlike [`AppState::admin_auth`] (which gates *creating* tournaments),
/// this gates reading/editing *this* tournament specifically.
pub struct TournamentInstance {
    pub store: RwLock<TournamentStore>,
    pub auth: Option<AuthConfig>,
}

impl TournamentInstance {
    /// Acquire the store for reading / writing, **recovering from a poisoned
    /// lock** rather than propagating the panic.
    ///
    /// A `std::sync::RwLock` is poisoned only when a thread panics *while holding
    /// the write guard*, and the poison flag means "the guarded value may have
    /// been left half-updated, with its invariants broken." Recovering with
    /// [`PoisonError::into_inner`](std::sync::PoisonError::into_inner) is sound
    /// **here** because that half-updated state cannot arise: every write path
    /// installs an already-complete value in a single move — [`mutate`] runs the
    /// closure against a *clone* and only swaps it in (`current.replace(next)`)
    /// *after* the closure returns `Ok`, and [`set_current`]/[`undo`] likewise
    /// replace `current` wholesale. So a panic mid-mutation unwinds *before* any
    /// partial write reaches `current`; the `TournamentStore` behind the lock is
    /// always a complete, consistent value, poison flag or not.
    ///
    /// The payoff is that one panicking mutation (e.g. an unexpected `osp-core`
    /// panic) fails only that one request, instead of poisoning the lock and
    /// turning every later request for this tournament — reads included — into a
    /// 500 until the process restarts. The registry's own methods ([`list`],
    /// [`remove`]) already recover the same way, so this keeps the per-tournament
    /// handlers consistent with them.
    ///
    /// [`mutate`]: TournamentStore::mutate
    /// [`set_current`]: TournamentStore::set_current
    /// [`undo`]: TournamentStore::undo
    /// [`list`]: TournamentRegistry::list
    /// [`remove`]: TournamentRegistry::remove
    pub fn read(&self) -> RwLockReadGuard<'_, TournamentStore> {
        self.store.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Acquire the store for writing, recovering from a poisoned lock — see
    /// [`read`](Self::read) for why that recovery is safe here.
    pub fn write(&self) -> RwLockWriteGuard<'_, TournamentStore> {
        self.store.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// Summary of a tournament for the picker list (`GET /api/tournaments`) — name
/// and whether it's locked, never the tournament's contents.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct TournamentSummary {
    pub id: Uuid,
    pub name: String,
    pub has_password: bool,
}

/// What's persisted alongside a tournament file for the bits that live outside
/// `osp-core` (the password hash). Kept as its own small sidecar file
/// (`{id}.auth.json`) rather than folded into `TournamentStore`'s persistence,
/// so the store stays ignorant of auth, same as before this feature.
#[derive(Serialize, Deserialize, Default)]
struct PersistedAuth {
    password_hash: Option<String>,
}

/// All tournaments known to this server process.
///
/// Two lock levels: this outer map is only written on create/delete, so every
/// request for an *existing* tournament only needs a brief **read** lock here
/// (to clone out the `Arc<TournamentInstance>`) plus whatever lock it takes on
/// that one instance's own store — two referees editing different tournaments
/// never contend on the same lock.
pub struct TournamentRegistry {
    instances: RwLock<HashMap<Uuid, Arc<TournamentInstance>>>,
    /// Directory holding one `{id}.json` (+ `{id}.auth.json`) per tournament,
    /// or `None` for in-memory-only (embedded/dev/tests).
    data_dir: Option<PathBuf>,
}

impl Default for TournamentRegistry {
    fn default() -> Self {
        Self::new(None)
    }
}

impl TournamentRegistry {
    /// Build a registry, loading every tournament already saved under
    /// `data_dir` (eager load-all: simplest, and fine at the scale one host
    /// actually runs — a handful of tournaments). A file that fails to parse is
    /// skipped with a warning, same as a single corrupt `OSP_DATA_FILE` used to
    /// be.
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        let mut instances = HashMap::new();
        if let Some(dir) = &data_dir {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    // Skip the auth sidecar files; they're loaded alongside
                    // their tournament below, not as entries of their own.
                    if path.extension().and_then(|e| e.to_str()) != Some("json")
                        || stem.ends_with(".auth")
                    {
                        continue;
                    }
                    let Ok(id) = Uuid::parse_str(stem) else {
                        tracing::warn!("skipping {}: not a tournament id", path.display());
                        continue;
                    };
                    let store = TournamentStore::with_persistence(path.clone());
                    if store.current().is_none() {
                        continue; // already warned inside `with_persistence`
                    }
                    // Fail closed: if the sidecar is present but unreadable we
                    // can't tell whether this tournament is password-protected, so
                    // refuse to load it rather than serve it open.
                    let auth = match Self::load_auth(dir, id) {
                        Ok(hash) => hash.map(AuthConfig::from_hash),
                        Err(e) => {
                            tracing::error!(
                                "refusing to load tournament {id}: its auth sidecar is \
                                 unreadable ({e}). Failing closed rather than serving a \
                                 password-protected tournament with no password; fix or \
                                 remove {id}.auth.json."
                            );
                            continue;
                        }
                    };
                    instances.insert(
                        id,
                        Arc::new(TournamentInstance {
                            store: RwLock::new(store),
                            auth,
                        }),
                    );
                }
            }
        }
        Self {
            instances: RwLock::new(instances),
            data_dir,
        }
    }

    fn tournament_path(&self, id: Uuid) -> Option<PathBuf> {
        self.data_dir
            .as_ref()
            .map(|dir| dir.join(format!("{id}.json")))
    }

    fn auth_path(&self, id: Uuid) -> Option<PathBuf> {
        self.data_dir
            .as_ref()
            .map(|dir| dir.join(format!("{id}.auth.json")))
    }

    /// Load the password hash from the `{id}.auth.json` sidecar, as three
    /// distinct outcomes so the caller can **fail closed** on damage:
    /// - the file is absent → `Ok(None)`: the tournament was created without a
    ///   password. A create *with* a password writes the sidecar before the
    ///   tournament file (see [`create`](Self::create)), so a present tournament
    ///   with no sidecar is genuinely open, not a torn create;
    /// - the file is present and valid → `Ok(Some(hash))`: password-protected;
    /// - the file is present but unreadable or corrupt → `Err(..)`: the caller
    ///   must refuse to load the tournament rather than expose a protected one
    ///   with no password. Atomic writes make this case a real anomaly, not the
    ///   normal torn-write it used to be.
    fn load_auth(dir: &Path, id: Uuid) -> Result<Option<String>, String> {
        let path = dir.join(format!("{id}.auth.json"));
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("read {}: {e}", path.display())),
        };
        let parsed: PersistedAuth =
            serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))?;
        Ok(parsed.password_hash)
    }

    /// Write the `{id}.auth.json` sidecar atomically (temp file + rename), the
    /// same durability the tournament file gets in [`TournamentStore::persist`]
    /// — a half-written sidecar would reload as an *unreadable* sidecar, which
    /// [`load_auth`](Self::load_auth) now (correctly) refuses to serve open.
    /// Best-effort otherwise: a failure is logged, not propagated.
    fn persist_auth(&self, id: Uuid, auth: &AuthConfig) {
        let Some(path) = self.auth_path(id) else {
            return; // in-memory only
        };
        if let Some(dir) = &self.data_dir {
            let _ = fs::create_dir_all(dir);
        }
        let file = PersistedAuth {
            password_hash: Some(auth.password_hash().to_string()),
        };
        let bytes = match serde_json::to_vec_pretty(&file) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!("could not serialize auth for {id}: {e}");
                return;
            }
        };
        // `{id}.auth.json` → `{id}.auth.tmp` (extension `tmp`, so the load-all
        // scan skips it, same as the tournament file's temp).
        let tmp = path.with_extension("tmp");
        if let Err(e) = fs::write(&tmp, &bytes) {
            tracing::warn!("could not write {}: {e}", tmp.display());
            return;
        }
        if let Err(e) = fs::rename(&tmp, &path) {
            tracing::warn!("could not replace {}: {e}", path.display());
        }
    }

    /// List every known tournament (id, name, whether it's password-protected)
    /// — enough for the picker, never the tournament's contents.
    pub fn list(&self) -> Vec<TournamentSummary> {
        let instances = self.instances.read().expect("registry lock poisoned");
        instances
            .iter()
            .filter_map(|(id, instance)| {
                // `instance.read()` recovers from a poisoned store lock (see its
                // doc): one tournament whose mutation panicked must not take the
                // whole picker down for every other tournament.
                let store = instance.read();
                let tournament = store.current()?;
                Some(TournamentSummary {
                    id: *id,
                    name: tournament.name.clone(),
                    has_password: instance.auth.is_some(),
                })
            })
            .collect()
    }

    /// Look up one tournament's instance by id.
    pub fn get(&self, id: Uuid) -> Option<Arc<TournamentInstance>> {
        self.instances
            .read()
            .expect("registry lock poisoned")
            .get(&id)
            .cloned()
    }

    /// Create a new, empty tournament named `name`, optionally protected by
    /// `password`. Persists it (tournament + auth sidecar) if a data directory
    /// is configured. Returns the new tournament's id and, when it has a
    /// password, its session token — read from the freshly built `AuthConfig`,
    /// so the caller needn't look the instance back up (and can't race a
    /// concurrent delete doing so).
    pub fn create(
        &self,
        name: &str,
        password: Option<String>,
    ) -> Result<(Uuid, Option<String>), TournamentError> {
        let tournament = Tournament::new(name)?;
        let id = tournament.id;
        if let Some(dir) = &self.data_dir {
            let _ = fs::create_dir_all(dir);
        }
        let auth = password.map(AuthConfig::new);
        let token = auth.as_ref().map(|a| a.token().to_string());
        // Write the auth sidecar *before* the tournament file, so a crash between
        // the two writes can only ever leave the sidecar orphaned (harmless: no
        // tournament file, so nothing loads), never a tournament file with its
        // sidecar missing (which would reload as open). See [`load_auth`].
        if let Some(auth) = &auth {
            self.persist_auth(id, auth);
        }
        let mut store = TournamentStore::empty(self.tournament_path(id));
        store.set_current(tournament);
        self.instances
            .write()
            .expect("registry lock poisoned")
            .insert(
                id,
                Arc::new(TournamentInstance {
                    store: RwLock::new(store),
                    auth,
                }),
            );
        Ok((id, token))
    }

    /// Remove a tournament: its registry entry, its persisted file (+ auth
    /// sidecar), and its backups directory. Returns whether it existed.
    pub fn remove(&self, id: Uuid) -> bool {
        let removed = self
            .instances
            .write()
            .expect("registry lock poisoned")
            .remove(&id);
        if let Some(instance) = &removed {
            // Tombstone the store *under its write lock* before deleting the
            // files. A concurrent request may already hold an `Arc` to this same
            // instance and be mid-mutation; serializing on the store lock and
            // flipping `deleted` guarantees any such write either ran before this
            // point (and is now deleted) or sees the tombstone and refuses to
            // persist — so a late write can't resurrect the file on disk.
            instance.write().mark_deleted();
            if let Some(path) = self.tournament_path(id) {
                let _ = fs::remove_file(path);
            }
            if let Some(path) = self.auth_path(id) {
                let _ = fs::remove_file(path);
            }
            crate::backup::delete_all(id);
        }
        removed.is_some()
    }
}

/// State shared across all requests.
///
/// The registry and ratings cache are `RwLock`-guarded and `Arc`-shared so
/// axum can clone the state cheaply per request.
#[derive(Clone, Default)]
pub struct AppState {
    pub registry: Arc<TournamentRegistry>,
    /// Cached FESA rating list (see [`crate::ratings`]). Global, not
    /// per-tournament — it's a shared external list.
    pub ratings: Arc<RwLock<Option<CachedRatings>>>,
    /// Gates `POST /api/tournaments` (creating new tournaments) — separate
    /// from any individual tournament's own password. `None` = anyone can
    /// create (fine for local/embedded/dev; set this on a publicly reachable
    /// host to stop creation-spam once the URL inevitably circulates).
    pub admin_auth: Option<AuthConfig>,
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
                .mutate(None, |t| {
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
        let mut store = TournamentStore::empty(None);
        assert_eq!(store.version(), 0);

        let mut rx = store.subscribe();
        store.set_current(Tournament::new("Cup").unwrap());

        // The version advanced and the new value was broadcast.
        assert_eq!(store.version(), 1);
        assert_eq!(rx.recv().await.unwrap(), 1);

        store
            .mutate(None, |t| {
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
            .mutate(None, |t| {
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

    #[test]
    fn registry_creates_lists_and_isolates_tournaments() {
        let registry = TournamentRegistry::default();
        let (a, _) = registry.create("Cup A", None).unwrap();
        let (b, _) = registry.create("Cup B", Some("secret".into())).unwrap();

        let mut names: Vec<_> = registry
            .list()
            .into_iter()
            .map(|s| (s.name, s.has_password))
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![("Cup A".to_string(), false), ("Cup B".to_string(), true)]
        );

        // Mutating one doesn't touch the other.
        registry
            .get(a)
            .unwrap()
            .store
            .write()
            .unwrap()
            .mutate(None, |t| {
                t.add_player(NewPlayer {
                    last_name: "Alice".into(),
                    ..Default::default()
                })
                .map(|_| ())
            })
            .unwrap();
        assert_eq!(
            registry
                .get(a)
                .unwrap()
                .store
                .read()
                .unwrap()
                .current()
                .unwrap()
                .players
                .len(),
            1
        );
        assert!(registry
            .get(b)
            .unwrap()
            .store
            .read()
            .unwrap()
            .current()
            .unwrap()
            .players
            .is_empty());

        assert!(registry.remove(a));
        assert!(registry.get(a).is_none());
        assert!(registry.get(b).is_some());
        assert!(!registry.remove(a)); // already gone
    }

    #[test]
    fn registry_persists_tournament_and_password_hash_across_reload() {
        let dir = std::env::temp_dir().join(format!("osp-registry-{}", uuid::Uuid::new_v4()));

        let id = {
            let registry = TournamentRegistry::new(Some(dir.clone()));
            registry
                .create("Persisted Cup", Some("secret".into()))
                .unwrap()
                .0
        };

        let reloaded = TournamentRegistry::new(Some(dir.clone()));
        let instance = reloaded.get(id).expect("reloaded from disk");
        assert_eq!(
            instance.store.read().unwrap().current().unwrap().name,
            "Persisted Cup"
        );
        let auth = instance.auth.as_ref().expect("password was persisted");
        assert!(auth.password_matches("secret"));
        assert!(!auth.password_matches("wrong"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_auth_sidecar_fails_closed_not_open() {
        let dir = std::env::temp_dir().join(format!("osp-authfail-{}", uuid::Uuid::new_v4()));

        let id = {
            let registry = TournamentRegistry::new(Some(dir.clone()));
            registry
                .create("Locked Cup", Some("secret".into()))
                .unwrap()
                .0
        };

        // Corrupt the sidecar (a torn/garbled write). The tournament file itself
        // is intact and says nothing about being locked.
        std::fs::write(dir.join(format!("{id}.auth.json")), b"{ this is not json")
            .expect("overwrite the sidecar");

        // The tournament must NOT come back open (which would drop its password);
        // it is dropped from the registry until an admin fixes the sidecar.
        let reloaded = TournamentRegistry::new(Some(dir.clone()));
        assert!(
            reloaded.get(id).is_none(),
            "a tournament with an unreadable auth sidecar must fail closed, not load open"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_poisoned_store_lock_still_serves_later_requests() {
        let instance = Arc::new(TournamentInstance {
            store: RwLock::new({
                let mut s = TournamentStore::empty(None);
                s.set_current(Tournament::new("Cup").unwrap());
                s
            }),
            auth: None,
        });

        // Poison the store lock: panic while holding its write guard, exactly the
        // shape of an osp-core panic inside a mutation. (The panic message on
        // stderr during this test is expected.)
        let poisoner = Arc::clone(&instance);
        let joined = std::thread::spawn(move || {
            let _guard = poisoner.store.write().unwrap();
            panic!("boom while holding the store write lock");
        })
        .join();
        assert!(joined.is_err(), "the spawned thread panicked as intended");
        assert!(
            instance.store.read().is_err(),
            "the store lock is now poisoned"
        );

        // The raw `.expect(...)` the handlers used to do would panic here, 500ing
        // every future request. The recovering accessors instead hand back the
        // (still-consistent) state and keep serving.
        assert_eq!(instance.read().current().unwrap().name, "Cup");
        instance
            .write()
            .set_current(Tournament::new("Cup 2").unwrap());
        assert_eq!(instance.read().current().unwrap().name, "Cup 2");
    }

    fn add_named(name: &str) -> impl FnOnce(&mut Tournament) -> Result<(), TournamentError> + '_ {
        move |t| {
            t.add_player(NewPlayer {
                last_name: name.into(),
                ..Default::default()
            })
            .map(|_| ())
        }
    }

    #[test]
    fn mutate_with_the_wrong_expected_version_is_a_conflict_and_a_no_op() {
        let mut store = TournamentStore::empty(None);
        store.set_current(Tournament::new("Cup").unwrap()); // version → 1

        // An edit based on a stale version is rejected atomically, changing nothing.
        assert!(matches!(
            store.mutate(Some(0), add_named("Alice")),
            Err(MutateError::VersionConflict)
        ));
        assert_eq!(store.version(), 1);
        assert!(store.current().unwrap().players.is_empty());

        // Based on the current version it goes through and advances the version.
        store.mutate(Some(1), add_named("Alice")).unwrap();
        assert_eq!(store.version(), 2);
        assert_eq!(store.current().unwrap().players.len(), 1);

        // `None` opts out of the check entirely (the desktop/local path).
        store.mutate(None, add_named("Bob")).unwrap();
        assert_eq!(store.current().unwrap().players.len(), 2);
    }

    #[test]
    fn a_tombstoned_store_refuses_mutations_and_never_rewrites_its_file() {
        let path =
            std::env::temp_dir().join(format!("osp-tombstone-{}.json", uuid::Uuid::new_v4()));
        let mut store = TournamentStore::with_persistence(path.clone());
        store.set_current(Tournament::new("Cup").unwrap());
        assert!(path.exists());

        // Simulate the tombstone `remove` sets under the store's write lock, then
        // delete the file as `remove` would.
        store.mark_deleted();
        std::fs::remove_file(&path).unwrap();

        // A mutation still in flight against this store now refuses rather than
        // recreating the deleted file — the delete/edit resurrection is closed.
        assert!(matches!(
            store.mutate(None, add_named("Alice")),
            Err(MutateError::NoTournament)
        ));
        assert!(
            !path.exists(),
            "a tombstoned store must not rewrite its file"
        );
    }

    #[test]
    fn removing_a_tournament_tombstones_its_store() {
        let registry = TournamentRegistry::default();
        let (id, _) = registry.create("Cup", None).unwrap();
        let instance = registry.get(id).expect("just created");

        assert!(registry.remove(id));
        // A request holding a stale `Arc` sees the tombstone and can't mutate.
        assert!(matches!(
            instance
                .store
                .write()
                .unwrap()
                .mutate(None, add_named("Alice")),
            Err(MutateError::NoTournament)
        ));
        assert!(instance.store.read().unwrap().is_deleted());
    }

    #[test]
    fn check_format_version_rejects_old_and_accepts_current() {
        // A current serialization round-trips through the version check.
        let current = serde_json::to_vec(&Tournament::new("Cup").unwrap()).unwrap();
        assert!(check_format_version(&current).is_ok());

        // A present-but-wrong version (the v1.0.0 files were format 4) is rejected
        // with the clear version error — this is the case a full deserialize would
        // instead fail on with an opaque field-level error.
        let old = br#"{"format_version":4,"handicap_policy":"allowed"}"#;
        assert!(matches!(
            check_format_version(old),
            Err(TournamentError::UnsupportedFormatVersion { found: 4, .. })
        ));

        // A missing version means "current" (matches the field's serde default).
        assert!(check_format_version(b"{}").is_ok());

        // Bytes that aren't even JSON are a malformed save, not a version error.
        assert!(matches!(
            check_format_version(b"not json"),
            Err(TournamentError::MalformedSave(_))
        ));
    }

    #[test]
    fn ensure_current_version_closes_the_replace_undo_race() {
        // Model the race the middleware pre-screen can't: two writers both read
        // version N, then serialize on the write lock. The first commits (→ N+1);
        // the second's version-unchecked mutation (load/restore/undo) must now be
        // rejected here rather than silently clobber the first.
        let mut store = TournamentStore::empty(None);
        store.set_current(Tournament::new("Cup").unwrap());
        let base = store.version(); // what both writers saw

        // Writer A commits first.
        store
            .mutate(Some(base), add_named("Alice"))
            .expect("A is based on the current version");
        assert_eq!(store.version(), base + 1);

        // Writer B's load/restore/undo, still based on `base`, is now stale.
        assert!(matches!(
            store.ensure_current_version(Some(base)),
            Err(MutateError::VersionConflict)
        ));
        // A client that opts out of the check (no header) still passes.
        assert!(store.ensure_current_version(None).is_ok());
        // And a writer based on the now-current version is accepted.
        assert!(store.ensure_current_version(Some(store.version())).is_ok());
    }
}
