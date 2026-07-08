//! Schema definitions and migration logic.

use crate::error::GResult;
use rusqlite::Connection;

pub fn init_games_db(conn: &Connection) -> GResult<()> {
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS games (
            id INTEGER PRIMARY KEY AUTOINCREMENT, alias TEXT UNIQUE NOT NULL,
            title TEXT NOT NULL, gameDir TEXT NOT NULL, dataDir TEXT NOT NULL,
            addedAt INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS idx_games_alias ON games (alias);
    "#)?;
    Ok(())
}

pub fn init_snaps_db(conn: &Connection) -> GResult<()> {
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS snaps (
            snapshotId TEXT PRIMARY KEY, parentSnapId TEXT, timestamp INTEGER NOT NULL,
            message TEXT DEFAULT NULL, fileCount INTEGER NOT NULL DEFAULT 0, addedSize INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS files (
            snapshotId TEXT NOT NULL, filePath TEXT NOT NULL, hash TEXT NOT NULL, fileSize INTEGER NOT NULL,
            PRIMARY KEY (snapshotId, filePath)
        );
        CREATE TABLE IF NOT EXISTS deleted_files (
            snapshotId TEXT NOT NULL, filePath TEXT NOT NULL, PRIMARY KEY (snapshotId, filePath)
        );
        CREATE INDEX IF NOT EXISTS idx_files_hash ON files (hash);
        CREATE INDEX IF NOT EXISTS idx_files_snapshot ON files (snapshotId);
        CREATE INDEX IF NOT EXISTS idx_deleted_snapshot ON deleted_files (snapshotId);
    "#)?;
    migrate_add_modified_time(conn)?;
    migrate_add_branches_and_meta(conn)?;
    Ok(())
}

fn migrate_add_modified_time(conn: &Connection) -> GResult<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(files);")?;
    let has_col: bool = stmt.query_map([], |r| r.get::<_, String>(1))?.filter_map(|r| r.ok()).any(|name| name == "modifiedTime");
    if !has_col {
        conn.execute_batch("ALTER TABLE files ADD COLUMN modifiedTime INTEGER NOT NULL DEFAULT 0;")?;
        log::info!("migration: added modifiedTime column");
    }
    Ok(())
}

fn migrate_add_branches_and_meta(conn: &Connection) -> GResult<()> {
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS branches (name TEXT PRIMARY KEY, snapshotId TEXT NOT NULL, createdAt INTEGER NOT NULL DEFAULT (unixepoch()));
        CREATE INDEX IF NOT EXISTS idx_branches_snapshot ON branches (snapshotId);
        CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    "#)?;
    let snap_count: i64 = conn.query_row("SELECT COUNT(*) FROM snaps;", [], |r| r.get(0))?;
    let branch_count: i64 = conn.query_row("SELECT COUNT(*) FROM branches;", [], |r| r.get(0))?;
    if snap_count > 0 && branch_count == 0 {
        let latest: Option<String> = conn.query_row("SELECT snapshotId FROM snaps ORDER BY timestamp DESC, rowid DESC LIMIT 1;", [], |r| r.get(0)).ok();
        if let Some(latest_id) = latest {
            conn.execute("INSERT INTO branches (name, snapshotId) VALUES ('main', ?1)", rusqlite::params![latest_id])?;
            conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('current_branch', 'main')", [])?;
            log::info!("migration: auto-created 'main' branch → {latest_id}");
        }
    }
    Ok(())
}
