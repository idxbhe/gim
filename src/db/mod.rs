//! Database layer.

pub mod games;
pub mod schema;
pub mod snaps;

pub use games::GamesDb;
pub use snaps::{diff_states, Branch, Diff, FileEntry, FileMeta, Snap, SnapsDb};

pub(crate) fn apply_pragmas(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5_000)?;
    Ok(())
}
