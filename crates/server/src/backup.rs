//! Automatic server-side backups of the tournament.
//!
//! Taken at each round state-machine transition (registration finalized, a
//! round prepared/confirmed/completed/cancelled) — coarser than the undo
//! stack, which snapshots on every single mutation. These live on disk so a
//! referee can recover from a multi-step mistake (or a server restart)
//! without needing a manual save. One rotating directory per tournament id;
//! the oldest backups beyond [`MAX_BACKUPS`] are deleted after each write.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use osp_core::Tournament;
use serde::Serialize;
use uuid::Uuid;

/// How many backups are kept per tournament before the oldest are rotated out.
/// A plain constant "for now" — easy to turn into a user-facing setting later.
pub const MAX_BACKUPS: usize = 10;

/// One backup's metadata, as listed to clients (the tournament body itself is
/// only fetched on restore).
#[derive(Serialize)]
pub struct BackupInfo {
    /// Opaque id used to restore this backup (its file stem).
    pub id: String,
    /// Unix seconds when the backup was taken.
    pub taken_at: u64,
    /// Which transition triggered it, e.g. "round 2 started".
    pub label: String,
}

fn backups_dir(tournament_id: Uuid) -> Option<PathBuf> {
    // Under `cargo test`, divert to the OS temp dir instead of the real
    // per-user data directory — every round-lifecycle test in this crate
    // exercises the endpoints that call `take`, and none of them should touch
    // the developer's actual OpenShogiPairings data.
    let root = if cfg!(test) {
        std::env::temp_dir().join("osp-test-backups")
    } else {
        dirs::data_dir()?.join("openshogipairings").join("backups")
    };
    Some(root.join(tournament_id.to_string()))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
pub fn take(tournament: &Tournament, label: &str) {
    let Some(dir) = backups_dir(tournament.id) else {
        tracing::warn!("backup: could not determine the data directory");
        return;
    };
    if let Err(e) = fs::create_dir_all(&dir) {
        tracing::warn!("backup: could not create {}: {e}", dir.display());
        return;
    }
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{}-{seq}-{}.json", now_secs(), slug(label)));
    match serde_json::to_vec_pretty(tournament) {
        Ok(json) => {
            if let Err(e) = fs::write(&path, json) {
                tracing::warn!("backup: could not write {}: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("backup: could not serialize the tournament: {e}"),
    }
    rotate(&dir);
}

/// Delete the oldest backups in `dir` beyond [`MAX_BACKUPS`], oldest first by
/// the `(secs, seq)` encoded in each filename — *not* a plain lexicographic
/// sort, since `seq` isn't zero-padded ("10" would otherwise sort before "9").
fn rotate(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
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
            let _ = fs::remove_file(path);
        }
    }
}

/// List backups for `tournament_id`, newest first.
pub fn list(tournament_id: Uuid) -> Vec<BackupInfo> {
    let Some(dir) = backups_dir(tournament_id) else { return Vec::new() };
    let Ok(entries) = fs::read_dir(&dir) else { return Vec::new() };
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
                BackupInfo { taken_at: secs, label, id: stem },
            ))
        })
        .collect();
    infos.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    infos.into_iter().map(|(_, _, info)| info).collect()
}

/// Load a specific backup by id (its file stem), if it exists for this
/// tournament. Rejects anything that isn't a plain `<secs>-<seq>-<slug>`
/// token (alphanumerics and dashes only), so this can never escape the
/// backups directory.
pub fn load(tournament_id: Uuid, id: &str) -> Option<Tournament> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let dir = backups_dir(tournament_id)?;
    let bytes = fs::read(dir.join(format!("{id}.json"))).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_then_list_then_load_round_trips() {
        let t = Tournament::new("Backup Test").unwrap();
        take(&t, "round 2 started");
        let backups = list(t.id);
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].label, "round 2 started");

        let restored = load(t.id, &backups[0].id).unwrap();
        assert_eq!(restored.id, t.id);
        assert_eq!(restored.name, "Backup Test");
    }

    #[test]
    fn list_is_newest_first() {
        let t = Tournament::new("Order Test").unwrap();
        take(&t, "first");
        take(&t, "second");
        take(&t, "third");
        let backups = list(t.id);
        assert_eq!(
            backups.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(),
            vec!["third", "second", "first"],
        );
    }

    #[test]
    fn rotation_keeps_only_the_newest_max_backups() {
        let t = Tournament::new("Rotation Test").unwrap();
        for i in 0..MAX_BACKUPS + 5 {
            take(&t, &format!("step {i}"));
        }
        let backups = list(t.id);
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
        take(&t, "only backup");
        assert!(load(t.id, "../../etc/passwd").is_none());
        assert!(load(t.id, "nonexistent-0-id").is_none());
    }

    #[test]
    fn list_is_empty_for_a_tournament_with_no_backups() {
        let random_id = Uuid::new_v4();
        assert!(list(random_id).is_empty());
    }
}
