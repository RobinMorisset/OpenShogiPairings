//! Automatic server-side backups of the tournament.
//!
//! Taken at each round state-machine transition (registration finalized, a
//! round prepared/confirmed/completed/cancelled) — coarser than the undo
//! stack, which snapshots on every single mutation. These live on disk so a
//! referee can recover from a multi-step mistake (or a server restart)
//! without needing a manual save. One rotating directory per tournament id;
//! the oldest backups beyond [`MAX_BACKUPS`] are deleted after each write.
//!
//! Deleting a tournament does **not** delete its backups. Instead the directory
//! gets a [`DELETED_MARKER`] file recording when (and what) was deleted, and
//! [`sweep`] removes marked directories once they are older than the retention
//! period — so the one irreversible operation in the app stays recoverable for
//! a month. A directory with no marker is a live tournament's and is never
//! swept.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use osp_core::{Tournament, TournamentError};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::state::TournamentProblem;

/// How many backups are kept per tournament before the oldest are rotated out.
/// A plain constant "for now" — easy to turn into a user-facing setting later.
pub(crate) const MAX_BACKUPS: usize = 10;

/// How long a deleted tournament's backups are kept before [`sweep`] removes
/// them, unless the host overrides it (`OSP_BACKUP_RETENTION_DAYS`). A month:
/// long enough that an accidental deletion is noticed — the usual way being the
/// next time someone looks for that tournament — without keeping every
/// tournament a club ever ran.
pub(crate) const DEFAULT_DELETED_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Marker file written into a tournament's backups directory when the
/// tournament itself is deleted; see [`DeletedMarker`]. Its *presence* is what
/// makes a directory eligible for the retention sweep, so a live tournament's
/// backups can never be swept by accident.
pub(crate) const DELETED_MARKER: &str = "deleted.json";

/// The label of the extra backup taken at deletion time.
pub(crate) const DELETED_LABEL: &str = "deleted";

/// What [`DELETED_MARKER`] holds: when the tournament was deleted, and enough
/// about it to offer the backups back later without reading them all.
///
/// The password hash is kept deliberately: a password-protected tournament must
/// not become readable to everyone merely by passing through the bin, so
/// whatever restores it later can demand the same password it had. It is a
/// bcrypt hash, exactly as in the `{id}.auth.json` sidecar this replaces — no
/// more sensitive than the file that was just deleted, and in the same
/// per-user directory.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeletedMarker {
    /// Unix seconds at which the tournament was deleted.
    pub deleted_at: u64,
    /// The tournament's name when it was deleted — what a "recently deleted"
    /// list has to show, and the backups themselves only give up on parse.
    pub name: String,
    /// bcrypt hash of the tournament's own password, if it had one.
    pub password_hash: Option<String>,
}

/// One backup's metadata, as listed to clients (the tournament body itself is
/// only fetched on restore).
#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub(crate) struct BackupInfo {
    /// Opaque id used to restore this backup (its file stem).
    pub id: String,
    /// Unix seconds when the backup was taken.
    #[ts(type = "number")]
    pub taken_at: u64,
    /// Which transition triggered it, e.g. "round 2 started".
    pub label: String,
}

/// One deleted tournament still in the bin, as offered back to a client.
///
/// The password itself is never sent — only `has_password`, so the picker knows
/// to ask for it before it can restore. The whole point of keeping the hash is
/// that a protected tournament stays protected on the way back.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../frontend/src/lib/generated/")]
pub(crate) struct DeletedTournament {
    /// Its id — the key to restore it by, and the id it comes back under: a
    /// restored tournament is the same tournament, not a copy of it.
    pub id: Uuid,
    pub name: String,
    /// Unix seconds at which it was deleted.
    #[ts(type = "number")]
    pub deleted_at: u64,
    pub has_password: bool,
    /// Its surviving backups, newest first — the last of them taken as it was
    /// deleted. Sent whole so the referee can pick an earlier one rather than
    /// only ever getting the final state back.
    pub backups: Vec<BackupInfo>,
    /// Why it cannot be restored, when it cannot — a save from a format version
    /// this build no longer reads, which is exactly what deleting an already
    /// unopenable tournament leaves behind. `None` is the normal case, and the
    /// only one where restoring is offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<TournamentProblem>,
}

/// Where backups go when the *host* was given no explicit root: the per-user
/// data directory, as every release has used. `None` on a platform with no
/// known data directory — the caller then keeps no backups at all rather than
/// inventing a location.
///
/// Called only where a host resolves its configuration — [`crate::serve_with_config`]
/// and the binaries that report the effective location. Deliberately *not*
/// called from [`crate::state::TournamentRegistry`]: a registry built with no
/// root keeps no backups, full stop. It used to fall back here, which meant
/// every `AppState::default()` in a test wrote into the developer's real
/// OpenShogiPairings data directory. A `cfg!(test)` diversion to the temp dir
/// was supposed to prevent that and could not: this crate's integration tests
/// link against the library compiled *without* `cfg(test)`, so the diversion
/// was dead exactly where the writes were coming from. Resolving the default
/// once, at the boundary that has a host to speak for, is what closes it.
pub(crate) fn default_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("openshogipairings").join("backups"))
}

/// The directory holding one tournament's backups: its own id under `root`.
pub(crate) fn dir_for(root: &Path, tournament_id: Uuid) -> PathBuf {
    root.join(tournament_id.to_string())
}

/// Seconds since the epoch, which every backup is named and dated by.
///
/// Panics rather than falling back on a clock set before 1970, because the
/// fallback was `0` and `0` is not merely wrong here: it is the timestamp
/// [`sweep`] reads as "deleted 56 years ago", so a tournament deleted on such a
/// machine has its backups — the only remaining copy — swept on the next boot.
/// A clock that far off is a broken machine, not a state to keep running in.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock is set before 1970")
        .as_secs()
}

/// A filename-safe version of `label`: lowercased, non-alphanumerics collapsed
/// to a single dash.
fn slug(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut last_was_dash = false;
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Disambiguates backups taken within the same second (e.g. two transitions
/// in a fast test), so filenames — and therefore ids — never collide.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Write a new backup of `tournament`, tagged with a human-readable `label`
/// (e.g. "round 2 started"), then rotate out the oldest backups beyond
/// [`MAX_BACKUPS`]. Best-effort: I/O or serialization failures are logged, not
/// propagated — a backup problem must never block the referee's actual action.
pub(crate) fn take(dir: Option<&Path>, tournament: &Tournament, label: &str) {
    match serde_json::to_vec_pretty(tournament) {
        Ok(json) => take_bytes(dir, &json, label),
        Err(e) => tracing::warn!("backup: could not serialize the tournament: {e}"),
    }
}

/// [`take`] for a tournament that is already serialized — which is the only way
/// to back up a save this build cannot parse into a [`Tournament`] (an older
/// format version, on its way to being deleted). The bytes are written
/// verbatim: whatever wrote them can still read them.
pub(crate) fn take_bytes(dir: Option<&Path>, bytes: &[u8], label: &str) {
    let Some(dir) = dir else {
        tracing::warn!("backup: no backups directory — this tournament is not being backed up");
        return;
    };
    if let Err(e) = fs::create_dir_all(dir) {
        tracing::warn!("backup: could not create {}: {e}", dir.display());
        return;
    }
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{}-{seq}-{}.json", now_secs(), slug(label)));
    if let Err(e) = fs::write(&path, bytes) {
        tracing::warn!("backup: could not write {}: {e}", path.display());
    }
    rotate(dir);
}

/// Delete the oldest backups in `dir` beyond [`MAX_BACKUPS`], oldest first by
/// the `(secs, seq)` encoded in each filename — *not* a plain lexicographic
/// sort, since `seq` isn't zero-padded ("10" would otherwise sort before "9").
fn rotate(dir: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        // No directory yet means nothing to rotate. Any other failure means
        // rotation has stopped, and a backups directory that grows without
        // bound is exactly what this exists to prevent — same treatment as
        // [`sweep`].
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!("backup: could not scan {} to rotate it: {e}", dir.display());
            return;
        }
    };
    let mut files: Vec<(u64, u64, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?.to_string();
            let mut parts = stem.splitn(3, '-');
            let secs: u64 = parts.next()?.parse().ok()?;
            let seq: u64 = parts.next()?.parse().ok()?;
            Some((secs, seq, path))
        })
        .collect();
    files.sort_by_key(|(secs, seq, _)| (*secs, *seq));
    if files.len() > MAX_BACKUPS {
        for (_, _, path) in &files[..files.len() - MAX_BACKUPS] {
            // Keep rotating the rest either way, but say so: a rotation that
            // keeps failing is how a backups directory grows without bound.
            if let Err(e) = fs::remove_file(path) {
                tracing::warn!("backup: could not rotate out {}: {e}", path.display());
            }
        }
    }
}

/// List the backups in `dir`, newest first.
pub(crate) fn list(dir: Option<&Path>) -> Vec<BackupInfo> {
    let Some(dir) = dir else {
        return Vec::new();
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        // Absent is "no backups taken yet", which is what an empty list means.
        // Anything else is "I cannot see your backups", which is emphatically
        // not the same answer — and this list is what the restore UI offers.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!("backup: could not list {}: {e}", dir.display());
            return Vec::new();
        }
    };
    // Sort by (seconds, sequence) — several backups can share the same second,
    // and `seq` is what breaks the tie in the order they were actually taken.
    let mut infos: Vec<(u64, u64, BackupInfo)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?.to_string();
            // `<secs>-<seq>-<slugified label>`; the label itself may contain dashes.
            let mut parts = stem.splitn(3, '-');
            let secs: u64 = parts.next()?.parse().ok()?;
            let seq: u64 = parts.next()?.parse().ok()?;
            let label = parts.next().unwrap_or("").replace('-', " ");
            Some((
                secs,
                seq,
                BackupInfo {
                    taken_at: secs,
                    label,
                    id: stem,
                },
            ))
        })
        .collect();
    infos.sort_by_key(|(secs, seq, _)| std::cmp::Reverse((*secs, *seq)));
    infos.into_iter().map(|(_, _, info)| info).collect()
}

/// Mark `dir` as belonging to a deleted tournament, so [`sweep`] removes it
/// once the retention period has passed. The backups themselves are left
/// exactly as they are — that is the point: deletion is the one referee action
/// with no undo, and until the sweep catches up these files are the way back.
///
/// A directory that doesn't exist is left alone rather than created: nothing was
/// ever backed up for that tournament, so there is nothing to keep and no
/// reason to leave an empty marked directory behind.
///
/// Best-effort like the rest of this module, but a failure is an **error**, not
/// a warning: an unmarked directory is never swept, so this is how a backups
/// root grows without bound.
pub(crate) fn mark_deleted(dir: Option<&Path>, name: &str, password_hash: Option<&str>) {
    let Some(dir) = dir else {
        return;
    };
    if !dir.is_dir() {
        return;
    }
    let marker = DeletedMarker {
        deleted_at: now_secs(),
        name: name.to_string(),
        password_hash: password_hash.map(str::to_string),
    };
    let path = dir.join(DELETED_MARKER);
    let json = match serde_json::to_vec_pretty(&marker) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("backup: could not serialize {}: {e}", path.display());
            return;
        }
    };
    if let Err(e) = fs::write(&path, json) {
        tracing::error!(
            "backup: could not write {}: {e}. These backups are kept, but nothing \
             will ever clean them up — delete {} by hand once you no longer want them.",
            path.display(),
            dir.display()
        );
    }
}

/// Drop `dir`'s deleted marker: its tournament exists again, so this is an
/// ordinary live tournament's backups directory once more — the same state as
/// one that was never deleted, history and all.
///
/// Returns whether the marker is gone. A failure here leaves a live tournament's
/// backups looking deleted, which is why [`sweep`] refuses to touch a directory
/// whose tournament is in the registry: the worst case is a stale entry in the
/// bin, never a live tournament's backups being swept out from under it.
pub(crate) fn unmark_deleted(dir: &Path) -> bool {
    let path = dir.join(DELETED_MARKER);
    match fs::remove_file(&path) {
        Ok(()) => true,
        // Already gone is the state we wanted.
        Err(e) if e.kind() == io::ErrorKind::NotFound => true,
        Err(e) => {
            tracing::error!(
                "backup: could not remove {}: {e}. Its tournament is back, so these \
                 backups are live again — delete that file by hand to stop it being \
                 listed as deleted.",
                path.display()
            );
            false
        }
    }
}

/// Read `dir`'s deleted marker: `Some` when the directory belongs to a deleted
/// tournament, `None` when it belongs to a live one (no marker) or the marker
/// can't be read. A corrupt marker is reported and treated as "not deleted",
/// the same way [`sweep`] treats it — the directory is then neither offered for
/// restore nor swept, which is the state that loses nothing.
pub(crate) fn read_marker(dir: &Path) -> Option<DeletedMarker> {
    let path = dir.join(DELETED_MARKER);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("backup: could not read {}: {e}", path.display());
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(marker) => Some(marker),
        Err(e) => {
            tracing::warn!("backup: could not parse {}: {e}", path.display());
            None
        }
    }
}

/// Every deleted tournament still in `root`, most recently deleted first.
///
/// A directory whose name isn't a tournament id can't be restored (the id is
/// how a client names it), so it is skipped — loudly, because nothing this
/// server writes produces one.
pub(crate) fn list_deleted(root: Option<&Path>) -> Vec<DeletedTournament> {
    let Some(root) = root else {
        return Vec::new();
    };
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!("backup: could not scan {}: {e}", root.display());
            return Vec::new();
        }
    };
    let mut deleted: Vec<DeletedTournament> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let dir = entry.path();
            let marker = read_marker(&dir)?;
            let Some(Ok(id)) = dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(Uuid::parse_str)
            else {
                tracing::warn!(
                    "backup: {} holds a deleted tournament but is not named after one; \
                     it cannot be restored",
                    dir.display()
                );
                return None;
            };
            let backups = list(Some(&dir));
            Some(DeletedTournament {
                id,
                name: marker.name,
                deleted_at: marker.deleted_at,
                has_password: marker.password_hash.is_some(),
                problem: restore_problem(&dir, &backups),
                backups,
            })
        })
        .collect();
    deleted.sort_by_key(|d| std::cmp::Reverse(d.deleted_at));
    deleted
}

/// Why restoring this deleted tournament would fail, or `None` when it would
/// work — by actually loading the backup a restore would use, which is the only
/// answer that can't be wrong.
///
/// Deleting a tournament this build could not read keeps its save verbatim, so
/// a bin entry can hold nothing but backups from an older format version. It has
/// to say so *in the list*: offering a Restore button that always fails is the
/// same silent-dead-end the picker's ⚠️ entries exist to avoid.
///
/// Costs one file read and parse per deleted tournament per listing. The bin
/// holds a month of deletions, so that is a handful of small files — if it ever
/// stops being, probe the format version alone instead of parsing.
fn restore_problem(dir: &Path, backups: &[BackupInfo]) -> Option<TournamentProblem> {
    let Some(newest) = backups.first() else {
        // Marked deleted, but nothing left to restore from. Not a state this
        // server produces — deletion writes a backup first — so it stays
        // English, as an anomaly rather than ordinary UI.
        return Some(TournamentProblem::plain(format!(
            "no backup left in {}",
            dir.display()
        )));
    };
    match load(Some(dir), &newest.id) {
        Ok(_) => None,
        Err(LoadError::Unreadable(e)) => Some(TournamentProblem::from(&e)),
        Err(LoadError::NotFound) => Some(TournamentProblem::plain(format!(
            "backup {} is listed but could not be read",
            newest.id
        ))),
    }
}

/// Delete the backups of every tournament deleted more than `retention` ago.
///
/// Scans `root` for directories carrying a [`DELETED_MARKER`]; anything without
/// one belongs to a live tournament and is skipped. Called at startup and after
/// each deletion, which between them covers both the laptop restarted for every
/// tournament and the server left running for months — no background task.
///
/// `live` is every tournament id the registry currently holds, and a directory
/// named after one of them is **never** swept whatever its marker says. The
/// marker is supposed to be gone by then (see [`unmark_deleted`]), so this only
/// matters when removing it failed — and the price of that failure must not be
/// a live tournament losing its backups.
///
/// Errs towards keeping in every other doubtful case too: a marker that can't be
/// read or parsed leaves its directory in place (loudly), because the
/// alternative is guessing an expiry date and deleting the only copy of a
/// tournament.
pub(crate) fn sweep(root: Option<&Path>, retention: Duration, live: &HashSet<Uuid>) {
    let Some(root) = root else {
        return;
    };
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        // Nothing has ever been backed up here; nothing to sweep.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(
                "backup: could not scan {} for expired backups: {e}",
                root.display()
            );
            return;
        }
    };
    let now = now_secs();
    for entry in entries.filter_map(Result::ok) {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        // No marker (a live tournament's backups), or one that can't be read:
        // [`read_marker`] has already said so, and either way these backups stay
        // — the alternative is guessing an expiry date and deleting the only
        // copy of a tournament.
        let Some(marker) = read_marker(&dir) else {
            continue;
        };
        // A directory whose tournament is running: a marker left behind by a
        // restore that could not remove it. Say so — it is also why that
        // tournament still shows up in the bin.
        if dir
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| Uuid::parse_str(n).ok())
            .is_some_and(|id| live.contains(&id))
        {
            tracing::warn!(
                "backup: {} is marked deleted but its tournament is live; keeping it. \
                 Remove {} by hand.",
                dir.display(),
                DELETED_MARKER
            );
            continue;
        }
        // `saturating_sub` so a marker stamped in the future (a clock moved
        // backwards) reads as age zero and is kept, never as expired.
        let age = Duration::from_secs(now.saturating_sub(marker.deleted_at));
        if age < retention {
            continue;
        }
        match fs::remove_dir_all(&dir) {
            Ok(()) => tracing::info!(
                "backup: removed the backups of \"{}\", deleted {} days ago",
                marker.name,
                age.as_secs() / 86_400
            ),
            Err(e) => tracing::warn!("backup: could not remove {}: {e}", dir.display()),
        }
    }
}

/// Why [`load`] could not hand back a tournament.
#[derive(Debug)]
pub(crate) enum LoadError {
    /// No backup with that id here.
    NotFound,
    /// The file is there, but this build cannot read it — an older format
    /// version, most often, which is exactly what a *backup* store accumulates
    /// across an upgrade. Carries the reason so the caller can say which.
    Unreadable(TournamentError),
}

/// Load a specific backup by id (its file stem) from `dir`.
///
/// Rejects anything that isn't a plain `<secs>-<seq>-<slug>` token
/// (alphanumerics and dashes only), so this can never escape the backups
/// directory.
///
/// A backup too old for this build is [`Unreadable`](LoadError::Unreadable),
/// not missing: "no such backup" for a file the referee is looking straight at
/// in the list is the kind of answer that sends someone hunting for a bug in
/// the wrong place. The bytes go through [`crate::save`] like any other
/// untrusted save — this store is a directory anything could have edited, and a
/// backup taken before an upgrade is exactly the file that needs the version
/// window applied to it.
pub(crate) fn load(dir: Option<&Path>, id: &str) -> Result<Tournament, LoadError> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(LoadError::NotFound);
    }
    let bytes = fs::read(dir.ok_or(LoadError::NotFound)?.join(format!("{id}.json")))
        .map_err(|_| LoadError::NotFound)?;
    crate::save::load(&bytes).map_err(LoadError::Unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root of this test's own, under the OS temp dir — so a `list` or a
    /// `sweep` here can never see (or delete) what a concurrently-running test
    /// is doing, and nothing lands in the real per-user data directory.
    fn isolated_root() -> PathBuf {
        std::env::temp_dir().join(format!("osp-backup-test-{}", Uuid::new_v4()))
    }

    /// A backups directory of this tournament's own, under a root of this
    /// test's own.
    fn dir_of(t: &Tournament) -> PathBuf {
        dir_for(&isolated_root(), t.id)
    }

    #[test]
    fn take_then_list_then_load_round_trips() {
        let t = Tournament::new("Backup Test").unwrap();
        let dir = dir_of(&t);
        take(Some(&dir), &t, "round 2 started");
        let backups = list(Some(&dir));
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].label, "round 2 started");

        let restored = load(Some(&dir), &backups[0].id).unwrap();
        assert_eq!(restored.id, t.id);
        assert_eq!(restored.name, "Backup Test");
    }

    #[test]
    fn list_is_newest_first() {
        let t = Tournament::new("Order Test").unwrap();
        let dir = dir_of(&t);
        take(Some(&dir), &t, "first");
        take(Some(&dir), &t, "second");
        take(Some(&dir), &t, "third");
        let backups = list(Some(&dir));
        assert_eq!(
            backups.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(),
            vec!["third", "second", "first"],
        );
    }

    #[test]
    fn rotation_keeps_only_the_newest_max_backups() {
        let t = Tournament::new("Rotation Test").unwrap();
        let dir = dir_of(&t);
        for i in 0..MAX_BACKUPS + 5 {
            take(Some(&dir), &t, &format!("step {i}"));
        }
        let backups = list(Some(&dir));
        assert_eq!(backups.len(), MAX_BACKUPS);
        // The newest one taken is still there; the earliest ones were rotated out.
        assert_eq!(backups[0].label, format!("step {}", MAX_BACKUPS + 4));
        assert!(backups.iter().all(|b| {
            let n: usize = b.label.trim_start_matches("step ").parse().unwrap();
            n >= 5 // steps 0..5 were rotated out
        }));
    }

    #[test]
    fn load_rejects_a_path_traversal_attempt() {
        let t = Tournament::new("Traversal Test").unwrap();
        let dir = dir_of(&t);
        take(Some(&dir), &t, "only backup");
        assert!(load(Some(&dir), "../../etc/passwd").is_err());
        assert!(load(Some(&dir), "nonexistent-0-id").is_err());
    }

    #[test]
    fn list_is_empty_for_a_tournament_with_no_backups() {
        let dir = dir_for(&isolated_root(), Uuid::new_v4());
        assert!(list(Some(&dir)).is_empty());
    }

    /// No directory at all (no known data directory on this platform) is a
    /// no-op everywhere rather than a panic or an invented location.
    #[test]
    fn no_directory_keeps_no_backups() {
        let t = Tournament::new("Homeless Test").unwrap();
        take(None, &t, "nowhere to go");
        assert!(list(None).is_empty());
        assert!(load(None, "0-0-nowhere-to-go").is_err());
        mark_deleted(None, "Homeless Test", None);
        sweep(None, Duration::ZERO, &HashSet::new());
    }

    /// Backdate a marker that [`mark_deleted`] just stamped with "now", so a
    /// sweep can be tested against a realistic retention instead of zero.
    fn backdate(dir: &Path, age: Duration) {
        let path = dir.join(DELETED_MARKER);
        let mut marker: DeletedMarker =
            serde_json::from_slice(&fs::read(&path).expect("a marker")).expect("a valid marker");
        marker.deleted_at -= age.as_secs();
        fs::write(&path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();
    }

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    #[test]
    fn marking_deleted_keeps_the_backups_and_records_the_password_hash() {
        let t = Tournament::new("Deleted Test").unwrap();
        let dir = dir_of(&t);
        take(Some(&dir), &t, "round 1 started");
        mark_deleted(Some(&dir), "Deleted Test", Some("$2b$hash"));

        // The backups are still there, and the marker is not one of them.
        let backups = list(Some(&dir));
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].label, "round 1 started");

        let marker: DeletedMarker =
            serde_json::from_slice(&fs::read(dir.join(DELETED_MARKER)).unwrap()).unwrap();
        assert_eq!(marker.name, "Deleted Test");
        assert_eq!(marker.password_hash.as_deref(), Some("$2b$hash"));
        assert!(marker.deleted_at > 0);
    }

    /// A tournament deleted before it was ever backed up has nothing to keep —
    /// don't leave an empty marked directory behind.
    #[test]
    fn marking_deleted_does_not_create_a_directory() {
        let dir = isolated_root().join("never-backed-up");
        mark_deleted(Some(&dir), "Never Backed Up", None);
        assert!(!dir.exists());
    }

    #[test]
    fn sweep_removes_expired_backups_and_keeps_the_rest() {
        let root = isolated_root();
        let expired = dir_for(&root, Uuid::new_v4());
        let recent = dir_for(&root, Uuid::new_v4());
        let live = dir_for(&root, Uuid::new_v4());
        let t = Tournament::new("Sweep Test").unwrap();
        for dir in [&expired, &recent, &live] {
            take(Some(dir), &t, "round 1 started");
        }
        mark_deleted(Some(&expired), "Expired", None);
        backdate(&expired, 40 * DAY);
        mark_deleted(Some(&recent), "Recent", None);
        backdate(&recent, 10 * DAY);
        // `live` is left unmarked: a tournament that still exists.

        sweep(Some(&root), 30 * DAY, &HashSet::new());

        assert!(!expired.exists(), "deleted 40 days ago, retention is 30");
        assert!(recent.exists(), "deleted 10 days ago, retention is 30");
        assert!(live.exists(), "not deleted at all");
        assert_eq!(list(Some(&recent)).len(), 1, "its backups are intact");

        fs::remove_dir_all(&root).ok();
    }

    /// A restore that could not remove the marker (see [`unmark_deleted`])
    /// leaves a live tournament's directory looking deleted. The sweep must
    /// still not touch it: a stale bin entry is a nuisance, deleting a running
    /// tournament's backups is not.
    #[test]
    fn sweep_never_touches_a_live_tournament_even_when_marked() {
        let root = isolated_root();
        let id = Uuid::new_v4();
        let dir = dir_for(&root, id);
        let t = Tournament::new("Still Live").unwrap();
        take(Some(&dir), &t, "round 1 started");
        mark_deleted(Some(&dir), "Still Live", None);

        sweep(Some(&root), Duration::ZERO, &HashSet::from([id]));
        assert!(dir.exists());
        assert_eq!(list(Some(&dir)).len(), 1);

        // Once it is no longer live, the same sweep clears it.
        sweep(Some(&root), Duration::ZERO, &HashSet::new());
        assert!(!dir.exists());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unmarking_makes_a_directory_ordinary_again() {
        let root = isolated_root();
        let dir = dir_for(&root, Uuid::new_v4());
        let t = Tournament::new("Restored Test").unwrap();
        take(Some(&dir), &t, "round 1 started");
        mark_deleted(Some(&dir), "Restored Test", None);
        assert!(unmark_deleted(&dir));

        // No marker: not in the bin, not sweepable, backups intact — exactly a
        // tournament that was never deleted.
        assert!(read_marker(&dir).is_none());
        assert!(list_deleted(Some(&root)).is_empty());
        sweep(Some(&root), Duration::ZERO, &HashSet::new());
        assert_eq!(list(Some(&dir)).len(), 1);

        // And unmarking what isn't marked is the state we wanted, not an error.
        assert!(unmark_deleted(&dir));

        fs::remove_dir_all(&root).ok();
    }

    /// The marker is the only thing that makes a directory sweepable — an
    /// unreadable one keeps its backups rather than costing someone a
    /// tournament.
    #[test]
    fn sweep_keeps_a_directory_whose_marker_is_corrupt() {
        let root = isolated_root();
        let dir = dir_for(&root, Uuid::new_v4());
        let t = Tournament::new("Corrupt Marker Test").unwrap();
        take(Some(&dir), &t, "round 1 started");
        fs::write(dir.join(DELETED_MARKER), b"{ not json").unwrap();

        sweep(Some(&root), Duration::ZERO, &HashSet::new());

        assert!(dir.exists());
        assert_eq!(list(Some(&dir)).len(), 1);

        fs::remove_dir_all(&root).ok();
    }
}
