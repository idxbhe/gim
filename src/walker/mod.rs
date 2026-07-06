//! Parallel file walker + hasher with mtime+size fast pre-filter.
//!
//! ## Two-pass pipeline
//!
//! 1. **Walk pass** (single-threaded): walkdir traverses the game
//!    directory, applies ignore patterns, and collects `(absolute_path,
//!    normalized_path, file_size, file_mtime)` for each surviving file.
//!    Metadata is fetched via `symlink_metadata()` — a single stat per
//!    file, no I/O for file content.
//!
//! 2. **Hash pass** (parallel via Rayon): for each file, decide whether
//!    we actually need to hash it:
//!    - File is NOT in the reference map → must hash (it's new or
//!      changed-in-some-way we can't determine cheaply).
//!    - File IS in reference, but size OR mtime differs → must hash to
//!      verify whether content actually changed.
//!    - File IS in reference AND size AND mtime match → SKIP hashing,
//!      reuse the reference hash.
//!
//! This means: for an idle game directory with no changes since the last
//! snapshot, the hash pass does **zero** file reads. Only metadata
//! (stat) was read in the walk pass, which takes milliseconds even for
//! 10,000+ files. The first run after a `gim restore` will also be
//! fast because `restore` sets the mtime of restored files to match
//! the snapshot's recorded mtime.
//!
//! ## Threat model for the pre-filter
//!
//! The pre-filter is a **heuristic**, not ground truth. Its correctness
//! rests on: "if size and mtime are unchanged, the file content is
//! unchanged." This holds for the overwhelming majority of game files
//! because game engines always rewrite save/config files wholesale,
//! updating mtime. The rare edge case (content changed but mtime
//! preserved by `touch -d` or by an editor that preserves mtime) is
//! documented; users can defeat it with `--full-hash` on any command
//! that uses the pre-filter.

use crate::db::FileMeta;
use crate::error::GResult;
use crate::hashing::{hash_file_with_retry, Hash};
use crate::ignore_mod::IgnoreSet;
use crate::path_utils;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// One file, fully hashed. The result of the pipeline.
#[derive(Debug, Clone)]
pub struct HashedFile {
    pub file_path: String, // normalized, forward-slash, relative
    pub hash: Hash,
    pub file_size: i64,
    pub modified_time: i64, // Unix seconds
}

/// A file that could not be hashed (e.g. locked by another process).
#[derive(Debug, Clone)]
pub struct LockedFile {
    pub file_path: String,
    pub error: String,
}

/// Options for the walker.
#[derive(Debug, Clone)]
pub struct WalkOptions {
    /// Number of worker threads. Defaults to `num_cpus` if 0.
    pub threads: usize,
    /// Max retries on locked files. Default 3.
    pub max_retries: u32,
    /// Delay between retries. Default 500ms.
    pub retry_delay: Duration,
    /// If `true`, ignore the reference map and hash every file. Used by
    /// `--full-hash` on `gim snap` / `gim status`. Default `false`.
    pub full_hash: bool,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            threads: 0,
            max_retries: 3,
            retry_delay: Duration::from_millis(500),
            full_hash: false,
        }
    }
}

/// Walk `game_dir` and hash every surviving file in parallel.
///
/// If `reference` is `Some` and `opts.full_hash` is `false`, files whose
/// `(size, mtime)` match the reference are not re-hashed — their hash is
/// inherited from the reference map. This is the mtime+size fast
/// pre-filter.
///
/// Files that cannot be opened (locked, permission denied after retries)
/// are returned in `locked`, not `files`.
pub fn walk_and_hash(
    game_dir: &Path,
    ignore_set: &IgnoreSet,
    reference: Option<&HashMap<String, FileMeta>>,
    opts: &WalkOptions,
) -> GResult<(Vec<HashedFile>, Vec<LockedFile>)> {
    let candidates = collect_candidates_with_meta(game_dir, ignore_set)?;

    let use_smart = !opts.full_hash && reference.is_some();
    let reference = reference.cloned().unwrap_or_default();

    let pool = if opts.threads > 0 {
        Some(
            rayon::ThreadPoolBuilder::new()
                .num_threads(opts.threads)
                .build()
                .map_err(|e| {
                    crate::error::GError::Other(format!("cannot build thread pool: {e}"))
                })?,
        )
    } else {
        None
    };

    let do_hash = || {
        candidates
            .par_iter()
            .map(|(path, normalized, size, mtime)| {
                // Smart pre-filter: skip hashing if size+mtime match reference.
                let need_hash = if use_smart {
                    match reference.get(normalized) {
                        Some(meta) => {
                            meta.file_size != *size || meta.modified_time != *mtime
                        }
                        None => true, // new file
                    }
                } else {
                    true // full-hash mode
                };

                if !need_hash {
                    let meta = reference.get(normalized).expect("checked above");
                    return HashResult::Ok {
                        file_path: normalized.clone(),
                        hash: meta.hash.clone(),
                        file_size: *size,
                        modified_time: *mtime,
                    };
                }

                match hash_file_with_retry(path, opts.max_retries, opts.retry_delay) {
                    Ok(Some((hash, _file_size_from_hash))) => HashResult::Ok {
                        file_path: normalized.clone(),
                        hash,
                        file_size: *size,
                        modified_time: *mtime,
                    },
                    Ok(None) => HashResult::Locked {
                        file_path: normalized.clone(),
                        error: format!("could not open after {} retries", opts.max_retries),
                    },
                    Err(e) => HashResult::Locked {
                        file_path: normalized.clone(),
                        error: format!("{e}"),
                    },
                }
            })
            .collect::<Vec<_>>()
    };

    let results = match &pool {
        Some(p) => p.install(do_hash),
        None => do_hash(),
    };

    let mut files = Vec::with_capacity(results.len());
    let mut locked = Vec::new();
    for r in results {
        match r {
            HashResult::Ok {
                file_path,
                hash,
                file_size,
                modified_time,
            } => files.push(HashedFile {
                file_path,
                hash,
                file_size,
                modified_time,
            }),
            HashResult::Locked { file_path, error } => locked.push(LockedFile {
                file_path,
                error,
            }),
        }
    }
    Ok((files, locked))
}

enum HashResult {
    Ok {
        file_path: String,
        hash: Hash,
        file_size: i64,
        modified_time: i64,
    },
    Locked {
        file_path: String,
        error: String,
    },
}

/// Walk `game_dir` and return only the file paths (no metadata, no
/// hashing). Used by `gim restore --full` where we need to know what
/// files exist on disk but don't need their hashes (we'll overwrite
/// everything anyway).
pub fn walk_only(game_dir: &Path, ignore_set: &IgnoreSet) -> GResult<Vec<String>> {
    let candidates = collect_candidates_with_meta(game_dir, ignore_set)?;
    Ok(candidates.into_iter().map(|(_, n, _, _)| n).collect())
}

/// First pass: walk the directory tree, apply ignore patterns, fetch
/// metadata (size + mtime) for each surviving file.
///
/// Returns `Vec<(absolute_path, normalized_relative_path, size_bytes,
/// mtime_unix_seconds)>`.
fn collect_candidates_with_meta(
    game_dir: &Path,
    ignore_set: &IgnoreSet,
) -> GResult<Vec<(PathBuf, String, i64, i64)>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(game_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let rel = e
                .path()
                .strip_prefix(game_dir)
                .ok()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            if rel.is_empty() {
                return true;
            }
            let normalized = rel.replace('\\', "/");
            !ignore_set.is_ignored(&normalized, e.file_type().is_dir())
        })
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = match entry.path().strip_prefix(game_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let normalized = path_utils::normalize(game_dir, rel)?;
        // Stat the file (follows symlink_metadata so we don't follow
        // symlinks — we want to record the file itself).
        let meta = match std::fs::symlink_metadata(entry.path()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let size = meta.len() as i64;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push((entry.path().to_path_buf(), normalized, size, mtime));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;
    use crate::hashing::hash_bytes;
    use std::fs;
    use std::path::PathBuf;

    fn setup_game() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        fs::write(dir.path().join("b.tmp"), b"ignored").unwrap();
        fs::create_dir_all(dir.path().join("mods")).unwrap();
        fs::write(dir.path().join("mods/sky.dds"), b"texture").unwrap();
        dir
    }

    #[test]
    fn walks_and_hashes_files() {
        let dir = setup_game();
        let ignore = IgnoreSet::empty().unwrap();
        let (files, locked) =
            walk_and_hash(dir.path(), &ignore, None, &WalkOptions::default()).unwrap();
        assert!(locked.is_empty());
        let paths: Vec<&str> = files.iter().map(|f| f.file_path.as_str()).collect();
        assert!(paths.contains(&"a.txt"));
        assert!(paths.contains(&"b.tmp"));
        assert!(paths.contains(&"mods/sky.dds"));
    }

    #[test]
    fn respects_default_ignores() {
        let dir = setup_game();
        let binary_dir = PathBuf::from("/tmp/nonexistent_binary");
        let paths = Paths::from_binary_dir(binary_dir).unwrap();
        let ignore = crate::ignore_mod::build_for_game(&paths, "test", dir.path()).unwrap();
        let (files, _) = walk_and_hash(dir.path(), &ignore, None, &WalkOptions::default()).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.file_path.as_str()).collect();
        assert!(paths.contains(&"a.txt"));
        assert!(!paths.contains(&"b.tmp"));
    }

    #[test]
    fn smart_skip_unchanged_files() {
        let dir = setup_game();
        let abs = dir.path().to_path_buf();
        let ignore = IgnoreSet::empty().unwrap();

        // First pass: full hash, build reference map.
        let (files, _) = walk_and_hash(&abs, &ignore, None, &WalkOptions::default()).unwrap();
        let mut reference: HashMap<String, FileMeta> = HashMap::new();
        for f in &files {
            reference.insert(
                f.file_path.clone(),
                FileMeta {
                    hash: f.hash.clone(),
                    file_size: f.file_size,
                    modified_time: f.modified_time,
                },
            );
        }

        // Second pass: smart walk with the reference. Since nothing
        // changed, no file should need re-hashing. We can verify this
        // by checking that the hashes still match (which they will
        // because we reuse them from reference).
        let (files2, _) = walk_and_hash(&abs, &ignore, Some(&reference), &WalkOptions::default())
            .unwrap();
        assert_eq!(files.len(), files2.len());
        for f in &files2 {
            let r = reference.get(&f.file_path).unwrap();
            assert_eq!(f.hash, r.hash, "hash mismatch for {}", f.file_path);
        }
    }

    #[test]
    fn smart_re_hashes_changed_files() {
        let dir = setup_game();
        let abs = dir.path().to_path_buf();
        let ignore = IgnoreSet::empty().unwrap();

        let (files, _) = walk_and_hash(&abs, &ignore, None, &WalkOptions::default()).unwrap();
        let mut reference: HashMap<String, FileMeta> = HashMap::new();
        for f in &files {
            reference.insert(
                f.file_path.clone(),
                FileMeta {
                    hash: f.hash.clone(),
                    file_size: f.file_size,
                    modified_time: f.modified_time,
                },
            );
        }

        // Modify a.txt — content changes, mtime changes too.
        std::thread::sleep(std::time::Duration::from_secs(2));
        fs::write(dir.path().join("a.txt"), b"hello world").unwrap();

        let (files2, _) = walk_and_hash(&abs, &ignore, Some(&reference), &WalkOptions::default())
            .unwrap();

        // The new hash of a.txt should differ from the reference.
        let a2 = files2.iter().find(|f| f.file_path == "a.txt").unwrap();
        let a_ref = reference.get("a.txt").unwrap();
        assert_ne!(a2.hash, a_ref.hash);
        assert_eq!(a2.hash, hash_bytes(b"hello world"));
    }
}
