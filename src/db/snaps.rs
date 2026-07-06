//! `snaps.db` — per-game snapshot & file metadata.
//!
//! Each game has its own `snaps.db` in `data/[alias]/`. This module
//! wraps all queries: snapshot creation, file-table inserts, diffs
//! against the parent snapshot, history traversal, and integrity check.

use crate::db::schema;
use crate::error::{GError, GResult};
use crate::hashing::Hash;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// A row in the `snaps` table.
#[derive(Debug, Clone)]
pub struct Snap {
    pub snapshot_id: String,
    pub parent_snap_id: Option<String>,
    pub timestamp: i64,
    pub message: Option<String>,
    pub file_count: i64,
    pub added_size: i64,
}

/// A single file entry — used for inserts and queries.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub file_path: String,
    pub hash: Hash,
    pub file_size: i64,
}

/// Result of a diff between two snapshots (or between a snapshot and
/// the current on-disk state).
#[derive(Debug, Default, Clone)]
pub struct Diff {
    /// Files present in the new state but not in the old state.
    pub added: Vec<FileEntry>,
    /// Files present in both but with different hash (modified).
    pub modified: Vec<FileEntry>,
    /// Files present in the old state but not in the new state (deleted).
    pub deleted: Vec<String>,
    /// Files present in both with the same hash.
    pub unchanged: Vec<FileEntry>,
}

impl Diff {
    pub fn total_changes(&self) -> usize {
        self.added.len() + self.modified.len() + self.deleted.len()
    }

    pub fn added_size(&self) -> i64 {
        // For "added size" per spec we count new + modified file sizes.
        self.added.iter().map(|f| f.file_size).sum::<i64>()
            + self.modified.iter().map(|f| f.file_size).sum::<i64>()
    }
}

/// Handle to an open `snaps.db` connection.
pub struct SnapsDb {
    conn: Connection,
}

impl SnapsDb {
    /// Open `snaps.db`, creating the schema if needed, and run an
    /// integrity check. Returns [`GError::DbCorrupt`] if the database
    /// is corrupted.
    pub fn open(path: &Path) -> GResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        crate::db::apply_pragmas(&conn)?;
        schema::init_snaps_db(&conn)?;

        // Best-effort integrity check. Failures are reported but do not
        // block opening — the user can run `g repair` explicitly.
        let ok: Option<String> = conn
            .query_row("PRAGMA integrity_check;", [], |r| r.get(0))
            .optional()?;
        if ok.as_deref() != Some("ok") {
            return Err(GError::DbCorrupt(path.to_path_buf()));
        }
        Ok(Self { conn })
    }

    /// Get the latest snapshot (by timestamp, falling back to insertion
    /// order if equal). Returns `Ok(None)` if no snapshots exist.
    pub fn latest_snapshot(&self) -> GResult<Option<Snap>> {
        let mut stmt = self.conn.prepare(
            "SELECT snapshotId, parentSnapId, timestamp, message, fileCount, addedSize
             FROM snaps
             ORDER BY timestamp DESC, rowid DESC
             LIMIT 1",
        )?;
        let snap = stmt
            .query_row([], |r| {
                Ok(Snap {
                    snapshot_id: r.get(0)?,
                    parent_snap_id: r.get(1)?,
                    timestamp: r.get(2)?,
                    message: r.get(3)?,
                    file_count: r.get(4)?,
                    added_size: r.get(5)?,
                })
            })
            .optional()?;
        Ok(snap)
    }

    /// Look up a snapshot by ID.
    pub fn get_snapshot(&self, id: &str) -> GResult<Option<Snap>> {
        let mut stmt = self.conn.prepare(
            "SELECT snapshotId, parentSnapId, timestamp, message, fileCount, addedSize
             FROM snaps WHERE snapshotId = ?1",
        )?;
        let snap = stmt
            .query_row(params![id], |r| {
                Ok(Snap {
                    snapshot_id: r.get(0)?,
                    parent_snap_id: r.get(1)?,
                    timestamp: r.get(2)?,
                    message: r.get(3)?,
                    file_count: r.get(4)?,
                    added_size: r.get(5)?,
                })
            })
            .optional()?;
        Ok(snap)
    }

    /// List all snapshots, newest first.
    pub fn list_snapshots(&self) -> GResult<Vec<Snap>> {
        let mut stmt = self.conn.prepare(
            "SELECT snapshotId, parentSnapId, timestamp, message, fileCount, addedSize
             FROM snaps
             ORDER BY timestamp DESC, rowid DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Snap {
                snapshot_id: r.get(0)?,
                parent_snap_id: r.get(1)?,
                timestamp: r.get(2)?,
                message: r.get(3)?,
                file_count: r.get(4)?,
                added_size: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Get every file entry for a snapshot, as a `HashMap` keyed by
    /// normalized file path. Used for diffing.
    pub fn files_for_snapshot(&self, snapshot_id: &str) -> GResult<HashMap<String, (Hash, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT filePath, hash, fileSize FROM files WHERE snapshotId = ?1")?;
        let rows = stmt.query_map(params![snapshot_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (Hash(r.get::<_, String>(1)?), r.get::<_, i64>(2)?),
            ))
        })?;
        let mut out = HashMap::new();
        for r in rows {
            let (p, h) = r?;
            out.insert(p, h);
        }
        Ok(out)
    }

    /// Returns the set of all distinct hashes referenced by any
    /// snapshot. Used by `g gc` to find orphaned objects.
    pub fn referenced_hashes(&self) -> GResult<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT hash FROM files")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = std::collections::HashSet::new();
        for r in rows {
            out.insert(r?);
        }
        Ok(out)
    }

    /// Begin a transaction. The caller is responsible for committing
    /// or rolling back.
    pub fn transaction(&mut self) -> GResult<rusqlite::Transaction<'_>> {
        Ok(self.conn.transaction()?)
    }

    /// Insert a snapshot record. Used by the snap command inside a
    /// larger transaction.
    pub fn insert_snap(
        tx: &rusqlite::Transaction<'_>,
        snapshot_id: &str,
        parent_snap_id: Option<&str>,
        timestamp: i64,
        message: Option<&str>,
        file_count: i64,
        added_size: i64,
    ) -> GResult<()> {
        tx.execute(
            "INSERT INTO snaps (snapshotId, parentSnapId, timestamp, message, fileCount, addedSize)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                snapshot_id,
                parent_snap_id,
                timestamp,
                message,
                file_count,
                added_size,
            ],
        )?;
        Ok(())
    }

    /// Bulk-insert file entries for a snapshot. Caller must hold a
    /// transaction.
    pub fn insert_files(
        tx: &rusqlite::Transaction<'_>,
        snapshot_id: &str,
        files: &[FileEntry],
    ) -> GResult<()> {
        let mut stmt = tx.prepare(
            "INSERT INTO files (snapshotId, filePath, hash, fileSize) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for f in files {
            stmt.execute(params![snapshot_id, f.file_path, f.hash.as_str(), f.file_size])?;
        }
        Ok(())
    }

    /// Bulk-insert deleted-file entries for a snapshot.
    pub fn insert_deleted_files(
        tx: &rusqlite::Transaction<'_>,
        snapshot_id: &str,
        files: &[String],
    ) -> GResult<()> {
        let mut stmt = tx.prepare(
            "INSERT INTO deleted_files (snapshotId, filePath) VALUES (?1, ?2)",
        )?;
        for f in files {
            stmt.execute(params![snapshot_id, f])?;
        }
        Ok(())
    }

    /// Convenience: current time in milliseconds since UNIX_EPOCH.
    pub fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Compute the diff between a parent snapshot's file map and the
/// current state map. Pure function — no DB access.
pub fn diff_states(
    parent: &HashMap<String, (Hash, i64)>,
    current: &HashMap<String, (Hash, i64)>,
) -> Diff {
    let mut d = Diff::default();
    for (path, (hash, size)) in current {
        match parent.get(path) {
            None => d.added.push(FileEntry {
                file_path: path.clone(),
                hash: hash.clone(),
                file_size: *size,
            }),
            Some((ph, _)) if ph != hash => d.modified.push(FileEntry {
                file_path: path.clone(),
                hash: hash.clone(),
                file_size: *size,
            }),
            Some(_) => d.unchanged.push(FileEntry {
                file_path: path.clone(),
                hash: hash.clone(),
                file_size: *size,
            }),
        }
    }
    for path in parent.keys() {
        if !current.contains_key(path) {
            d.deleted.push(path.clone());
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp() -> PathBuf {
        let f = tempfile::NamedTempFile::new().unwrap();
        f.into_temp_path().keep().unwrap()
    }

    #[test]
    fn empty_db_has_no_snapshots() {
        let p = tmp();
        let db = SnapsDb::open(&p).unwrap();
        assert!(db.latest_snapshot().unwrap().is_none());
        assert!(db.list_snapshots().unwrap().is_empty());
    }

    #[test]
    fn diff_added_modified_unchanged_deleted() {
        let mut parent = HashMap::new();
        parent.insert("a.txt".into(), (Hash("aaaa".into()), 10));
        parent.insert("b.txt".into(), (Hash("bbbb".into()), 20));
        parent.insert("c.txt".into(), (Hash("cccc".into()), 30));

        let mut current = HashMap::new();
        current.insert("a.txt".into(), (Hash("aaaa".into()), 10)); // unchanged
        current.insert("b.txt".into(), (Hash("BBBB".into()), 20)); // modified
        current.insert("d.txt".into(), (Hash("dddd".into()), 40)); // added
        // c.txt is deleted

        let d = diff_states(&parent, &current);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.modified.len(), 1);
        assert_eq!(d.unchanged.len(), 1);
        assert_eq!(d.deleted.len(), 1);
    }
}
