//! Schema definitions and migration logic for both databases.
//!
//! Per spec, the schema is fixed (no migrations needed for v0.1). We
//! define the DDL here so that `add` and `repair` can both call into
//! the same code path.

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

/// Apply the `snaps.db` schema. Idempotent.
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

        CREATE INDEX IF NOT EXISTS idx_files_hash ON files (hash);
        CREATE INDEX IF NOT EXISTS idx_files_snapshot ON files (snapshotId);

        CREATE TABLE IF NOT EXISTS deleted_files (
            snapshotId   TEXT    NOT NULL,
            filePath     TEXT    NOT NULL,
            PRIMARY KEY (snapshotId, filePath)
        );

        CREATE INDEX IF NOT EXISTS idx_deleted_snapshot ON deleted_files (snapshotId);
        "#,
    )?;
    Ok(())
}
