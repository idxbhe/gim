//! Advisory locking.

use crate::error::{GError, GResult};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub struct LockGuard { _file: File, path: PathBuf }

impl LockGuard {
    pub fn try_acquire_exclusive(path: &Path) -> GResult<Option<Self>> {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let file = OpenOptions::new().create(true).read(true).write(true).truncate(false).open(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { _file: file, path: path.to_path_buf() })),
            Err(_) => Ok(None),
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Ok(f) = std::fs::File::open(&self.path) { let _ = fs2::FileExt::unlock(&f); }
    }
}

pub fn acquire_game_lock(alias: &str, snaps_db_path: &Path) -> GResult<LockGuard> {
    let lock_path = snaps_db_path.with_extension("db.lock");
    match LockGuard::try_acquire_exclusive(&lock_path)? {
        Some(g) => Ok(g),
        None => Err(GError::Locked(alias.to_string(), lock_path.clone())),
    }
}
