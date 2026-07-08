//! Error types for `gim`.

use std::io;
use std::path::PathBuf;
use thiserror::Error;

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
    #[error("invalid snapshot id \"{0}\"")]
    InvalidSnapshotId(String),
    #[error("no snapshots exist for game \"{0}\" — run `gim snap {0}` first")]
    NoSnapshots(String),
    #[error("\"{0}\" is locked by another operation (if this is a mistake, delete the lockfile at {1})")]
    Locked(String, PathBuf),
    #[error("database corruption detected in {0}")]
    DbCorrupt(PathBuf),
    #[error("branch \"{0}\" does not exist for game \"{1}\"")]
    BranchNotFound(String, String),
    #[error("branch \"{0}\" already exists for game \"{1}\"")]
    BranchExists(String, String),
    #[error("cannot delete the current branch \"{0}\" — switch to another branch first")]
    CannotDeleteCurrentBranch(String),
    #[error("cannot delete the protected \"main\" branch")]
    CannotDeleteMainBranch,
    #[error("no current branch set for game \"{0}\"")]
    NoCurrentBranch(String),
    #[error("uncommitted changes detected — run `gim snap` first or use --force")]
    UncommittedChanges,
    #[error("snapshot \"{0}\" is referenced by {1} branch(es): {2}")]
    SnapshotReferencedByBranch(String, usize, String),
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

pub type GResult<T> = Result<T, GError>;

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
        | GError::Config(_)
        | GError::BranchNotFound(_, _)
        | GError::BranchExists(_, _)
        | GError::CannotDeleteCurrentBranch(_)
        | GError::CannotDeleteMainBranch
        | GError::NoCurrentBranch(_)
        | GError::UncommittedChanges
        | GError::SnapshotReferencedByBranch(_, _, _) => 2,
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
