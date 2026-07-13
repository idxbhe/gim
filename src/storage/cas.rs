use crate::error::{GError, GResult};
use crate::hashing::Hash;
use crate::path_utils;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct Cas { objects_dir: PathBuf }
impl Cas {
    pub fn new(d: PathBuf) -> Self { Self { objects_dir: d } }

    pub fn ensure(&self) -> GResult<()> {
        fs::create_dir_all(&self.objects_dir)?;
        // Cleanup any stale .tmp.* files from previous crashes.
        // This is cheap (one readdir per prefix subdir) and prevents
        // permanent tmp file leaks.
        self.cleanup_tmp_files()?;
        Ok(())
    }

    /// Remove stale `.tmp.*` files left by interrupted store_from calls.
    fn cleanup_tmp_files(&self) -> GResult<()> {
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
    pub fn store_from(&self, src: &Path, hash: &Hash) -> GResult<()> {
        let final_path = self.path_for(hash);
        if final_path.exists() { return Ok(()); }
        if let Some(p) = final_path.parent() { fs::create_dir_all(p)?; }
        let cnt = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_path = final_path.with_extension(format!("tmp.{pid}.{cnt}"));

        // Use std::fs::copy for OS-optimized file copy.
        // On Linux: copy_file_range (kernel-space, zero user copy).
        // On macOS: fcopyfile. On Windows: CopyFileEx.
        std::fs::copy(src, &tmp_path)?;

        // fsync the tmp file to ensure durability before rename.
        if let Ok(f) = File::open(&tmp_path) {
            let _ = f.sync_all();
        }

        match fs::rename(&tmp_path, &final_path) {
            Ok(()) => Ok(()),
            Err(_) if final_path.exists() => { let _ = fs::remove_file(&tmp_path); Ok(()) }
            Err(e) => { let _ = fs::remove_file(&tmp_path); Err(GError::Io(e)) }
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
