//! Error types for `gim`.
//!
//! All public APIs return `Result<T, GError>` (or `anyhow::Result` at the
//! binary boundary). Internal modules use the strongly-typed [`GError`]
//! enum so that the CLI layer can map errors to the exact user-facing
//! messages defined in the spec.

use std::io;
use std::path::PathBuf;
use thiserror::Error;

/// Top-level error type used by every internal module.
#[derive(Debug, Error)]
pub enum GError {
    #[error("game with alias \"{0}\" already exists")]
    AliasExists(String),

    #[error("game with alias \"{0}\" does not exist")]
    AliasNotFound(String),

    #[error("game directory \"{0}\" does not exist")]
    GameDirMissing(PathBuf),

    #[error("game directory \"{0}\" is not a directory")]
    GameDirNotDir(PathBuf),

    #[error("data directory \"{0}\" is not a directory")]
    DataDirNotDir(PathBuf),

    #[error("snapshot \"{0}\" does not exist for game \"{1}\"")]
    SnapshotNotFound(String, String),

    #[error("snapshot id \"{0}\" already exists for game \"{1}\"")]
    SnapshotIdExists(String, String),

    #[error("invalid snapshot id \"{0}\": must match ^[A-Za-z0-9._-]+$ and not start with a dot")]
    InvalidSnapshotId(String),

    #[error("no snapshots exist for game \"{0}\" — run `gim snap {0}` first")]
    NoSnapshots(String),

    #[error("\"{0}\" is locked by another operation (if this is a mistake, delete the lockfile at {1})")]
    Locked(String, PathBuf),

    #[error("database corruption detected in {0} — run `gim repair` to attempt recovery")]
    DbCorrupt(PathBuf),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("path error: {0}")]
    Path(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("ignore pattern error in {file}: {message}")]
    IgnorePattern { file: PathBuf, message: String },

    #[error("hashing error: {0}")]
    Hashing(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("ignore-pattern error: {0}")]
    Ignore(#[from] ignore::Error),

    #[error("filetime error: {0}")]
    FileTime(String),

    #[error("operation cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

/// Convenience type alias used throughout the codebase.
pub type GResult<T> = Result<T, GError>;

/// Convert a [`GError`] into an exit code for the CLI process.
pub fn exit_code(err: &GError) -> i32 {
    match err {
        GError::AliasExists(_)
        | GError::AliasNotFound(_)
        | GError::GameDirMissing(_)
        | GError::GameDirNotDir(_)
        | GError::DataDirNotDir(_)
        | GError::SnapshotNotFound(_, _)
        | GError::SnapshotIdExists(_, _)
        | GError::InvalidSnapshotId(_)
        | GError::NoSnapshots(_)
        | GError::Locked(_, _)
        | GError::DbCorrupt(_)
        | GError::IgnorePattern { .. }
        | GError::Path(_)
        | GError::Config(_) => 2,
        // Fatal/unexpected errors
        GError::Sqlite(_)
        | GError::Io(_)
        | GError::Hashing(_)
        | GError::Json(_)
        | GError::Ignore(_)
        | GError::FileTime(_)
        | GError::Other(_)
        | GError::Cancelled => 1,
    }
}
