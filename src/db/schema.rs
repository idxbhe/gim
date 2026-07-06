//! Schema definitions and migration logic for both databases.

use crate::error::GResult;
use rusqlite::Connection;

/// Apply the `games.db` schema. Idempotent.
pub fn init_games_db(conn: &Connection) -> GResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS games (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            alias      TEXT    UNIQUE NOT NULL,
            title      TEXT    NOT NULL,
            gameDir    TEXT    NOT NULL,
            dataDir    TEXT    NOT NULL,
            addedAt    INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS idx_games_alias ON games (alias);
        "#,
    )?;
    Ok(())
}

/// Apply the `snaps.db` schema. Idempotent. Also runs migrations to
/// add the `modifiedTime` column to legacy databases.
pub fn init_snaps_db(conn: &Connection) -> GResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS snaps (
            snapshotId   TEXT    PRIMARY KEY,
            parentSnapId TEXT,
            timestamp    INTEGER NOT NULL,
            message      TEXT    DEFAULT NULL,
            fileCount    INTEGER NOT NULL DEFAULT 0,
            addedSize    INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS files (
            snapshotId   TEXT    NOT NULL,
            filePath     TEXT    NOT NULL,
            hash         TEXT    NOT NULL,
            fileSize     INTEGER NOT NULL,
            PRIMARY KEY (snapshotId, filePath)
        );

        CREATE TABLE IF NOT EXISTS deleted_files (
            snapshotId   TEXT    NOT NULL,
            filePath     TEXT    NOT NULL,
            PRIMARY KEY (snapshotId, filePath)
        );

        CREATE INDEX IF NOT EXISTS idx_files_hash ON files (hash);
        CREATE INDEX IF NOT EXISTS idx_files_snapshot ON files (snapshotId);
        CREATE INDEX IF NOT EXISTS idx_deleted_snapshot ON deleted_files (snapshotId);
        "#,
    )?;

    // Migration: add modifiedTime column to files (v0.2 schema change).
    // We use PRAGMA table_info to check if the column already exists,
    // since ALTER TABLE ADD COLUMN fails on existing columns.
    migrate_add_modified_time(conn)?;

    Ok(())
}

/// Migration: add `modifiedTime INTEGER NOT NULL DEFAULT 0` column to
/// the `files` table. Runs only on databases created by gim v0.1.
fn migrate_add_modified_time(conn: &Connection) -> GResult<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(files);")?;
    let has_col: bool = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "modifiedTime");
    if !has_col {
        conn.execute_batch(
            "ALTER TABLE files ADD COLUMN modifiedTime INTEGER NOT NULL DEFAULT 0;",
        )?;
        log::info!("migration: added modifiedTime column to files table");
    }
    Ok(())
}
