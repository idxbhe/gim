//! Content-addressable storage (CAS).
//!
//! File blobs are stored under `objects/[hash_prefix]/[hash]`, where
//! `hash_prefix` is the first 2 characters of the XXH3-128 hex digest.
//! This 2-level sharding prevents any single directory from growing
//! too large (which degrades filesystem performance on most platforms).
//!
//! Identical files across snapshots share the same object — automatic
//! deduplication. Writes are atomic: we copy to a `.tmp` file and rename
//! to the final name only after a successful copy+fsync, so a crash
//! never leaves a partially-written object visible.

use crate::error::{GError, GResult};
use crate::hashing::Hash;
use crate::path_utils;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Handle to a game's `objects/` directory.
pub struct Cas {
    objects_dir: PathBuf,
}

impl Cas {
    pub fn new(objects_dir: PathBuf) -> Self {
        Self { objects_dir }
    }

    /// Ensure the objects directory exists. Call this once before any
    /// read/write operation.
    pub fn ensure(&self) -> GResult<()> {
        fs::create_dir_all(&self.objects_dir)?;
        Ok(())
    }

    /// Path to the object file for a given hash.
    pub fn path_for(&self, hash: &Hash) -> PathBuf {
        path_utils::object_path(&self.objects_dir, hash.as_str())
    }

    /// Check whether an object already exists in the store. Used to
    /// skip redundant copies during `snap`.
    pub fn exists(&self, hash: &Hash) -> bool {
        self.path_for(hash).exists()
    }

    /// Store a file from `src` into the CAS under the given hash.
    ///
    /// If the object already exists, this is a no-op (deduplication).
    /// Otherwise, we copy to a sibling `.tmp` file, fsync, then rename
    /// atomically. On the rare race where two processes try to write
    /// the same object simultaneously, the rename is atomic and the
    /// loser's tmp file is left behind (cleaned up by `g gc`).
    pub fn store_from(&self, src: &Path, hash: &Hash) -> GResult<()> {
        let final_path = self.path_for(hash);
        if final_path.exists() {
            return Ok(());
        }
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_path = final_path.with_extension("tmp");

        // Atomic copy: read source in chunks, write to .tmp, fsync, rename.
        let mut src_file = File::open(src)?;
        let mut dst_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)?;
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = src_file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            dst_file.write_all(&buf[..n])?;
        }
        dst_file.flush()?;
        dst_file.sync_all()?;

        // Atomic rename (on same filesystem, which it is — both inside objects/).
        fs::rename(&tmp_path, &final_path)?;
        Ok(())
    }

    /// Open an object for reading. Used by `restore` to copy files
    /// back into the game directory.
    pub fn open(&self, hash: &Hash) -> GResult<File> {
        let p = self.path_for(hash);
        File::open(&p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GError::Other(format!(
                    "object {} not found in CAS — database may be inconsistent; run `g repair`",
                    hash
                ))
            } else {
                GError::Io(e)
            }
        })
    }

    /// Delete an object. Used by `g gc` to remove orphaned objects.
    /// Returns `Ok(true)` if a file was deleted, `Ok(false)` if it
    /// did not exist.
    pub fn delete(&self, hash: &str) -> GResult<bool> {
        let p = path_utils::object_path(&self.objects_dir, hash);
        match fs::remove_file(&p) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(GError::Io(e)),
        }
    }

    /// Scan the entire `objects/` directory and yield every stored hash
    /// (the filename, which is the hash). Used by `g gc`.
    pub fn list_all_hashes(&self) -> GResult<std::collections::HashSet<String>> {
        let mut out = std::collections::HashSet::new();
        if !self.objects_dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&self.objects_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            for sub in fs::read_dir(entry.path())? {
                let sub = sub?;
                if !sub.file_type()?.is_file() {
                    continue;
                }
                if let Some(name) = sub.file_name().to_str() {
                    // Only include valid-looking 32-char hex names.
                    if name.len() == 32 && name.chars().all(|c| c.is_ascii_hexdigit()) {
                        out.insert(name.to_string());
                    }
                }
            }
        }
        Ok(out)
    }

    /// Scan `objects/` and return all loose `.tmp` files (left over
    /// from interrupted writes). Used by `g gc` for cleanup.
    pub fn list_tmp_files(&self) -> GResult<Vec<PathBuf>> {
        let mut out = Vec::new();
        if !self.objects_dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&self.objects_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            for sub in fs::read_dir(entry.path())? {
                let sub = sub?;
                if sub.file_name().to_string_lossy().ends_with(".tmp") {
                    out.push(sub.path());
                }
            }
        }
        Ok(out)
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
        cas.store_from(src.path(), &h).unwrap(); // should be a no-op
        assert!(cas.exists(&h));
        assert!(cas.open(&h).is_ok());
    }

    #[test]
    fn list_all_hashes_returns_only_32hex_names() {
        let tmp = tempfile::tempdir().unwrap();
        let cas = Cas::new(tmp.path().join("objects"));
        cas.ensure().unwrap();
        // Create a valid object.
        let h = Hash("aabbccddeeff00112233445566778899".into());
        let p = cas.path_for(&h);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"data").unwrap();
        // Create a junk file.
        let junk = tmp.path().join("objects/aa/junk.txt");
        std::fs::write(&junk, b"x").unwrap();
        let all = cas.list_all_hashes().unwrap();
        assert!(all.contains("aabbccddeeff00112233445566778899"));
        assert!(!all.contains("junk.txt"));
    }
}
