//! Database layer.
//!
//! Two SQLite databases per the spec:
//! - `data/games.db` — global game registry (single, shared).
//! - `data/[alias]/snaps.db` — per-game snapshot & file metadata.
//!
//! Both databases use WAL mode for better concurrent read performance.
//! Foreign keys are enabled on every connection. The `snaps.db`
//! connection also runs `PRAGMA integrity_check` on first open.

pub mod games;
pub mod schema;
pub mod snaps;

pub use games::GamesDb;
pub use snaps::{diff_states, Diff, FileEntry, Snap, SnapsDb};

/// Apply the standard set of SQLite pragmas to a fresh connection.
/// These are shared by both `games.db` and `snaps.db`.
pub(crate) fn apply_pragmas(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5_000)?;
    Ok(())
}
