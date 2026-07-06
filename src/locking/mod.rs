//! Advisory locking — prevents concurrent `snap` / `restore` on the
//! same game.
//!
//! Per spec, only one snap or restore may run per game at a time. We
//! implement this with an exclusive lock on a sentinel file inside the
//! game's data directory (`data/[alias]/snaps.db.lock`). The lock is
//! automatically released when the process exits or the guard is dropped.

use crate::error::{GError, GResult};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// RAII guard for an advisory lock. Dropping this releases the lock.
pub struct LockGuard {
    _file: File,
    path: PathBuf,
}

impl LockGuard {
    /// Acquire an exclusive lock on `path`. Blocks until the lock is
    /// available. The lock file is created if it does not exist.
    pub fn acquire_exclusive(path: &Path) -> GResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        file.lock_exclusive().map_err(|e| {
            GError::Other(format!(
                "cannot acquire exclusive lock on {}: {e}",
                path.display()
            ))
        })?;
        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }

    /// Try to acquire an exclusive lock without blocking. Returns
    /// `Ok(Some(guard))` on success, `Ok(None)` if the lock is held by
    /// another process.
    pub fn try_acquire_exclusive(path: &Path) -> GResult<Option<Self>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self {
                _file: file,
                path: path.to_path_buf(),
            })),
            Err(_) => Ok(None),
        }
    }

    /// Path to the lock file (for error reporting).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Best-effort unlock; the OS releases the lock when the file
        // descriptor closes anyway.
        if let Ok(f) = std::fs::File::open(&self.path) {
            let _ = fs2::FileExt::unlock(&f);
        }
    }
}

/// Acquire a non-blocking lock for a game's `snaps.db`. If the lock is
/// already held, returns [`GError::Locked`] with a user-facing message
/// matching the spec format.
pub fn acquire_game_lock(
    alias: &str,
    snaps_db_path: &Path,
) -> GResult<LockGuard> {
    let lock_path = snaps_db_path.with_extension("db.lock");
    match LockGuard::try_acquire_exclusive(&lock_path)? {
        Some(g) => Ok(g),
        None => Err(GError::Locked(alias.to_string(), lock_path.clone())),
    }
}
