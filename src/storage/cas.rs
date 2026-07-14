use crate::error::{GError, GResult};
use crate::hashing::Hash;
use crate::path_utils;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct Cas { objects_dir: PathBuf }
impl Cas {
    pub fn new(d: PathBuf) -> Self { Self { objects_dir: d } }

    pub fn ensure(&self) -> GResult<()> {
        fs::create_dir_all(&self.objects_dir)?;
        Ok(())
    }

    /// Remove stale `.tmp.*` files left by interrupted store_from calls.
    /// Called by `gim gc` — not during `ensure()` to avoid startup delay.
    pub fn cleanup_tmp_files(&self) -> GResult<()> {
        if !self.objects_dir.exists() { return Ok(()); }
        for entry in fs::read_dir(&self.objects_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() { continue; }
            for sub in fs::read_dir(entry.path())? {
                let sub = sub?;
                let name = sub.file_name();
                let name = name.to_string_lossy();
                if name.contains(".tmp.") {
                    let _ = fs::remove_file(sub.path());
                }
            }
        }
        Ok(())
    }

    pub fn path_for(&self, h: &Hash) -> PathBuf { path_utils::object_path(&self.objects_dir, h.as_str()) }
    pub fn exists(&self, h: &Hash) -> bool { self.path_for(h).exists() }

    /// Store a file from `src` into the CAS under the given hash.
    ///
    /// Uses `std::fs::copy` which on Linux dispatches to
    /// `copy_file_range` (kernel-space copy, zero user-space buffer),
    /// and on macOS uses `fcopyfile`. This is significantly faster than
    /// manual `io::copy` for large game files.
    ///
    /// The tmp file is protected by a RAII guard (`TmpGuard`) that
    /// automatically deletes it on drop — whether the function returns
    /// Ok, Err, or panics. This is more robust than manual cleanup in
    /// error paths.
    pub fn store_from(&self, src: &Path, hash: &Hash) -> GResult<()> {
        let final_path = self.path_for(hash);
        if final_path.exists() { return Ok(()); }
        if let Some(p) = final_path.parent() { fs::create_dir_all(p)?; }
        let cnt = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_path = final_path.with_extension(format!("tmp.{pid}.{cnt}"));

        // RAII guard: if we exit this function for ANY reason (early
        // return, error, panic) without explicitly calling `keep()`,
        // the tmp file is deleted. This prevents leaks even on panic.
        let mut guard = TmpGuard::new(&tmp_path);

        // Use std::fs::copy for OS-optimized file copy.
        std::fs::copy(src, &tmp_path)?;

        // fsync the tmp file to ensure durability before rename.
        // Open with write access — on Windows, FlushFileBuffers requires
        // write access. Using read-only File::open would silently fail.
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&tmp_path) {
            if let Err(e) = f.sync_all() {
                log::debug!("fsync failed for {:?}: {}", tmp_path, e);
            }
        }

        match fs::rename(&tmp_path, &final_path) {
            Ok(()) => {
                guard.keep();
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && final_path.exists() => {
                // Another process won the race — the final object exists.
                // Our tmp file will be deleted by the guard on drop.
                Ok(())
            }
            Err(e) => Err(GError::Io(e)),
        }
    }

    pub fn open(&self, h: &Hash) -> GResult<File> {
        let p = self.path_for(h);
        File::open(&p).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { GError::Other(format!("object {h} not found")) } else { GError::Io(e) })
    }

    pub fn delete(&self, hash: &str) -> GResult<bool> {
        match fs::remove_file(path_utils::object_path(&self.objects_dir, hash)) {
            Ok(()) => Ok(true), Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false), Err(e) => Err(GError::Io(e)),
        }
    }

    pub fn list_all_hashes(&self) -> GResult<std::collections::HashSet<String>> {
        let mut out = std::collections::HashSet::new();
        if !self.objects_dir.exists() { return Ok(out); }
        for e in fs::read_dir(&self.objects_dir)? {
            let e = e?; if !e.file_type()?.is_dir() { continue; }
            for s in fs::read_dir(e.path())? {
                let s = s?; if !s.file_type()?.is_file() { continue; }
                if let Some(n) = s.file_name().to_str() {
                    let b = n.as_bytes();
                    if b.len() >= 32 && b.iter().all(|c| c.is_ascii_hexdigit()) {
                        out.insert(n.to_string());
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn list_tmp_files(&self) -> GResult<Vec<PathBuf>> {
        let mut out = Vec::new();
        if !self.objects_dir.exists() { return Ok(out); }
        for e in fs::read_dir(&self.objects_dir)? {
            let e = e?; if !e.file_type()?.is_dir() { continue; }
            for s in fs::read_dir(e.path())? {
                let s = s?; if s.file_name().to_string_lossy().contains(".tmp.") { out.push(s.path()); }
            }
        }
        Ok(out)
    }
}

/// RAII guard for a temporary file. Deletes the file on drop unless
/// `keep()` was called. This ensures tmp files are cleaned up even on
/// panic or early return — no manual cleanup needed in error paths.
struct TmpGuard {
    path: PathBuf,
    keep: bool,
}

impl TmpGuard {
    fn new(path: &Path) -> Self {
        Self { path: path.to_path_buf(), keep: false }
    }

    /// Mark the file as successfully handled — don't delete on drop.
    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for TmpGuard {
    fn drop(&mut self) {
        if !self.keep {
            // Best-effort delete — ignore errors (file may already be gone).
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn store_dedupes() {
        let tmp = tempfile::tempdir().unwrap();
        let cas = Cas::new(tmp.path().join("objects"));
        cas.ensure().unwrap();
        let mut src = tempfile::NamedTempFile::new().unwrap();
        writeln!(src, "hello").unwrap();
        src.flush().unwrap();
        let h = Hash("aabbccddeeff00112233445566778899".into());
        cas.store_from(src.path(), &h).unwrap();
        cas.store_from(src.path(), &h).unwrap(); // no-op
        assert!(cas.exists(&h));
    }

    #[test]
    fn tmp_guard_deletes_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_path = tmp.path().join("test.tmp.123");
        std::fs::write(&tmp_path, b"data").unwrap();
        assert!(tmp_path.exists());
        {
            let _guard = TmpGuard::new(&tmp_path);
            // guard drops here, should delete file
        }
        assert!(!tmp_path.exists());
    }

    #[test]
    fn tmp_guard_keep_preserves_file() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_path = tmp.path().join("test.tmp.456");
        std::fs::write(&tmp_path, b"data").unwrap();
        {
            let mut guard = TmpGuard::new(&tmp_path);
            guard.keep();
            // guard drops here, but keep() was called
        }
        assert!(tmp_path.exists());
    }

    #[test]
    fn store_from_cleans_tmp_on_success() {
        // After successful store_from, no .tmp.* files should remain.
        let tmp = tempfile::tempdir().unwrap();
        let cas = Cas::new(tmp.path().join("objects"));
        cas.ensure().unwrap();
        let mut src = tempfile::NamedTempFile::new().unwrap();
        writeln!(src, "hello").unwrap();
        src.flush().unwrap();
        let h = Hash("aabbccddeeff00112233445566778899".into());
        cas.store_from(src.path(), &h).unwrap();
        let tmps = cas.list_tmp_files().unwrap();
        assert!(tmps.is_empty(), "tmp files leaked: {:?}", tmps);
    }
}
