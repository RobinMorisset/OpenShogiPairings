//! Shared server state: a registry of tournament instances.
//!
//! See `docs/archive/multi-tournament.md`. One process can now hold several
//! tournaments; [`TournamentRegistry`] is the map from each tournament's own
//! id (already a stable `Uuid` on [`Tournament`]) to its
//! [`TournamentInstance`] (live state + its own optional password). Handlers
//! resolve the instance for a request via [`crate::scope::TournamentCtx`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use axum::body::Bytes;
use constant_time_eq::constant_time_eq;
use osp_core::{Tournament, TournamentDates, TournamentError};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use ts_rs::TS;
use uuid::Uuid;

use crate::auth::AuthConfig;
use crate::history::ChangeLabel;
use crate::ratings::CachedRatings;

/// Capacity of the change-notification channel. Small on purpose: subscribers
/// only care about the *latest* version, and one that lags past the buffer is
/// simply told to resync (see [`crate::live`]).
const NOTIFY_CAPACITY: usize = 16;

/// Why a tournament file on disk could not be loaded, kept so the tournament
/// can be *listed* rather than silently vanish.
///
/// A save from an older format version is the ordinary case: the referee
/// upgraded, and the tournament they ran last month is now unreadable. Hiding it
/// and logging an error where nobody looks is how a tournament quietly stops
/// existing — worse, its file and backups then sit on disk forever, since the
/// only thing that deletes them is a picker entry to click.
pub(crate) struct LoadProblem {
    /// What is wrong, in the same vocabulary as any other rejected save — so the
    /// picker can say "format 8, this build reads 9" in the referee's language.
    pub error: TournamentError,
    /// The tournament's name, probed out of the file even though the file as a
    /// whole doesn't parse: `name` has been a plain top-level string in every
    /// format version, and "Nancy Open 2026 can't be opened" beats a uuid.
    pub name: Option<String>,
}

impl LoadProblem {
    fn new(error: TournamentError, bytes: &[u8]) -> Self {
        #[derive(Deserialize)]
        struct NameProbe {
            name: Option<String>,
        }
        let name = serde_json::from_slice::<NameProbe>(bytes)
            .ok()
            .and_then(|probe| probe.name)
            .filter(|name| !name.trim().is_empty());
        Self { error, name }
    }

    /// What to call this tournament in the picker: its own name when the probe
    /// found one, else its id — which is the filename, and so still enough to
    /// go and look at what is wrong.
    fn display_name(&self, id: Uuid) -> String {
        self.name.clone().unwrap_or_else(|| id.to_string())
    }
}

/// One step of the undo history: a tournament to go back (or forward) to, and
/// the change that separates it from the current one.
///
/// The same shape serves both stacks, which is what makes undo and redo exact
/// mirrors of each other: on the undo stack `snapshot` is the state *before*
/// the change, on the redo stack the state *after* it, and `label` names that
/// change either way.
///
/// The label is carried rather than derived, because by the time anyone asks,
/// the change has been folded into the tournament and the two snapshots on
/// either side of it no longer say which edit produced the difference — see
/// [`crate::history`].
struct HistoryEntry {
    snapshot: Tournament,
    label: ChangeLabel,
}

/// The current tournament plus a linear undo history.
///
/// The history is a stack of full tournament snapshots taken *before* each
/// player mutation; undo pops the most recent one onto a second stack, and redo
/// moves it back. Snapshots are cheap (a tournament is just a list of players),
/// which is why we snapshot rather than track inverse commands. Loading a
/// tournament resets both stacks, so undo is scoped to edits made since.
///
/// A new mutation **discards** the redo stack, the standard rule: once history
/// forks, the abandoned branch is no longer reachable by any sequence of undos,
/// and offering to jump to it would splice in a state that never followed from
/// what is on screen.
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
    history: Vec<HistoryEntry>,
    /// The changes undone since the last mutation, newest last — what redo
    /// walks back up. Emptied by [`mutate`](Self::mutate) and
    /// [`set_current`](Self::set_current).
    redone: Vec<HistoryEntry>,
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
    /// Why `current` is `None`, when the reason is a save file this build cannot
    /// read (see [`LoadProblem`]). Such a store is inert: `persist` needs a
    /// `current` to write, so the unreadable file is never overwritten.
    problem: Option<LoadProblem>,
    /// Whether the last [`persist`](Self::persist) attempt failed — i.e. the
    /// state answered to clients exists only in this process's memory. Rides out
    /// on [`crate::tournament::TournamentView`] so the referee sees a banner:
    /// the write is deliberately non-blocking (an edit must not be refused
    /// because the disk is full), but "your edit is saved" is then a lie, and on
    /// the desktop app there is no log to find it in.
    persist_failed: bool,
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
            redone: Vec::new(),
            persist_path,
            version: 0,
            notifier,
            deleted: false,
            problem: None,
            persist_failed: false,
        }
    }

    /// Build a store backed by `path`: load any tournament already saved there,
    /// and write the current one through to it on every change.
    ///
    /// A file that exists but cannot be read leaves the store empty *and*
    /// [`problem`](Self::problem)ed, which is what lets the registry keep the
    /// tournament visible — see [`LoadProblem`].
    pub fn with_persistence(path: PathBuf) -> Self {
        let (current, problem) = match Self::load_from_disk(&path) {
            Ok(current) => (current, None),
            Err(problem) => (None, Some(problem)),
        };
        if current.is_some() {
            tracing::info!("loaded the saved tournament from {}", path.display());
        }
        Self {
            current,
            problem,
            ..Self::empty(Some(path))
        }
    }

    /// Why this store is empty, when it is empty because its file could not be
    /// read. `None` both for a loaded tournament and for one that never had a
    /// file — only a *broken* save sets this.
    pub(crate) fn problem(&self) -> Option<&LoadProblem> {
        self.problem.as_ref()
    }

    /// The current change version. Bumped on every state change.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Subscribe to change notifications: each message is the new
    /// [`version`](Self::version).
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

    /// Re-send the *current* version, without advancing it: nothing about the
    /// tournament changed, but something about who may see it did.
    ///
    /// This is what makes revoking a public link take effect at once (see
    /// [`TournamentRegistry::set_publication`]). A public reader's stream
    /// re-checks its capability key on every notification, so waking it is the
    /// whole mechanism — without this, a rotated key would only take hold at
    /// the *next* edit, and a revoked reader would keep receiving results in
    /// the meantime.
    ///
    /// Harmless to the referee streams: their clients refetch only when the
    /// version they are told about is *newer* than the one they hold, so a
    /// repeat of the current one is ignored.
    fn ping(&self) {
        let _ = self.notifier.send(self.version);
    }

    /// Read and parse the persisted tournament: `Ok(None)` if the file is absent
    /// (or unreadable at the filesystem level), `Err` if it is there but this
    /// build cannot make a tournament of it.
    ///
    /// A file that is present but unreadable — a wrong/old format version,
    /// corrupt bytes, or a record that breaks an invariant the rest of the
    /// program relies on — is reported loudly at `error` level and left
    /// **untouched**, and the reason comes back so the tournament can still be
    /// listed (see [`LoadProblem`]). This is deliberate: an old file from before
    /// an incompatible format bump must be rejected, not silently discarded —
    /// losing an in-progress event on a routine binary upgrade would be far
    /// worse than refusing to serve it.
    fn load_from_disk(path: &Path) -> Result<Option<Tournament>, LoadProblem> {
        let bytes = match fs::read(path) {
            // Genuinely absent is the normal empty-start case: no file, no
            // tournament, nothing to tell anyone about.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            // Present but unreadable (permissions, a bad sector, a disappearing
            // network mount) is a *save* problem like any other. The registry
            // only asks about a file it has just seen in the directory listing,
            // so silently answering "no tournament here" would drop an event the
            // referee can see on disk from the picker they use to find it.
            Err(e) => {
                tracing::error!(
                    "could not read {}: {e}. The file is left untouched.",
                    path.display()
                );
                return Err(LoadProblem::new(
                    TournamentError::MalformedSave(format!("could not read the file: {e}")),
                    &[],
                ));
            }
            Ok(bytes) => bytes,
        };
        // A file off the disk passes exactly the gate an imported one does — the
        // version window, the parse, and the invariants (see [`crate::save`]).
        match crate::save::load(&bytes) {
            Ok(tournament) => Ok(Some(tournament)),
            Err(error) => {
                tracing::error!(
                    "refusing to load {}: {error}. The file is left untouched — load it \
                     with a matching build, or delete that tournament from the picker.",
                    path.display()
                );
                Err(LoadProblem::new(error, &bytes))
            }
        }
    }

    /// Write the current tournament through to disk, if a path is configured.
    ///
    /// Atomic (temp file + rename) and non-blocking: a persistence failure is
    /// logged, never propagated — it must not break the referee's action, just
    /// as backups don't (see [`crate::backup`]).
    ///
    /// It is *recorded*, though, in [`persist_failed`](Self::persist_failed):
    /// answering `200` to an edit that only ever reached memory, with nothing but
    /// a log line to say so, is how a full disk turns into a tournament that
    /// vanishes at the next restart — and the desktop app has no log the referee
    /// can read. The flag rides out on the view so the UI can say it.
    fn persist(&mut self) {
        // A tournament removed from the registry must never be written back —
        // otherwise a mutation still in flight when it was deleted would recreate
        // its file after `remove` deleted it (see [`TournamentRegistry::remove`]).
        if self.deleted {
            return;
        }
        let (Some(path), Some(current)) = (&self.persist_path, &self.current) else {
            return;
        };
        let outcome = Self::write_through(path, current);
        if let Err(e) = &outcome {
            tracing::warn!("persist: {e}");
        } else if self.persist_failed {
            tracing::info!("persist: writing to disk works again");
        }
        self.persist_failed = outcome.is_err();
    }

    /// The write itself: serialize, write a temp file, rename it over the target.
    fn write_through(path: &Path, current: &Tournament) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(current)
            .map_err(|e| format!("could not serialize the tournament: {e}"))?;
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, &bytes).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
        // `rename` replaces the destination atomically on both Unix and Windows.
        fs::rename(&tmp, path).map_err(|e| format!("could not replace {}: {e}", path.display()))
    }

    /// Whether the state this store answers with is safely on disk.
    ///
    /// `true` both for a healthy write-through store and for one with no file at
    /// all (the embedded desktop app, dev, tests): there is nothing to warn about
    /// where persistence was never the promise.
    pub(crate) fn persisted(&self) -> bool {
        !self.persist_failed
    }

    /// The current tournament, if one exists.
    pub fn current(&self) -> Option<&Tournament> {
        self.current.as_ref()
    }

    /// What an undo would revert, or `None` when there is nothing to undo.
    ///
    /// Doubles as the "is the button live" flag it replaced: a caller that only
    /// wants to know whether an undo is possible asks `is_some()`. Keeping the
    /// two as one field is deliberate — a separate boolean could disagree with
    /// the label beside it, and there is no state in which it should.
    pub(crate) fn undo_label(&self) -> Option<&ChangeLabel> {
        self.history.last().map(|entry| &entry.label)
    }

    /// What a redo would re-apply, or `None` when there is nothing to redo — on
    /// the same terms as [`undo_label`](Self::undo_label).
    pub(crate) fn redo_label(&self) -> Option<&ChangeLabel> {
        self.redone.last().map(|entry| &entry.label)
    }

    /// Set the current tournament and clear both history stacks.
    ///
    /// Used by create and load — these establish a new baseline, so prior
    /// history no longer applies. Clearing the redo stack here is not
    /// housekeeping: its snapshots carry the *previous* tournament's id, so a
    /// redo against a stale one would swap in a different tournament entirely.
    pub fn set_current(&mut self, tournament: Tournament) {
        self.current = Some(tournament);
        self.history.clear();
        self.redone.clear();
        self.persist();
        self.bump_and_notify();
    }

    /// Apply a mutation to the current tournament, checkpointing first.
    ///
    /// The mutation runs against a clone; only if it succeeds do we snapshot the
    /// previous state onto the history and swap in the new one. A failed
    /// mutation (e.g. validation error) leaves state and history untouched.
    ///
    /// `label` says what the change was, for the undo button to name (see
    /// [`crate::history`]). It is built by the caller *before* the mutation runs,
    /// since a label naming a player or a team has to be read from the state
    /// the change is about to replace — the very thing that is gone afterwards.
    ///
    /// `expected` is the tournament version the client's edit was based on, or
    /// `None` to skip the check. Comparing it here — under the same write lock the
    /// mutation runs in — is what makes optimistic-concurrency detection actually
    /// atomic: the [`crate::live::check_version`] middleware only pre-screens (it
    /// reads the version and releases the lock before the handler runs), so two
    /// edits racing from the same base version must be separated *here*, or the
    /// later one would silently clobber the earlier one with no `409`.
    pub(crate) fn mutate<F>(
        &mut self,
        expected: Option<u32>,
        label: ChangeLabel,
        f: F,
    ) -> Result<(), MutateError>
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
        let snapshot = self.current.replace(next).expect("current was Some");
        self.history.push(HistoryEntry { snapshot, label });
        // History has forked: whatever was undone is no longer reachable.
        self.redone.clear();
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
    /// `undo`) that don't go through [`mutate`](Self::mutate)'s built-in check.
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

    /// Revert to the previous state, putting the change on the redo stack.
    ///
    /// An empty history is an error rather than a silent no-op. Everything that
    /// empties it — an undo, a load, a restored backup — bumps the version, so a
    /// caller that passed [`ensure_current_version`](Self::ensure_current_version)
    /// cannot legitimately arrive here: reaching it means the request was built
    /// from a view that has moved, and answering as though an undo had happened
    /// would report a change that never took place.
    pub fn undo(&mut self) -> Result<(), NothingLeft> {
        self.step(Direction::Undo)
    }

    /// Re-apply the change the last undo took back, putting it back on the undo
    /// stack. The exact mirror of [`undo`](Self::undo), including its refusal to
    /// pretend when there is nothing there.
    pub fn redo(&mut self) -> Result<(), NothingLeft> {
        self.step(Direction::Redo)
    }

    /// Move one step along the history, in either direction.
    ///
    /// Written once rather than twice because the two are genuinely the same
    /// operation: pop the far stack, swap its snapshot with the current
    /// tournament, and push what was current onto the near stack — carrying the
    /// label along, since it names the change between the two states no matter
    /// which side you approach it from.
    fn step(&mut self, direction: Direction) -> Result<(), NothingLeft> {
        let (from, to) = match direction {
            Direction::Undo => (&mut self.history, &mut self.redone),
            Direction::Redo => (&mut self.redone, &mut self.history),
        };
        let entry = from.pop().ok_or(NothingLeft(direction))?;
        let current = self
            .current
            .replace(entry.snapshot)
            .expect("a store with history has a current tournament");
        to.push(HistoryEntry {
            snapshot: current,
            label: entry.label,
        });
        self.persist();
        self.bump_and_notify();
        Ok(())
    }
}

/// Which way along the history a step goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Undo,
    Redo,
}

impl Direction {
    /// The verb, for the message a refusal carries.
    fn verb(self) -> &'static str {
        match self {
            Direction::Undo => "undo",
            Direction::Redo => "redo",
        }
    }
}

/// [`TournamentStore::undo`] or [`redo`](TournamentStore::redo) was asked to
/// move through a history that has nothing left in that direction.
#[derive(Debug, Clone, Copy)]
pub struct NothingLeft(pub Direction);

impl std::fmt::Display for NothingLeft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "there is nothing left to {}", self.0.verb())
    }
}

impl std::error::Error for NothingLeft {}

/// No tournament with this id is registered.
#[derive(Debug, Clone, Copy)]
pub struct NoSuchTournament(pub Uuid);

impl std::fmt::Display for NoSuchTournament {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no tournament {}", self.0)
    }
}

impl std::error::Error for NoSuchTournament {}

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
///
/// `publication` is the other half of the same story, pointing the other way:
/// the capability key of the **public reader page** (see [`crate::public`]),
/// `None` — the default — when the tournament is not published at all.
pub struct TournamentInstance {
    pub store: RwLock<TournamentStore>,
    pub auth: Option<AuthConfig>,
    /// The public reader page's capability key, or `None` when this tournament
    /// is not published. Unlike `auth` this changes at runtime (the referee
    /// publishes, rotates the key, or unpublishes mid-tournament), so it lives
    /// behind its own lock; it is persisted in the same `{id}.auth.json`
    /// sidecar, since it is access-control state and must not travel inside a
    /// save file a referee mails around.
    publication: RwLock<Option<String>>,
    /// The public projection, serialized, together with the store version it
    /// was built from — so one mutation costs one serialization rather than one
    /// per reader (see `docs/archive/public-access.md` §4). Invalidated by version
    /// mismatch, never explicitly.
    public_payload: Mutex<Option<(u32, Bytes)>>,
    /// How many public SSE streams are currently open on this tournament. Not a
    /// scaling measure — a room of a hundred phones is fine — but an abuse one:
    /// the referee stream was implicitly bounded by knowing the password, and
    /// this one is not bounded by anything at all.
    public_streams: AtomicUsize,
    /// Where this tournament's automatic backups are written (see
    /// [`crate::backup`]) — its own directory under the registry's backups
    /// root. `None` when no root could be resolved at all, which keeps no
    /// backups rather than inventing a location. Carried per instance so a
    /// handler that has resolved the tournament (and only it) can back up,
    /// list and report the directory without reaching back for the registry.
    pub backups_dir: Option<PathBuf>,
}

/// Holds one public SSE stream's slot against [`TournamentInstance::public_streams`],
/// releasing it when the stream is dropped (client disconnect, server shutdown).
pub(crate) struct PublicStreamSlot(Arc<TournamentInstance>);

impl Drop for PublicStreamSlot {
    fn drop(&mut self) {
        self.0.public_streams.fetch_sub(1, Ordering::Relaxed);
    }
}

impl TournamentInstance {
    /// Assemble an instance around an already-built store.
    fn new(
        store: TournamentStore,
        auth: Option<AuthConfig>,
        public_key: Option<String>,
        backups_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            store: RwLock::new(store),
            auth,
            publication: RwLock::new(public_key),
            public_payload: Mutex::new(None),
            public_streams: AtomicUsize::new(0),
            backups_dir,
        }
    }

    /// This tournament's public capability key, or `None` when it is not
    /// published. Only ever handed to an authenticated referee — it is what the
    /// QR code on the wall encodes.
    pub fn public_key(&self) -> Option<String> {
        self.publication
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Whether `presented` is this tournament's public capability key. Compared
    /// in constant time, like the session token: it is a bearer credential, and
    /// the fact that it grants only reading doesn't make leaking it through
    /// timing any more appealing. Always false when unpublished, so an
    /// unpublished tournament cannot be reached by guessing.
    pub(crate) fn public_key_matches(&self, presented: &str) -> bool {
        match self.public_key() {
            Some(key) => constant_time_eq(presented.as_bytes(), key.as_bytes()),
            None => false,
        }
    }

    /// Publish (with a freshly minted key), rotate the key, or unpublish
    /// (`None`). Does *not* persist — go through
    /// [`TournamentRegistry::set_publication`], which owns the sidecar.
    fn set_public_key(&self, key: Option<String>) {
        *self.publication.write().unwrap_or_else(|e| e.into_inner()) = key;
    }

    /// The cached public payload if it was built from `version`, else `None`.
    pub(crate) fn cached_public_payload(&self, version: u32) -> Option<Bytes> {
        let cache = self
            .public_payload
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match &*cache {
            Some((cached, payload)) if *cached == version => Some(payload.clone()),
            _ => None,
        }
    }

    /// Remember `payload` as the public projection of `version`.
    pub(crate) fn cache_public_payload(&self, version: u32, payload: Bytes) {
        *self
            .public_payload
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some((version, payload));
    }

    /// Claim a slot for one public SSE stream, or `None` when `cap` are already
    /// open. The returned guard releases the slot when dropped.
    pub(crate) fn open_public_stream(self: &Arc<Self>, cap: usize) -> Option<PublicStreamSlot> {
        if self.public_streams.fetch_add(1, Ordering::Relaxed) >= cap {
            self.public_streams.fetch_sub(1, Ordering::Relaxed);
            return None;
        }
        Some(PublicStreamSlot(Arc::clone(self)))
    }

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
    /// 500 until the process restarts. The registry's own lock, over the map of
    /// instances, takes the other line: [`list`] and [`remove`] `.expect()` on
    /// it, since a poisoned *registry* means a panic ran while the map itself was
    /// being reshaped, and there is no argument that it is still whole.
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

/// A domain error carried outside an error *response*: what is wrong with a
/// tournament the picker lists but cannot open (see
/// [`TournamentSummary`]).
///
/// Same `code` + `values` contract as [`crate::error::ApiError`]'s own body, and the client localizes it
/// through the same table — a broken save has to explain itself in the
/// referee's language just as much as a failed request does. The English
/// `message` rides along as the fallback for a client that doesn't know the
/// code, exactly as in an error body.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct TournamentProblem {
    /// Stable machine code, e.g. `unsupported_format_version`. Absent for the
    /// errors that stay English (see [`crate::error::domain_payload`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    /// Interpolation values for the translated message.
    #[ts(type = "Record<string, string>")]
    pub values: BTreeMap<String, String>,
    /// The server's own English wording, for a client with no translation.
    pub message: String,
}

impl TournamentProblem {
    /// A problem with no machine code: shown in English, deliberately, because
    /// it can only arise from something being broken (files disappearing from
    /// under the server) rather than from anything the referee did. Dressing
    /// that up as ordinary translated UI would hide what it is.
    pub(crate) fn plain(message: String) -> Self {
        Self {
            code: None,
            values: BTreeMap::new(),
            message,
        }
    }
}

impl From<&TournamentError> for TournamentProblem {
    fn from(err: &TournamentError) -> Self {
        let (code, values) = match crate::error::domain_payload(err) {
            Some((code, values)) => (Some(code), values),
            None => (None, BTreeMap::new()),
        };
        Self {
            code,
            values,
            message: err.to_string(),
        }
    }
}

/// Summary of a tournament for the picker list (`GET /api/tournaments`) — its
/// name, where and when it is held, and whether it's locked, never the
/// tournament's contents.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub struct TournamentSummary {
    pub id: Uuid,
    pub name: String,
    pub has_password: bool,
    /// Where the event is held and when, straight from its settings — each
    /// absent when the referee never entered it, and all three absent for a
    /// tournament that cannot be read at all (see [`Self::problem`]).
    ///
    /// Not contents: they are the three lines a referee facing a server with
    /// four tournaments called "Open" needs to tell them apart, which is the
    /// job the picker's list has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dates: Option<TournamentDates>,
    /// Why this one cannot be opened — a save from a format version this build
    /// no longer reads, most often. `None` is the normal case.
    ///
    /// It is listed *because* it is broken, not in spite of it: a tournament
    /// that vanishes from the picker looks deleted, and the referee has no way
    /// to get rid of its files either. So it appears, greyed out, explaining
    /// itself, with a delete button that works.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<TournamentProblem>,
}

/// What's persisted alongside a tournament file for the bits that live outside
/// `osp-core`: the password hash, and the public reader page's capability key.
/// Kept as its own small sidecar file (`{id}.auth.json`) rather than folded
/// into `TournamentStore`'s persistence, so the store stays ignorant of auth,
/// same as before this feature — and so neither travels inside a save file the
/// referee mails around.
#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PersistedAuth {
    #[serde(default)]
    password_hash: Option<String>,
    /// The public reader page's capability key; absent = not published, which
    /// is the default and what every sidecar written before publication existed
    /// says.
    #[serde(default)]
    public_key: Option<String>,
}

/// Mint a fresh public capability key: 192 random bits, hex-encoded.
///
/// Not a security boundary against a determined attacker — the link is meant to
/// be photographed and forwarded — but against *accidental* discovery, which is
/// the actual risk: an abandoned tournament must not sit in a search engine with
/// forty people's names in it. Rotating it revokes every link that was handed
/// out, independently of the tournament password.
fn mint_public_key() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
    /// Root holding one directory of automatic backups per tournament (see
    /// [`crate::backup`]). Independent of `data_dir`: backups are a recovery
    /// store, and a referee may well want them on a different disk from the
    /// live tournaments. `None` = keep no backups at all.
    backups_root: Option<PathBuf>,
    /// How long the backups of a *deleted* tournament are kept before
    /// [`crate::backup::sweep`] removes them (see [`remove`](Self::remove)).
    deleted_retention: Duration,
}

impl Default for TournamentRegistry {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl TournamentRegistry {
    /// Build a registry, loading every tournament already saved under
    /// `data_dir` (eager load-all: simplest, and fine at the scale one host
    /// actually runs — a handful of tournaments). A file that fails to parse is
    /// skipped with a warning, same as a single corrupt `OSP_DATA_FILE` used to
    /// be.
    ///
    /// `backups_root` says where the automatic backups go; `None` keeps none at
    /// all. It is *not* resolved to a default here — a registry given no root
    /// writes nowhere, so that constructing one (as every test does) can never
    /// scatter files through the developer's real data directory. The per-user
    /// default lives at the host boundary, in [`crate::serve_with_config`].
    /// Deleted tournaments' backups are kept for
    /// [`DEFAULT_DELETED_RETENTION`](crate::backup::DEFAULT_DELETED_RETENTION);
    /// use [`with_retention`](Self::with_retention) to say otherwise.
    pub fn new(data_dir: Option<PathBuf>, backups_root: Option<PathBuf>) -> Self {
        Self::with_retention(
            data_dir,
            backups_root,
            crate::backup::DEFAULT_DELETED_RETENTION,
        )
    }

    /// As [`new`](Self::new), but with an explicit retention for the backups of
    /// deleted tournaments (see [`remove`](Self::remove)). `Duration::ZERO`
    /// deletes them immediately, which is the behaviour every release before
    /// this one had.
    pub fn with_retention(
        data_dir: Option<PathBuf>,
        backups_root: Option<PathBuf>,
        deleted_retention: Duration,
    ) -> Self {
        let mut instances = HashMap::new();
        if let Some(dir) = &data_dir {
            // An absent directory is the normal first-boot case. Anything else —
            // an unmounted volume, a permissions change — would otherwise present
            // as "you have no tournaments", moments after `main` logged where it
            // is keeping them, and the picker's empty state is exactly where a
            // referee would start creating replacements for events that are
            // still on disk.
            let entries = match fs::read_dir(dir) {
                Ok(entries) => Some(entries),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => {
                    tracing::error!(
                        "could not read the data directory {}: {e}. No saved tournament \
                         can be loaded — this is not an empty server.",
                        dir.display()
                    );
                    None
                }
            };
            if let Some(entries) = entries {
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
                    // A file this build cannot read is kept as an instance
                    // anyway — empty, but carrying its reason (see
                    // [`LoadProblem`]) so the picker can list it, explain
                    // itself, and let the referee delete it. Every route but
                    // `DELETE` finds no tournament there, which is the truth.
                    // `None` with no problem is nothing to hold on to at all.
                    if store.current().is_none() && store.problem().is_none() {
                        continue; // already warned inside `with_persistence`
                    }
                    // Fail closed: if the sidecar is present but unreadable we
                    // can't tell whether this tournament is password-protected, so
                    // refuse to load it rather than serve it open.
                    let persisted = match Self::load_auth(dir, id) {
                        Ok(persisted) => persisted,
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
                        Arc::new(TournamentInstance::new(
                            store,
                            persisted.password_hash.map(AuthConfig::from_hash),
                            persisted.public_key,
                            backups_root
                                .as_ref()
                                .map(|root| crate::backup::dir_for(root, id)),
                        )),
                    );
                }
            }
        }
        // Startup is the sweep that matters for the desktop app and for any
        // host restarted between tournaments; the one in `remove` covers the
        // server that just keeps running. The tournaments just loaded are the
        // live ones, whose directories it must not touch.
        crate::backup::sweep(
            backups_root.as_deref(),
            deleted_retention,
            &instances.keys().copied().collect(),
        );
        Self {
            instances: RwLock::new(instances),
            data_dir,
            backups_root,
            deleted_retention,
        }
    }

    /// Where a given tournament's automatic backups live, or `None` when no
    /// backups root could be resolved.
    fn backups_dir(&self, id: Uuid) -> Option<PathBuf> {
        self.backups_root
            .as_ref()
            .map(|root| crate::backup::dir_for(root, id))
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

    /// Load the `{id}.auth.json` sidecar, as three distinct outcomes so the
    /// caller can **fail closed** on damage:
    /// - the file is absent → `Ok(default)`: the tournament was created without
    ///   a password and was never published. A create *with* a password writes
    ///   the sidecar before the tournament file (see [`create`](Self::create)),
    ///   so a present tournament with no sidecar is genuinely open, not a torn
    ///   create;
    /// - the file is present and valid → `Ok(..)`: whatever it says;
    /// - the file is present but unreadable or corrupt → `Err(..)`: the caller
    ///   must refuse to load the tournament rather than expose a protected one
    ///   with no password. Atomic writes make this case a real anomaly, not the
    ///   normal torn-write it used to be.
    fn load_auth(dir: &Path, id: Uuid) -> Result<PersistedAuth, String> {
        let path = dir.join(format!("{id}.auth.json"));
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PersistedAuth::default())
            }
            Err(e) => return Err(format!("read {}: {e}", path.display())),
        };
        serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))
    }

    /// Write the `{id}.auth.json` sidecar atomically (temp file + rename), the
    /// same durability the tournament file gets in [`TournamentStore::persist`]
    /// — a half-written sidecar would reload as an *unreadable* sidecar, which
    /// [`load_auth`](Self::load_auth) now (correctly) refuses to serve open.
    /// Best-effort otherwise: a failure is logged, not propagated.
    fn persist_auth(&self, id: Uuid, file: &PersistedAuth) {
        let Some(path) = self.auth_path(id) else {
            return; // in-memory only
        };
        if let Some(dir) = &self.data_dir {
            if let Err(e) = fs::create_dir_all(dir) {
                // The write below would fail anyway; say which step went wrong,
                // since "the data directory can't be created" and "this one file
                // can't be written" call for different fixes.
                tracing::warn!("could not create the data directory {}: {e}", dir.display());
            }
        }
        let bytes = match serde_json::to_vec_pretty(file) {
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

    /// List known tournaments (id, name, city/country/dates, whether it's
    /// password-protected) — enough for the picker, never the tournament's
    /// contents.
    ///
    /// `published_only` restricts the listing to tournaments that have opted
    /// into public reading. That is what an unauthenticated caller gets on a
    /// server that has an admin password: the picker is otherwise the one thing
    /// a stranger who finds the URL can enumerate, and once there is a public
    /// reader UI it would become that UI's front door for tournaments that
    /// never opted in (see `docs/archive/public-access.md` §3.1).
    pub fn list(&self, published_only: bool) -> Vec<TournamentSummary> {
        let instances = self.instances.read().expect("registry lock poisoned");
        instances
            .iter()
            .filter(|(_, instance)| !published_only || instance.public_key().is_some())
            .filter_map(|(id, instance)| {
                // `instance.read()` recovers from a poisoned store lock (see its
                // doc): one tournament whose mutation panicked must not take the
                // whole picker down for every other tournament.
                let store = instance.read();
                let (name, problem) = match store.current() {
                    Some(tournament) => (tournament.name.clone(), None),
                    // A save this build cannot read: listed, but saying why, so
                    // the referee can see it is there and delete it. Never to a
                    // stranger, though — a published tournament that has since
                    // become unreadable has nothing to offer a reader but a
                    // name and a complaint.
                    None => {
                        let problem = store.problem()?;
                        if published_only {
                            return None;
                        }
                        (
                            problem.display_name(*id),
                            Some(TournamentProblem::from(&problem.error)),
                        )
                    }
                };
                // Second `current()` rather than another arm of the match
                // above: an unreadable tournament has no settings to describe
                // it with, and `and_then` says so once instead of three times.
                let settings = store.current().map(|tournament| &tournament.settings);
                Some(TournamentSummary {
                    id: *id,
                    name,
                    has_password: instance.auth.is_some(),
                    city: settings.and_then(|s| s.city.clone()),
                    country: settings.and_then(|s| s.country.clone()),
                    dates: settings.and_then(|s| s.dates.clone()),
                    problem,
                })
            })
            .collect()
    }

    /// Publish a tournament (minting a fresh capability key, or rotating the
    /// existing one), or unpublish it. Returns the new key, or `None` when the
    /// tournament is now unpublished.
    ///
    /// Publication is *not* a tournament mutation: it changes no content, bumps
    /// no version, and is not undoable — it is access-control state, so it goes
    /// straight to the sidecar next to the password hash.
    pub fn set_publication(
        &self,
        id: Uuid,
        published: bool,
    ) -> Result<Option<String>, NoSuchTournament> {
        let instance = self.get(id).ok_or(NoSuchTournament(id))?;
        let key = published.then(mint_public_key);
        instance.set_public_key(key.clone());
        self.persist_auth(
            id,
            &PersistedAuth {
                password_hash: instance
                    .auth
                    .as_ref()
                    .map(|a| a.password_hash().to_string()),
                public_key: key.clone(),
            },
        );
        // Wake every open stream, so a reader holding the *old* key is cut off
        // now rather than at the next edit. A rotation that leaves the people
        // already watching connected is not a revocation at all.
        instance.read().ping();
        Ok(key)
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
    /// `password`.
    ///
    /// See [`insert`](Self::insert) for what registering entails and what the
    /// return value means.
    pub fn create(
        &self,
        name: &str,
        password: Option<String>,
    ) -> Result<(Uuid, Option<String>), TournamentError> {
        Ok(self.insert(Tournament::new(name)?, password))
    }

    /// Register an already-built `tournament` under a fresh id, optionally
    /// protected by `password`. Persists it (tournament + auth sidecar) if a
    /// data directory is configured. Returns the new tournament's id and, when
    /// it has a password, its session token — read from the freshly built
    /// `AuthConfig`, so the caller needn't look the instance back up (and can't
    /// race a concurrent delete doing so).
    ///
    /// The tournament's own `id` is overwritten: the registry, its persisted
    /// filename and its backups are all keyed by the id minted here, so an
    /// imported file's id cannot be allowed to collide with a tournament that
    /// already exists. Callers importing an untrusted file must have run
    /// [`Tournament::validate_loaded`] first — this only assigns an id, it does
    /// not vet what it is registering.
    pub fn insert(
        &self,
        tournament: Tournament,
        password: Option<String>,
    ) -> (Uuid, Option<String>) {
        self.insert_at(Uuid::new_v4(), tournament, password)
    }

    /// [`insert`](Self::insert) under a caller-chosen `id`, for the one case
    /// where the id is not ours to mint: restoring a deleted tournament, which
    /// comes back as *itself* (see [`restore_deleted`](Self::restore_deleted)).
    /// The caller must have checked the id is free — this overwrites whatever
    /// entry it finds, which for any other caller would be a lost tournament.
    fn insert_at(
        &self,
        id: Uuid,
        mut tournament: Tournament,
        password: Option<String>,
    ) -> (Uuid, Option<String>) {
        tournament.id = id;
        if let Some(dir) = &self.data_dir {
            // Nothing below can be persisted without it, and each of those
            // failures is logged on its own; this names the shared cause.
            if let Err(e) = fs::create_dir_all(dir) {
                tracing::warn!("could not create the data directory {}: {e}", dir.display());
            }
        }
        let auth = password.map(AuthConfig::new);
        let token = auth.as_ref().map(|a| a.token().to_string());
        // Write the auth sidecar *before* the tournament file, so a crash between
        // the two writes can only ever leave the sidecar orphaned (harmless: no
        // tournament file, so nothing loads), never a tournament file with its
        // sidecar missing (which would reload as open). See [`load_auth`].
        if let Some(auth) = &auth {
            self.persist_auth(
                id,
                &PersistedAuth {
                    password_hash: Some(auth.password_hash().to_string()),
                    // A new tournament is never published: the referee opts in
                    // explicitly, afterwards.
                    public_key: None,
                },
            );
        }
        let mut store = TournamentStore::empty(self.tournament_path(id));
        store.set_current(tournament);
        self.instances
            .write()
            .expect("registry lock poisoned")
            .insert(
                id,
                Arc::new(TournamentInstance::new(
                    store,
                    auth,
                    None,
                    self.backups_dir(id),
                )),
            );
        (id, token)
    }

    /// Remove a tournament: its registry entry, its persisted file (+ auth
    /// sidecar). Returns whether it existed.
    ///
    /// Its **backups are kept** — a final one is taken first, and the directory
    /// is marked deleted so [`crate::backup::sweep`] removes it once
    /// `deleted_retention` has passed. Deleting is the only referee action with
    /// no undo, and the easiest to do by accident; for that month, the backups
    /// are the way back.
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
            let mut store = instance.write();
            // The final backup, under that same lock and before the tombstone:
            // automatic backups are only taken at round transitions, so the
            // newest one can be many mutations old — without this, the exact
            // state someone just deleted is the one state not recoverable.
            let name = match (store.current(), store.problem()) {
                (Some(tournament), _) => {
                    crate::backup::take(
                        instance.backups_dir.as_deref(),
                        tournament,
                        crate::backup::DELETED_LABEL,
                    );
                    tournament.name.clone()
                }
                // A save this build cannot read (see [`LoadProblem`]). Its file
                // is the only copy of that tournament and we are about to delete
                // it, so it goes to the backups verbatim — unreadable to *this*
                // build, which is not the same as worthless: the build that
                // wrote it can still read it.
                (None, Some(problem)) => {
                    match self.tournament_path(id).map(fs::read) {
                        Some(Ok(bytes)) => crate::backup::take_bytes(
                            instance.backups_dir.as_deref(),
                            &bytes,
                            crate::backup::DELETED_LABEL,
                        ),
                        Some(Err(e)) => tracing::warn!(
                            "deleting {id}: could not copy its unreadable save into the \
                             backups first ({e}); deleting it anyway, as asked"
                        ),
                        None => {}
                    }
                    problem.display_name(id)
                }
                (None, None) => {
                    debug_assert!(
                        false,
                        "a registered tournament always has a state or a problem"
                    );
                    tracing::warn!(
                        "deleting {id}: it held no tournament, so no final backup was taken"
                    );
                    String::new()
                }
            };
            store.mark_deleted();
            drop(store);
            // A file that was never written is already in the state we want, so
            // `NotFound` is success; anything else means a deleted tournament is
            // still on disk and will come back at the next start — worth saying.
            let delete = |path: PathBuf| {
                if let Err(e) = fs::remove_file(&path) {
                    if e.kind() != io::ErrorKind::NotFound {
                        tracing::warn!("could not delete {}: {e}", path.display());
                    }
                }
            };
            if let Some(path) = self.tournament_path(id) {
                delete(path);
            }
            if let Some(path) = self.auth_path(id) {
                delete(path);
            }
            // The password hash outlives the sidecar it was just deleted with:
            // a protected tournament must not become readable to anyone who
            // fishes it out of the bin later.
            crate::backup::mark_deleted(
                instance.backups_dir.as_deref(),
                &name,
                instance.auth.as_ref().map(|auth| auth.password_hash()),
            );
            // Whatever expired while this server was up goes now — the other
            // sweep is at startup, and a server can stay up for months. This one
            // is already out of `instances`, so its own directory is fair game.
            crate::backup::sweep(
                self.backups_root.as_deref(),
                self.deleted_retention,
                &self.live_ids(),
            );
        }
        removed.is_some()
    }

    /// The ids of every tournament this registry currently holds — what tells
    /// [`crate::backup::sweep`] which directories belong to something live.
    fn live_ids(&self) -> HashSet<Uuid> {
        self.instances
            .read()
            .expect("registry lock poisoned")
            .keys()
            .copied()
            .collect()
    }

    /// Every deleted tournament still recoverable, most recently deleted first
    /// (see [`remove`](Self::remove)).
    ///
    /// Sweeps first, so nothing already past its retention is ever offered: a
    /// server left running for months would otherwise list entries the next
    /// restart is about to delete. A live tournament is never listed either,
    /// however its directory is marked — the same rule the sweep follows.
    pub(crate) fn list_deleted(&self) -> Vec<crate::backup::DeletedTournament> {
        let live = self.live_ids();
        crate::backup::sweep(self.backups_root.as_deref(), self.deleted_retention, &live);
        let mut deleted = crate::backup::list_deleted(self.backups_root.as_deref());
        deleted.retain(|d| !live.contains(&d.id));
        deleted
    }

    /// Bring a deleted tournament back, from one of the backups kept with it:
    /// `backup_id` names which, defaulting to the newest (the one taken as it
    /// was deleted).
    ///
    /// It comes back as **itself** — same id, so the backups kept with it become
    /// its own again, marker dropped, leaving exactly the state of a tournament
    /// that was never deleted. That is also why it stops being listed as
    /// deleted, and why an id that is somehow live already is refused rather
    /// than overwritten.
    ///
    /// `password` is the password the tournament had — required if it had one,
    /// refused otherwise; it also becomes the restored tournament's own
    /// password, so a protected tournament comes back exactly as protected as it
    /// was. Returns its id and session token, like [`insert`](Self::insert).
    ///
    /// Nothing is deleted here: the whole backup history moves back with the
    /// tournament, so going further back afterwards is the ordinary restore-a-
    /// backup flow rather than anything to do with the bin.
    pub(crate) fn restore_deleted(
        &self,
        id: Uuid,
        backup_id: Option<&str>,
        password: Option<&str>,
    ) -> Result<(Uuid, Option<String>), RestoreError> {
        if self.get(id).is_some() {
            return Err(RestoreError::AlreadyLive);
        }
        let dir = self.backups_dir(id).ok_or(RestoreError::NotFound)?;
        // No marker means this is not a deleted tournament's directory — a live
        // one's, or nothing at all. Either way there is nothing here to restore.
        let marker = crate::backup::read_marker(&dir).ok_or(RestoreError::NotFound)?;
        match (&marker.password_hash, password) {
            (Some(hash), Some(presented)) if AuthConfig::hash_matches(hash, presented) => {}
            (Some(_), _) => return Err(RestoreError::WrongPassword),
            // Volunteering a password for a tournament that had none would
            // silently protect the restored copy with something the referee
            // never set on it. Say so instead.
            (None, Some(_)) => {
                return Err(RestoreError::PasswordNotNeeded);
            }
            (None, None) => {}
        }
        let backup_id = match backup_id {
            Some(backup_id) => backup_id.to_string(),
            None => {
                crate::backup::list(Some(&dir))
                    .into_iter()
                    .next()
                    .ok_or(RestoreError::NoBackups)?
                    .id
            }
        };
        let tournament = crate::backup::load(Some(&dir), &backup_id).map_err(|e| match e {
            crate::backup::LoadError::NotFound => RestoreError::NoSuchBackup,
            // Deleting a tournament this build could not read keeps its file
            // verbatim, so the bin can hold backups only an older build can
            // open. Restoring one has to say so.
            crate::backup::LoadError::Unreadable(e) => RestoreError::Invalid(e),
        })?;
        // A backup this server wrote should always be loadable, but it is a file
        // on disk that anything could have edited since — so it is vetted like an
        // import, which `backup::load` does by going through [`crate::save`].
        let restored = self.insert_at(id, tournament, password.map(str::to_string));
        // Only now that it is registered: until this point a failure had to
        // leave the entry in the bin, and from here on the directory is a live
        // tournament's again. A marker that survives is loud but harmless — the
        // sweep won't touch a live tournament's backups.
        crate::backup::unmark_deleted(&dir);
        Ok(restored)
    }
}

/// Why [`TournamentRegistry::restore_deleted`] could not restore.
#[derive(Debug)]
pub(crate) enum RestoreError {
    /// No deleted tournament with that id (never existed, or already swept).
    NotFound,
    /// A tournament with that id is already running — it has been restored
    /// once, and restoring again would overwrite the copy in use.
    AlreadyLive,
    /// It is password-protected and the password presented was wrong or absent.
    WrongPassword,
    /// A password was presented for a tournament that had none.
    PasswordNotNeeded,
    /// Its directory survived but holds no backup to restore from.
    NoBackups,
    /// The requested backup isn't one of its backups.
    NoSuchBackup,
    /// The backup file itself no longer describes a valid tournament.
    Invalid(TournamentError),
}

/// State shared across all requests.
///
/// The registry and ratings cache are `RwLock`-guarded and `Arc`-shared so
/// axum can clone the state cheaply per request.
#[derive(Clone)]
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
    /// Minted once per server run, like the per-boot bearer tokens.
    ///
    /// A tournament's `version` is session state: it starts at `0` on every
    /// boot (see [`TournamentStore`]), so it identifies a state only *within*
    /// one run. Anything that caches by version across runs — a public reader's
    /// `ETag`, and the ordering key the phase-3 push will carry — must pair it
    /// with this, or a restart would serve a reader version 5 of a different
    /// tournament state and be told "I already have that one", silently and
    /// forever.
    pub boot_id: Arc<str>,
    /// Browser origins this server answers *in addition* to the app's own (see
    /// `CROSS_ORIGIN_CLIENTS` in [`crate::router`]'s module).
    ///
    /// Empty everywhere that has not deliberately set `OSP_EXTRA_ORIGINS`, which
    /// is every deployment: it exists so a second checkout can run its Vite
    /// server on another port and still be answered. Named origins rather than a
    /// loopback wildcard, because "any port on localhost" is a real widening —
    /// any other local server's page could then script an API that, on a host
    /// with no admin password, is open on every route.
    pub extra_origins: Arc<[String]>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            registry: Arc::default(),
            ratings: Arc::default(),
            admin_auth: None,
            boot_id: Uuid::new_v4().simple().to_string().into(),
            extra_origins: Arc::from([] as [String; 0]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osp_core::NewPlayer;

    /// A stand-in label for the tests that exercise the snapshot, version and
    /// persistence machinery rather than the wording — which is every test here
    /// but the one that reads a label back.
    fn any_label() -> ChangeLabel {
        ChangeLabel::settings()
    }

    #[test]
    fn persists_current_and_reloads_from_disk() {
        let path = std::env::temp_dir().join(format!("osp-persist-{}.json", uuid::Uuid::new_v4()));

        // Write through a create + a player mutation.
        {
            let mut store = TournamentStore::with_persistence(path.clone());
            store.set_current(Tournament::new("Persisted Cup").unwrap());
            store
                .mutate(None, any_label(), |t| {
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

    #[test]
    fn a_failed_write_through_is_recorded_without_refusing_the_edit() {
        // A directory that isn't there stands in for the full disk: the write
        // fails, the edit still lands, and the store says so.
        let path = std::env::temp_dir()
            .join(format!("osp-missing-{}", uuid::Uuid::new_v4()))
            .join("t.json");
        let mut store = TournamentStore::with_persistence(path.clone());
        assert!(store.persisted(), "nothing has been written yet");

        store.set_current(Tournament::new("Doomed Cup").unwrap());
        assert!(
            store.current().is_some(),
            "the edit must not be refused because the disk is unwritable"
        );
        assert!(!store.persisted(), "…but it must not look saved either");

        // And it clears once writing works again.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        store
            .mutate(None, any_label(), |t| {
                t.add_player(NewPlayer {
                    last_name: "Bob".into(),
                    ..Default::default()
                })
                .map(|_| ())
            })
            .unwrap();
        assert!(store.persisted());

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
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
            .mutate(None, any_label(), |t| {
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
            .mutate(None, any_label(), |t| {
                t.add_player(NewPlayer {
                    last_name: "Alice".into(),
                    ..Default::default()
                })
                .map(|_| ())
            })
            .unwrap();
        store.undo().expect("the add is undoable");

        // The on-disk copy reflects the undo (no players), not the pre-undo state.
        let reloaded = TournamentStore::with_persistence(path.clone());
        assert!(reloaded.current().unwrap().players.is_empty());

        std::fs::remove_file(&path).ok();
    }

    /// The label names the change at the *top* of the stack, and each undo
    /// uncovers the one below — which is what lets the button keep saying what
    /// it would take back rather than only whether it could.
    #[test]
    fn the_label_tracks_the_top_of_the_stack() {
        let mut store = TournamentStore::empty(None);
        store.set_current(Tournament::new("Cup").unwrap());
        assert!(store.undo_label().is_none());

        for name in ["Alice", "Bob"] {
            let new = NewPlayer {
                last_name: name.into(),
                ..Default::default()
            };
            store
                .mutate(None, ChangeLabel::register_player(&new), add_named(name))
                .unwrap();
        }

        let label = store.undo_label().expect("Bob's registration");
        assert_eq!(label.code, crate::history::ChangeCode::RegisterPlayer);
        assert_eq!(label.values.get("name").map(String::as_str), Some("Bob"));

        store.undo().unwrap();
        let label = store.undo_label().expect("Alice's, below it");
        assert_eq!(label.values.get("name").map(String::as_str), Some("Alice"));

        store.undo().unwrap();
        assert!(store.undo_label().is_none());
    }

    /// Redo walks back up what undo took down, and the two labels swap sides as
    /// it goes: whatever undo would revert is what redo just re-applied.
    #[test]
    fn redo_walks_back_up_the_stack_undo_came_down() {
        let mut store = TournamentStore::empty(None);
        store.set_current(Tournament::new("Cup").unwrap());
        store.mutate(None, any_label(), add_named("Alice")).unwrap();
        store.mutate(None, any_label(), add_named("Bob")).unwrap();

        store.undo().unwrap();
        store.undo().unwrap();
        assert!(store.current().unwrap().players.is_empty());
        assert!(store.undo_label().is_none());
        assert!(store.redo_label().is_some());

        store.redo().unwrap();
        store.redo().unwrap();
        let names: Vec<&str> = store
            .current()
            .unwrap()
            .players
            .iter()
            .map(|p| p.last_name.as_str())
            .collect();
        // Both back, and in the order they were added — a redo that restored a
        // snapshot from the wrong side would put them back reversed, or lose one.
        assert_eq!(names, ["Alice", "Bob"]);
        assert!(store.redo_label().is_none());
        assert!(store.undo_label().is_some());
    }

    /// The rule that keeps the history a line rather than a tree: once a new
    /// edit lands, the undone branch is unreachable and must not be offered.
    #[test]
    fn a_new_mutation_discards_what_was_undone() {
        let mut store = TournamentStore::empty(None);
        store.set_current(Tournament::new("Cup").unwrap());
        store.mutate(None, any_label(), add_named("Alice")).unwrap();
        store.undo().unwrap();
        assert!(store.redo_label().is_some());

        store.mutate(None, any_label(), add_named("Bob")).unwrap();

        assert!(store.redo_label().is_none());
        assert!(matches!(store.redo(), Err(NothingLeft(Direction::Redo))));
        // And the branch really is gone: redoing Alice on top of Bob would have
        // swapped in a tournament that never contained him.
        let names: Vec<&str> = store
            .current()
            .unwrap()
            .players
            .iter()
            .map(|p| p.last_name.as_str())
            .collect();
        assert_eq!(names, ["Bob"]);
    }

    /// A load establishes a new baseline, and its snapshots carry a different
    /// tournament id — so a redo left over from the previous one would not
    /// merely be stale, it would swap the whole tournament out.
    #[test]
    fn loading_a_tournament_clears_the_redo_stack_too() {
        let mut store = TournamentStore::empty(None);
        store.set_current(Tournament::new("Cup").unwrap());
        store.mutate(None, any_label(), add_named("Alice")).unwrap();
        store.undo().unwrap();
        assert!(store.redo_label().is_some());

        let other = Tournament::new("Another Cup").unwrap();
        let other_id = other.id;
        store.set_current(other);

        assert!(store.redo_label().is_none());
        assert!(store.undo_label().is_none());
        assert_eq!(store.current().unwrap().id, other_id);
    }

    /// An undo with nothing to revert must say so rather than report success:
    /// a caller told `Ok` would believe a change it never made had landed.
    #[test]
    fn undo_with_an_empty_history_is_an_error_not_a_no_op() {
        let mut store = TournamentStore::empty(None);
        store.set_current(Tournament::new("Cup").unwrap());
        let version = store.version();

        assert!(store.undo_label().is_none());
        assert!(matches!(store.undo(), Err(NothingLeft(Direction::Undo))));
        assert!(matches!(store.redo(), Err(NothingLeft(Direction::Redo))));
        // And they changed nothing: no phantom version bump for subscribers to
        // refetch on, so a rejected step cannot look like somebody's edit.
        assert_eq!(store.version(), version);
    }

    #[test]
    fn registry_creates_lists_and_isolates_tournaments() {
        let registry = TournamentRegistry::default();
        let (a, _) = registry.create("Cup A", None).unwrap();
        let (b, _) = registry.create("Cup B", Some("secret".into())).unwrap();

        let mut names: Vec<_> = registry
            .list(false)
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
            .mutate(None, any_label(), |t| {
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
            let registry = TournamentRegistry::new(Some(dir.clone()), None);
            registry
                .create("Persisted Cup", Some("secret".into()))
                .unwrap()
                .0
        };

        let reloaded = TournamentRegistry::new(Some(dir.clone()), None);
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
    fn publication_survives_a_restart_alongside_the_password() {
        let dir = std::env::temp_dir().join(format!("osp-publish-{}", uuid::Uuid::new_v4()));

        let (id, key) = {
            let registry = TournamentRegistry::new(Some(dir.clone()), None);
            let (id, _) = registry
                .create("Published Cup", Some("secret".into()))
                .unwrap();
            let key = registry
                .set_publication(id, true)
                .unwrap()
                .expect("published");
            (id, key)
        };

        // The key comes back, and so does the password it sits beside — the two
        // share one sidecar, so writing either must not drop the other.
        let reloaded = TournamentRegistry::new(Some(dir.clone()), None);
        let instance = reloaded.get(id).expect("reloaded from disk");
        assert_eq!(instance.public_key().as_deref(), Some(key.as_str()));
        assert!(instance.auth.as_ref().unwrap().password_matches("secret"));

        // Unpublishing is persisted too, and the tournament is no longer listed
        // to an unauthenticated caller.
        reloaded.set_publication(id, false).unwrap();
        assert!(reloaded.list(true).is_empty());
        let again = TournamentRegistry::new(Some(dir.clone()), None);
        assert!(again.get(id).unwrap().public_key().is_none());
        assert!(again
            .get(id)
            .unwrap()
            .auth
            .as_ref()
            .unwrap()
            .password_matches("secret"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn public_stream_slots_are_capped_and_released_on_disconnect() {
        let instance = Arc::new(TournamentInstance::new(
            TournamentStore::empty(None),
            None,
            None,
            None,
        ));

        let first = instance.open_public_stream(2).expect("under the cap");
        let second = instance.open_public_stream(2).expect("at the cap");
        // An unauthenticated stream anyone may open is a resource-exhaustion
        // surface, so the third is refused rather than merely slow.
        assert!(instance.open_public_stream(2).is_none());

        // A reader who walks out of the venue frees their slot.
        drop(first);
        let third = instance.open_public_stream(2).expect("a slot came free");
        assert!(instance.open_public_stream(2).is_none());
        drop((second, third));
        assert!(instance.open_public_stream(2).is_some());
    }

    #[test]
    fn a_corrupt_auth_sidecar_fails_closed_not_open() {
        let dir = std::env::temp_dir().join(format!("osp-authfail-{}", uuid::Uuid::new_v4()));

        let id = {
            let registry = TournamentRegistry::new(Some(dir.clone()), None);
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
        let reloaded = TournamentRegistry::new(Some(dir.clone()), None);
        assert!(
            reloaded.get(id).is_none(),
            "a tournament with an unreadable auth sidecar must fail closed, not load open"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_poisoned_store_lock_still_serves_later_requests() {
        let instance = Arc::new(TournamentInstance::new(
            {
                let mut s = TournamentStore::empty(None);
                s.set_current(Tournament::new("Cup").unwrap());
                s
            },
            None,
            None,
            None,
        ));

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
            store.mutate(Some(0), any_label(), add_named("Alice")),
            Err(MutateError::VersionConflict)
        ));
        assert_eq!(store.version(), 1);
        assert!(store.current().unwrap().players.is_empty());

        // Based on the current version it goes through and advances the version.
        store
            .mutate(Some(1), any_label(), add_named("Alice"))
            .unwrap();
        assert_eq!(store.version(), 2);
        assert_eq!(store.current().unwrap().players.len(), 1);

        // `None` opts out of the check entirely (the desktop/local path).
        store.mutate(None, any_label(), add_named("Bob")).unwrap();
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
            store.mutate(None, any_label(), add_named("Alice")),
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
                .mutate(None, any_label(), add_named("Alice")),
            Err(MutateError::NoTournament)
        ));
        assert!(instance.store.read().unwrap().is_deleted());
    }

    /// Roots of this test's own, so a sweep in one test can't see another's.
    fn isolated_roots() -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("osp-retention-{}", uuid::Uuid::new_v4()));
        (base.join("tournaments"), base.join("backups"))
    }

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    /// A registry given no backups root resolves none of its own — it keeps no
    /// backups at all, and in particular does not reach for the per-user data
    /// directory.
    ///
    /// This is the whole of the fix for integration tests writing into the
    /// developer's real OpenShogiPairings data: `AppState::default()` — which
    /// every one of them builds, most without a thought for backups — comes
    /// through here. Guarding it needs a *unit* test rather than an assertion
    /// over the real directory, because an integration test could not tell a
    /// file it wrote from one already there.
    #[test]
    fn a_registry_given_no_backups_root_keeps_no_backups() {
        let registry = TournamentRegistry::default();
        assert!(
            registry.backups_root.is_none(),
            "a registry with no root must not invent one"
        );

        let (id, _) = registry.create("Rootless Cup", None).unwrap();
        assert!(
            registry.backups_dir(id).is_none(),
            "and so no tournament under it has anywhere to write"
        );
        assert!(registry.list_deleted().is_empty());
        assert!(registry.remove(id), "deleting it is still fine");
    }

    #[test]
    fn removing_a_tournament_keeps_its_backups_and_its_password_hash() {
        let (data_dir, backups_root) = isolated_roots();
        let registry = TournamentRegistry::with_retention(
            Some(data_dir.clone()),
            Some(backups_root.clone()),
            30 * DAY,
        );
        let (id, _) = registry
            .create("Doomed Cup", Some("secret".into()))
            .unwrap();

        assert!(registry.remove(id));

        // The tournament and its auth sidecar are gone...
        assert!(!data_dir.join(format!("{id}.json")).exists());
        assert!(!data_dir.join(format!("{id}.auth.json")).exists());

        // ...but a backup of the state it was deleted in is not, even though
        // this tournament never reached a round transition.
        let dir = crate::backup::dir_for(&backups_root, id);
        let backups = crate::backup::list(Some(&dir));
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].label, crate::backup::DELETED_LABEL);
        let restored = crate::backup::load(Some(&dir), &backups[0].id).expect("the final backup");
        assert_eq!(restored.name, "Doomed Cup");

        // And the marker carries the password hash the sidecar took to its
        // grave: restoring this must not hand it over unprotected.
        let marker: crate::backup::DeletedMarker = serde_json::from_slice(
            &fs::read(dir.join(crate::backup::DELETED_MARKER)).expect("a marker"),
        )
        .expect("a valid marker");
        assert_eq!(marker.name, "Doomed Cup");
        assert!(marker
            .password_hash
            .expect("the hash outlives the sidecar")
            .starts_with("$2"));

        fs::remove_dir_all(data_dir.parent().unwrap()).ok();
    }

    /// Zero retention is the pre-retention behaviour, and the sweep in `remove`
    /// is what applies it: the backups go with the tournament.
    #[test]
    fn zero_retention_deletes_the_backups_with_the_tournament() {
        let (data_dir, backups_root) = isolated_roots();
        let registry = TournamentRegistry::with_retention(
            Some(data_dir.clone()),
            Some(backups_root.clone()),
            Duration::ZERO,
        );
        let (id, _) = registry.create("Doomed Cup", None).unwrap();

        assert!(registry.remove(id));
        assert!(!crate::backup::dir_for(&backups_root, id).exists());

        fs::remove_dir_all(data_dir.parent().unwrap()).ok();
    }

    /// The other sweep: a host restarted after the retention has run out clears
    /// what it finds, without needing another deletion to trigger it.
    #[test]
    fn startup_sweeps_backups_whose_retention_has_run_out() {
        let (data_dir, backups_root) = isolated_roots();
        let (deleted, kept) = {
            let registry = TournamentRegistry::with_retention(
                Some(data_dir.clone()),
                Some(backups_root.clone()),
                30 * DAY,
            );
            let (deleted, _) = registry.create("Doomed Cup", None).unwrap();
            let (kept, _) = registry.create("Live Cup", None).unwrap();
            // Give the survivor a backup of its own, so the sweep has something
            // it must *not* touch.
            let instance = registry.get(kept).expect("just created");
            crate::backup::take(
                instance.backups_dir.as_deref(),
                instance.read().current().unwrap(),
                "round 1 started",
            );
            assert!(registry.remove(deleted));
            (deleted, kept)
        };
        assert!(crate::backup::dir_for(&backups_root, deleted).exists());

        // Restart with a retention that has already elapsed for that deletion.
        let reloaded = TournamentRegistry::with_retention(
            Some(data_dir.clone()),
            Some(backups_root.clone()),
            Duration::ZERO,
        );
        assert!(!crate::backup::dir_for(&backups_root, deleted).exists());
        // The live tournament reloaded, and kept every backup it had: only a
        // marked directory is ever swept.
        assert!(reloaded.get(kept).is_some());
        assert_eq!(
            crate::backup::list(Some(&crate::backup::dir_for(&backups_root, kept))).len(),
            1
        );

        fs::remove_dir_all(data_dir.parent().unwrap()).ok();
    }

    /// The picker is the third way a save reaches this build (after import and a
    /// backup restore), and a referee upgrading a hosted server has their v1.3.0
    /// files sitting in its data directory — so the upgrade has to happen on the
    /// boot path too, not only on the one the file dialog uses.
    #[test]
    fn a_v1_3_0_file_in_the_data_directory_opens_at_boot() {
        let (data_dir, backups_root) = isolated_roots();
        fs::create_dir_all(&data_dir).unwrap();
        let id = uuid::Uuid::new_v4();
        fs::write(
            data_dir.join(format!("{id}.json")),
            include_str!("../tests/fixtures/v1.3.0-settings.json"),
        )
        .unwrap();

        let registry = TournamentRegistry::with_retention(
            Some(data_dir.clone()),
            Some(backups_root.clone()),
            30 * DAY,
        );
        let listed = registry.list(false);
        assert_eq!(listed.len(), 1);
        assert!(listed[0].problem.is_none(), "it must simply open");
        assert_eq!(listed[0].name, "Maximal v1.3.0 Open");

        let instance = registry.get(id).expect("listed, therefore known");
        let store = instance.read();
        let tournament = store.current().expect("it opened");
        assert_eq!(
            tournament.format_version,
            osp_core::TOURNAMENT_FORMAT_VERSION
        );
        assert_eq!(tournament.players.len(), 2);
        assert_eq!(tournament.settings.categories.len(), 1);
        drop(store);

        fs::remove_dir_all(data_dir.parent().unwrap()).ok();
    }

    /// A save this build cannot read stays in the picker — listed, explaining
    /// itself, and deletable. Hiding it is how a tournament silently stops
    /// existing while its files sit on disk with nothing to remove them.
    #[test]
    fn a_save_from_an_old_format_is_listed_with_its_problem_and_can_be_deleted() {
        let (data_dir, backups_root) = isolated_roots();
        fs::create_dir_all(&data_dir).unwrap();
        let id = uuid::Uuid::new_v4();
        fs::write(
            data_dir.join(format!("{id}.json")),
            br#"{"format_version":4,"name":"Nancy Open 2025","players":[]}"#,
        )
        .unwrap();

        let registry = TournamentRegistry::with_retention(
            Some(data_dir.clone()),
            Some(backups_root.clone()),
            30 * DAY,
        );
        let listed = registry.list(false);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        // Named from the file even though the file as a whole doesn't parse.
        assert_eq!(listed[0].name, "Nancy Open 2025");
        let problem = listed[0].problem.as_ref().expect("it cannot be opened");
        assert_eq!(problem.code, Some("unsupported_format_version"));
        assert_eq!(problem.values["found"], "4");

        // It holds no tournament, so nothing can be done *with* it...
        let instance = registry.get(id).expect("listed, therefore known");
        assert!(instance.read().current().is_none());
        // ...and a stranger is never shown it, whatever its sidecar says.
        assert!(registry.list(true).is_empty());

        // Deleting works, and keeps the unreadable file as a backup: this build
        // can't read it, but the one that wrote it still can.
        assert!(registry.remove(id));
        assert!(!data_dir.join(format!("{id}.json")).exists());
        assert!(registry.list(false).is_empty());
        let deleted = registry.list_deleted();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].name, "Nancy Open 2025");
        assert_eq!(deleted[0].backups.len(), 1);
        // The bin says it too, so the picker can grey it out and offer no
        // Restore button rather than one that always fails.
        assert_eq!(
            deleted[0].problem.as_ref().map(|p| p.code),
            Some(Some("unsupported_format_version"))
        );
        let kept = fs::read(
            crate::backup::dir_for(&backups_root, id)
                .join(format!("{}.json", deleted[0].backups[0].id)),
        )
        .unwrap();
        assert!(String::from_utf8_lossy(&kept).contains("Nancy Open 2025"));

        // Restoring it says what is actually wrong, rather than pretending the
        // backup isn't there.
        assert!(matches!(
            registry.restore_deleted(id, None, None),
            Err(RestoreError::Invalid(
                TournamentError::UnsupportedFormatVersion { found: 4, .. }
            ))
        ));

        fs::remove_dir_all(data_dir.parent().unwrap()).ok();
    }

    /// A file that isn't JSON at all gets the same treatment, minus the name.
    #[test]
    fn a_corrupt_save_is_listed_under_its_id() {
        let (data_dir, backups_root) = isolated_roots();
        fs::create_dir_all(&data_dir).unwrap();
        let id = uuid::Uuid::new_v4();
        fs::write(data_dir.join(format!("{id}.json")), b"{ not json").unwrap();

        let registry = TournamentRegistry::new(Some(data_dir.clone()), Some(backups_root));
        let listed = registry.list(false);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, id.to_string());
        assert_eq!(
            listed[0].problem.as_ref().map(|p| p.code),
            Some(Some("malformed_save"))
        );

        fs::remove_dir_all(data_dir.parent().unwrap()).ok();
    }

    /// A save whose cup no longer matches its bracket size parses fine — the
    /// shape is intact — and then panics the first time a response derives the
    /// bracket from it. Only `validate_loaded` catches that, and the boot path
    /// has to run it just as the import route does.
    #[test]
    fn a_save_whose_cup_fails_validation_is_listed_rather_than_loaded() {
        use osp_core::TournamentSettings;

        let (data_dir, backups_root) = isolated_roots();
        fs::create_dir_all(&data_dir).unwrap();
        let mut t = Tournament::new("Nancy Cup 2026").unwrap();
        t.update_settings(TournamentSettings {
            cup_enabled: true,
            ..Default::default()
        })
        .unwrap();
        for i in 0..8 {
            let player = t
                .add_player(NewPlayer {
                    last_name: format!("E{i}"),
                    rating: Some(2000 - i * 10),
                    ..Default::default()
                })
                .unwrap()
                .id;
            t.set_player_eligible(player, true).unwrap();
        }
        t.finalize_registration_with(Some(8)).unwrap();
        // Hand-edit the bracket size, as only a text editor can.
        t.cup.as_mut().unwrap().size = 12;
        let id = t.id;
        fs::write(
            data_dir.join(format!("{id}.json")),
            serde_json::to_vec(&t).unwrap(),
        )
        .unwrap();

        let registry = TournamentRegistry::new(Some(data_dir.clone()), Some(backups_root));
        let listed = registry.list(false);
        assert_eq!(listed.len(), 1);
        // Listed under its own name, with the reason — not served, and not
        // silently dropped either.
        assert_eq!(listed[0].name, "Nancy Cup 2026");
        let problem = listed[0].problem.as_ref().expect("listed as a problem");
        assert!(
            problem.message.contains("invalid cup size 12"),
            "unexpected problem message: {}",
            problem.message
        );

        fs::remove_dir_all(data_dir.parent().unwrap()).ok();
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
            .mutate(Some(base), any_label(), add_named("Alice"))
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
