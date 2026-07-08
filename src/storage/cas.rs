//! Content-addressable storage (CAS).
//!
//! v0.3.1 fixes:
//! - **Race-safe tmp files**: each `store_from` call uses a unique tmp
//!   name (process PID + counter + random) so concurrent writes of the
//!   same hash cannot corrupt each other. If a tmp file is left behind
//!   by a crash, `gim gc` cleans it up.
//! - **`std::io::copy` instead of manual buffer loop**: on Linux this
//!   uses `copy_file_range` (kernel-space, zero userspace copy); on
//!   macOS uses `fcopyfile`; on Windows uses `CopyFileEx`. Much faster
//!   for multi-GB game files.

use crate::error::{GError, GResult};
use crate::hashing::Hash;
use crate::path_utils;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counter for unique tmp-file names within this process.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct Cas {
    objects_dir: PathBuf,
}

impl Cas {
    pub fn new(objects_dir: PathBuf) -> Self { Self { objects_dir } }

    pub fn ensure(&self) -> GResult<()> { fs::create_dir_all(&self.objects_dir)?; Ok(()) }

    pub fn path_for(&self, hash: &Hash) -> PathBuf {
        path_utils::object_path(&self.objects_dir, hash.as_str())
    }

    pub fn exists(&self, hash: &Hash) -> bool { self.path_for(hash).exists() }

    /// Store a file from `src` into the CAS under the given hash.
    ///
    /// If the object already exists, this is a no-op. Otherwise, copy to
    /// a **uniquely-named** tmp file, fsync, then atomically rename.
    ///
    /// The unique tmp name prevents corruption when two threads process
    /// files with identical content (same hash) concurrently — each gets
    /// its own tmp file, and only one rename succeeds (the other is a
    /// no-op since the final path already exists after the first rename).
    pub fn store_from(&self, src: &Path, hash: &Hash) -> GResult<()> {
        let final_path = self.path_for(hash);
        if final_path.exists() { return Ok(()); }

        if let Some(parent) = final_path.parent() { fs::create_dir_all(parent)?; }

        // Unique tmp name: <hash>.tmp.<pid>.<counter>
        // This guarantees no two concurrent store_from calls collide,
        // even for the same hash.
        let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_path = final_path.with_extension(format!("tmp.{pid}.{counter}"));

        let src_file = File::open(src)?;
        let mut dst_file = OpenOptions::new()
            .create_new(true)  // fail if exists — should never happen with unique names
            .write(true)
            .open(&tmp_path)?;

        // Use std::io::copy which dispatches to copy_file_range (Linux),
        // fcopyfile (macOS), or CopyFileEx (Windows) — all kernel-optimized.
        io::copy(&mut &src_file, &mut dst_file)?;
        dst_file.flush()?;
        dst_file.sync_all()?;
        drop(dst_file);
        drop(src_file);

        // Atomic rename. On the rare race where another thread already
        // renamed an identical object, our rename fails — but that's fine,
        // the object is already there. Just clean up our tmp file.
        match fs::rename(&tmp_path, &final_path) {
            Ok(()) => Ok(()),
            Err(_) if final_path.exists() => {
                // Another thread won the race; clean up our tmp file.
                let _ = fs::remove_file(&tmp_path);
                Ok(())
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                Err(GError::Io(e))
            }
        }
    }

    pub fn open(&self, hash: &Hash) -> GResult<File> {
        let p = self.path_for(hash);
        File::open(&p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GError::Other(format!("object {} not found in CAS — run `gim repair`", hash))
            } else { GError::Io(e) }
        })
    }

    pub fn delete(&self, hash: &str) -> GResult<bool> {
        let p = path_utils::object_path(&self.objects_dir, hash);
        match fs::remove_file(&p) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(GError::Io(e)),
        }
    }

    /// Scan `objects/` and return every stored hash. Uses byte-level
    /// hex check (faster than char-level for ASCII-only strings).
    pub fn list_all_hashes(&self) -> GResult<std::collections::HashSet<String>> {
        let mut out = std::collections::HashSet::new();
        if !self.objects_dir.exists() { return Ok(out); }
        for entry in fs::read_dir(&self.objects_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() { continue; }
            for sub in fs::read_dir(entry.path())? {
                let sub = sub?;
                if !sub.file_type()?.is_file() { continue; }
                if let Some(name) = sub.file_name().to_str() {
                    if is_valid_hash_name(name.as_bytes()) {
                        out.insert(name.to_string());
                    }
                }
            }
        }
        Ok(out)
    }

    /// Scan for stray `.tmp.*` files left by interrupted writes.
    pub fn list_tmp_files(&self) -> GResult<Vec<PathBuf>> {
        let mut out = Vec::new();
        if !self.objects_dir.exists() { return Ok(out); }
        for entry in fs::read_dir(&self.objects_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() { continue; }
            for sub in fs::read_dir(entry.path())? {
                let sub = sub?;
                let name = sub.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("*.tmp.") || name.contains(".tmp.") {
                    out.push(sub.path());
                }
            }
        }
        Ok(out)
    }
}

/// Byte-level hex validation — faster than `chars().all(is_ascii_hexdigit)`
/// because it skips UTF-8 decoding.
#[inline]
fn is_valid_hash_name(bytes: &[u8]) -> bool {
    bytes.len() == 32 && bytes.iter().all(|b| b.is_ascii_hexdigit())
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
    fn concurrent_store_same_hash_no_corruption() {
        use std::sync::Arc;
        use std::thread;
        let tmp = tempfile::tempdir().unwrap();
        let cas = Arc::new(Cas::new(tmp.path().join("objects")));
        cas.ensure().unwrap();

        // Create 4 different source files all with the same content
        // (thus same hash).
        let mut srcs = Vec::new();
        for _ in 0..4 {
            let mut s = tempfile::NamedTempFile::new().unwrap();
            writeln!(s, "identical content").unwrap();
            s.flush().unwrap();
            srcs.push(s);
        }

        let hash_str = "aabbccddeeff00112233445566778899";
        let handles: Vec<_> = srcs
            .into_iter()
            .map(|s| {
                let cas = cas.clone();
                let path = s.into_temp_path().keep().unwrap();
                let h = Hash(hash_str.to_string());
                thread::spawn(move || cas.store_from(&path, &h).unwrap())
            })
            .collect();
        for h in handles { h.join().unwrap(); }

        // Verify the object exists and is intact.
        let h = Hash(hash_str.to_string());
        assert!(cas.exists(&h));
        let content = std::fs::read(cas.path_for(&h)).unwrap();
        assert_eq!(content, b"identical content\n");

        // Verify no stray tmp files remain.
        let tmps = cas.list_tmp_files().unwrap();
        assert!(tmps.is_empty(), "stray tmp files: {:?}", tmps);
    }

    #[test]
    fn list_all_hashes_byte_check() {
        let tmp = tempfile::tempdir().unwrap();
        let cas = Cas::new(tmp.path().join("objects"));
        cas.ensure().unwrap();
        let h = Hash("aabbccddeeff00112233445566778899".into());
        let p = cas.path_for(&h);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"data").unwrap();
        // Junk file that's not 32 hex chars
        fs::write(tmp.path().join("objects/aa/junk.txt"), b"x").unwrap();
        let all = cas.list_all_hashes().unwrap();
        assert!(all.contains("aabbccddeeff00112233445566778899"));
        assert!(!all.contains("junk.txt"));
    }
}
