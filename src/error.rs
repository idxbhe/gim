use std::io;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GError {
    #[error("game with alias \"{0}\" already exists")] AliasExists(String),
    #[error("game with alias \"{0}\" does not exist")] AliasNotFound(String),
    #[error("game directory \"{0}\" does not exist")] GameDirMissing(PathBuf),
    #[error("game directory \"{0}\" is not a directory")] GameDirNotDir(PathBuf),
    #[error("snapshot \"{0}\" does not exist for game \"{1}\"")] SnapshotNotFound(String, String),
    #[error("snapshot id \"{0}\" already exists for game \"{1}\"")] SnapshotIdExists(String, String),
    #[error("invalid snapshot id \"{0}\"")] InvalidSnapshotId(String),
    #[error("no snapshots exist for game \"{0}\"")] NoSnapshots(String),
    #[error("\"{0}\" is locked by another operation (lockfile: {1})")] Locked(String, PathBuf),
    #[error("database corruption detected in {0}")] DbCorrupt(PathBuf),
    #[error("branch \"{0}\" does not exist for game \"{1}\"")] BranchNotFound(String, String),
    #[error("branch \"{0}\" already exists for game \"{1}\"")] BranchExists(String, String),
    #[error("cannot delete the current branch \"{0}\"")] CannotDeleteCurrentBranch(String),
    #[error("cannot delete the protected \"main\" branch")] CannotDeleteMainBranch,
    #[error("uncommitted changes detected — use --force to discard")] UncommittedChanges,
    #[error("snapshot \"{0}\" is referenced by {1} branch(es): {2}")] SnapshotReferencedByBranch(String, usize, String),
    #[error("rehash cancelled by user")] RehashCancelled,
    #[error("hash algorithm mismatch: config says \"{0}\" but snapshot data uses \"{1}\"")] HashAlgorithmMismatch(String, String),
    #[error("repack error: {0}")] Repack(String),
    #[error("unpack error: {0}")] Unpack(String),
    #[error("xtool error: {0}")] Xtool(String),
    #[error("invalid manifest: {0}")] InvalidManifest(String),
    #[error("compaction cancelled by user")] CompactCancelled,
    #[error("compaction error: {0}")] Compact(String),
    #[error("a compaction is already running for game \"{0}\" (lockfile: {1})")] CompactRunning(String, PathBuf),
    #[error("operation requires Windows (only supported on Windows)")] NotSupportedPlatform,
    #[error("sqlite error: {0}")] Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")] Io(#[from] io::Error),
    #[error("path error: {0}")] Path(String),
    #[error("config error: {0}")] Config(String),
    #[error("ignore pattern error in {file}: {message}")] IgnorePattern { file: PathBuf, message: String },
    #[error("json error: {0}")] Json(#[from] serde_json::Error),
    #[error("ignore-pattern error: {0}")] Ignore(#[from] ignore::Error),
    #[error("{0}")] Other(String),
}

pub type GResult<T> = Result<T, GError>;

pub fn exit_code(err: &GError) -> i32 {
    match err {
        GError::AliasExists(_) | GError::AliasNotFound(_) | GError::GameDirMissing(_)
        | GError::GameDirNotDir(_) | GError::SnapshotNotFound(_, _) | GError::SnapshotIdExists(_, _)
        | GError::InvalidSnapshotId(_) | GError::NoSnapshots(_) | GError::Locked(_, _)
        | GError::DbCorrupt(_) | GError::IgnorePattern { .. } | GError::Path(_)
        | GError::Config(_) | GError::BranchNotFound(_, _) | GError::BranchExists(_, _)
        | GError::CannotDeleteCurrentBranch(_) | GError::CannotDeleteMainBranch
        | GError::UncommittedChanges | GError::SnapshotReferencedByBranch(_, _, _)
        | GError::RehashCancelled | GError::HashAlgorithmMismatch(_, _)
        | GError::InvalidManifest(_) | GError::CompactCancelled
        | GError::CompactRunning(_, _) | GError::NotSupportedPlatform => 2,
        _ => 1,
    }
}
