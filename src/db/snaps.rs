use crate::db::schema;
use crate::error::{GError, GResult};
use crate::hashing::Hash;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Snap { pub snapshot_id: String, pub parent_snap_id: Option<String>, pub timestamp: i64, pub message: Option<String>, pub file_count: i64, pub added_size: i64 }
#[derive(Debug, Clone)]
pub struct FileEntry { pub file_path: String, pub hash: Hash, pub file_size: i64, pub modified_time: i64 }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta { pub hash: Hash, pub file_size: i64, pub modified_time: i64 }
#[derive(Debug, Clone)]
pub struct Branch { pub name: String, pub snapshot_id: String, pub created_at: i64 }
#[derive(Debug, Default, Clone)]
pub struct Diff { pub added: Vec<FileEntry>, pub modified: Vec<FileEntry>, pub deleted: Vec<String>, pub unchanged: Vec<FileEntry> }
impl Diff {
    pub fn total_changes(&self) -> usize { self.added.len() + self.modified.len() + self.deleted.len() }
    pub fn added_size(&self) -> i64 { self.added.iter().map(|f| f.file_size).sum::<i64>() + self.modified.iter().map(|f| f.file_size).sum::<i64>() }
}

pub struct SnapsDb { conn: Connection }
impl SnapsDb {
    pub fn open(path: &Path) -> GResult<Self> {
        if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
        let conn = Connection::open(path)?;
        crate::db::apply_pragmas(&conn)?;
        schema::init_snaps_db(&conn)?;
        let ok: Option<String> = conn.query_row("PRAGMA integrity_check;", [], |r| r.get(0)).optional()?;
        if ok.as_deref() != Some("ok") { return Err(GError::DbCorrupt(path.to_path_buf())); }
        Ok(Self { conn })
    }
    pub fn latest_snapshot(&self) -> GResult<Option<Snap>> {
        let mut stmt = self.conn.prepare("SELECT snapshotId, parentSnapId, timestamp, message, fileCount, addedSize FROM snaps ORDER BY timestamp DESC, rowid DESC LIMIT 1")?;
        stmt.query_row([], |r| Ok(Snap { snapshot_id: r.get(0)?, parent_snap_id: r.get(1)?, timestamp: r.get(2)?, message: r.get(3)?, file_count: r.get(4)?, added_size: r.get(5)? })).optional().map_err(GError::Sqlite)
    }
    pub fn get_snapshot(&self, id: &str) -> GResult<Option<Snap>> {
        let mut stmt = self.conn.prepare("SELECT snapshotId, parentSnapId, timestamp, message, fileCount, addedSize FROM snaps WHERE snapshotId = ?1")?;
        stmt.query_row(params![id], |r| Ok(Snap { snapshot_id: r.get(0)?, parent_snap_id: r.get(1)?, timestamp: r.get(2)?, message: r.get(3)?, file_count: r.get(4)?, added_size: r.get(5)? })).optional().map_err(GError::Sqlite)
    }
    pub fn list_snapshots(&self) -> GResult<Vec<Snap>> {
        let mut stmt = self.conn.prepare("SELECT snapshotId, parentSnapId, timestamp, message, fileCount, addedSize FROM snaps ORDER BY timestamp DESC, rowid DESC")?;
        let rows = stmt.query_map([], |r| Ok(Snap { snapshot_id: r.get(0)?, parent_snap_id: r.get(1)?, timestamp: r.get(2)?, message: r.get(3)?, file_count: r.get(4)?, added_size: r.get(5)? }))?;
        let mut out = Vec::new(); for r in rows { out.push(r?); } Ok(out)
    }
    pub fn children_of(&self, pid: &str) -> GResult<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT snapshotId FROM snaps WHERE parentSnapId = ?1")?;
        let rows = stmt.query_map(params![pid], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new(); for r in rows { out.push(r?); } Ok(out)
    }
    pub fn files_for_snapshot(&self, sid: &str) -> GResult<HashMap<String, FileMeta>> {
        let mut stmt = self.conn.prepare("SELECT filePath, hash, fileSize, modifiedTime FROM files WHERE snapshotId = ?1")?;
        let rows = stmt.query_map(params![sid], |r| Ok((r.get::<_, String>(0)?, FileMeta { hash: Hash(r.get::<_, String>(1)?), file_size: r.get::<_, i64>(2)?, modified_time: r.get::<_, i64>(3)? })))?;
        let mut out = HashMap::new(); for r in rows { let (p, m) = r?; out.insert(p, m); } Ok(out)
    }
    pub fn referenced_hashes(&self) -> GResult<std::collections::HashSet<String>> {
        let mut stmt = self.conn.prepare("SELECT DISTINCT hash FROM files")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = std::collections::HashSet::new(); for r in rows { out.insert(r?); } Ok(out)
    }
    pub fn transaction(&mut self) -> GResult<rusqlite::Transaction<'_>> { Ok(self.conn.transaction()?) }
    pub fn insert_snap(tx: &rusqlite::Transaction<'_>, sid: &str, pid: Option<&str>, ts: i64, msg: Option<&str>, fc: i64, asz: i64) -> GResult<()> {
        tx.execute("INSERT INTO snaps (snapshotId, parentSnapId, timestamp, message, fileCount, addedSize) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![sid, pid, ts, msg, fc, asz])?; Ok(())
    }
    pub fn insert_files(tx: &rusqlite::Transaction<'_>, sid: &str, files: &[FileEntry]) -> GResult<()> {
        // Use prepare_cached for statement reuse — avoids re-parsing
        // SQL on every row. The statement is compiled once and reused
        // across all execute() calls in this loop.
        let mut stmt = tx.prepare_cached("INSERT INTO files (snapshotId, filePath, hash, fileSize, modifiedTime) VALUES (?1, ?2, ?3, ?4, ?5)")?;
        for f in files {
            stmt.execute(params![sid, f.file_path, f.hash.as_str(), f.file_size, f.modified_time])?;
        }
        Ok(())
    }

    pub fn insert_deleted_files(tx: &rusqlite::Transaction<'_>, sid: &str, files: &[String]) -> GResult<()> {
        let mut stmt = tx.prepare_cached("INSERT INTO deleted_files (snapshotId, filePath) VALUES (?1, ?2)")?;
        for f in files {
            stmt.execute(params![sid, f])?;
        }
        Ok(())
    }
    pub fn list_branches(&self) -> GResult<Vec<Branch>> {
        let mut stmt = self.conn.prepare("SELECT name, snapshotId, createdAt FROM branches ORDER BY name ASC")?;
        let rows = stmt.query_map([], |r| Ok(Branch { name: r.get(0)?, snapshot_id: r.get(1)?, created_at: r.get(2)? }))?;
        let mut out = Vec::new(); for r in rows { out.push(r?); } Ok(out)
    }
    pub fn get_branch(&self, name: &str) -> GResult<Option<Branch>> {
        let mut stmt = self.conn.prepare("SELECT name, snapshotId, createdAt FROM branches WHERE name = ?1")?;
        stmt.query_row(params![name], |r| Ok(Branch { name: r.get(0)?, snapshot_id: r.get(1)?, created_at: r.get(2)? })).optional().map_err(GError::Sqlite)
    }
    pub fn insert_branch(&self, name: &str, sid: &str) -> GResult<()> {
        self.conn.execute("INSERT INTO branches (name, snapshotId) VALUES (?1, ?2)", params![name, sid]).map_err(|e| match e {
            rusqlite::Error::SqliteFailure(err, _) if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY => GError::BranchExists(name.to_string(), String::new()),
            o => GError::Sqlite(o),
        })?; Ok(())
    }
    pub fn delete_branch(&self, name: &str) -> GResult<bool> { Ok(self.conn.execute("DELETE FROM branches WHERE name = ?1", params![name])? > 0) }
    pub fn branches_for_snapshot(&self, sid: &str) -> GResult<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT name FROM branches WHERE snapshotId = ?1 ORDER BY name")?;
        let rows = stmt.query_map(params![sid], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new(); for r in rows { out.push(r?); } Ok(out)
    }
    pub fn get_meta(&self, k: &str) -> GResult<Option<String>> {
        let mut stmt = self.conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
        stmt.query_row(params![k], |r| r.get::<_, String>(0)).optional().map_err(GError::Sqlite)
    }
    pub fn set_meta(&self, k: &str, v: &str) -> GResult<()> {
        self.conn.execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![k, v])?; Ok(())
    }
    pub fn get_current_branch(&self) -> GResult<Option<Branch>> {
        let name = self.get_meta("current_branch")?;
        match name {
            None => Ok(None),
            Some(n) if n.is_empty() => Ok(None),
            Some(n) => match self.get_branch(&n)? {
                Some(b) => Ok(Some(b)),
                None => Err(GError::Other(format!("current_branch points to missing branch \"{n}\""))),
            },
        }
    }
    pub fn set_current_branch(&self, n: &str) -> GResult<()> { self.set_meta("current_branch", n) }
    pub fn ensure_main_branch(&self) -> GResult<()> {
        if self.get_branch("main")?.is_none() && self.latest_snapshot()?.is_some() {
            self.insert_branch("main", &self.latest_snapshot()?.unwrap().snapshot_id)?;
        }
        if self.get_meta("current_branch")?.is_none() && self.get_branch("main")?.is_some() { self.set_current_branch("main")?; }
        Ok(())
    }
    pub fn now_ms() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0) }
}

pub fn diff_states(parent: &HashMap<String, FileMeta>, current: &HashMap<String, FileMeta>) -> Diff {
    let mut d = Diff::default();
    for (p, m) in current {
        match parent.get(p) {
            None => d.added.push(FileEntry { file_path: p.clone(), hash: m.hash.clone(), file_size: m.file_size, modified_time: m.modified_time }),
            Some(pm) if pm.hash != m.hash => d.modified.push(FileEntry { file_path: p.clone(), hash: m.hash.clone(), file_size: m.file_size, modified_time: m.modified_time }),
            Some(_) => d.unchanged.push(FileEntry { file_path: p.clone(), hash: m.hash.clone(), file_size: m.file_size, modified_time: m.modified_time }),
        }
    }
    for p in parent.keys() { if !current.contains_key(p) { d.deleted.push(p.clone()); } }
    d
}
