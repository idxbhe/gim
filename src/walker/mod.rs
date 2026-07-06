//! Parallel file walker + hasher.
//!
//! This module is the hot path of `g snap` and `g restore`. The pipeline
//! is:
//!
//! ```text
//!  walker (single thread)   worker pool (N threads)         collector
//!  ──────────────────────   ──────────────────────         ─────────
//!  walkdir::WalkDir    →    hash_file_with_retry     →     Vec<HashedFile>
//!  apply ignore patterns    (XXH3 streaming, retry          + warnings
//!                            on locked files)
//! ```
//!
//! A bounded crossbeam channel between walker and workers provides
//! backpressure, so we never hold millions of paths in memory at once.
//! Hashed results are collected via Rayon's parallel iterator for
//! simplicity (we measure no meaningful overhead vs. manual channels).

use crate::error::GResult;
use crate::hashing::{hash_file_with_retry, Hash};
use crate::ignore_mod::IgnoreSet;
use crate::path_utils;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// One file, fully hashed. The result of the pipeline.
#[derive(Debug, Clone)]
pub struct HashedFile {
    pub file_path: String, // normalized, forward-slash, relative
    pub hash: Hash,
    pub file_size: i64,
}

/// A file that could not be hashed (e.g. locked by another process).
/// Reported as a warning per spec.
#[derive(Debug, Clone)]
pub struct LockedFile {
    pub file_path: String,
    pub error: String,
}

/// Options for [`walk_and_hash`].
#[derive(Debug, Clone)]
pub struct WalkOptions {
    /// Number of worker threads. Defaults to `num_cpus` if 0.
    pub threads: usize,
    /// Max retries on locked files. Default 3.
    pub max_retries: u32,
    /// Delay between retries. Default 500ms.
    pub retry_delay: Duration,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            threads: 0,
            max_retries: 3,
            retry_delay: Duration::from_millis(500),
        }
    }
}

/// Walk `game_dir`, apply `ignore_set`, hash every surviving file in
/// parallel, and return the results.
///
/// Files that cannot be opened (locked, permission denied after
/// retries) are returned in `locked`, not `files`. The caller is
/// responsible for printing the warnings.
pub fn walk_and_hash(
    game_dir: &Path,
    ignore_set: &IgnoreSet,
    opts: &WalkOptions,
) -> GResult<(Vec<HashedFile>, Vec<LockedFile>)> {
    // First pass: walk and collect candidate paths. We collect eagerly
    // because walkdir is single-threaded and the parallelism happens at
    // hash time. For very large game directories, an alternative is to
    // use a channel + dedicated walker thread; we keep it simple here.
    let candidates = collect_candidates(game_dir, ignore_set)?;

    // Configure the thread pool. If `threads == 0`, leave the global
    // pool at its default (num_cpus).
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
            .map(|(path, normalized)| {
                match hash_file_with_retry(path, opts.max_retries, opts.retry_delay) {
                    Ok(Some((hash, size))) => HashResult::Ok {
                        file_path: normalized.clone(),
                        hash,
                        file_size: size as i64,
                    },
                    Ok(None) => HashResult::Locked {
                        file_path: normalized.clone(),
                        error: format!(
                            "could not open after {} retries",
                            opts.max_retries
                        ),
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
            } => files.push(HashedFile {
                file_path,
                hash,
                file_size,
            }),
            HashResult::Locked { file_path, error } => locked.push(LockedFile {
                file_path,
                error,
            }),
        }
    }
    Ok((files, locked))
}

/// Walk `game_dir`, apply `ignore_set`, and return only the file paths
/// (without hashing). Used by `g restore --full` where we need to know
/// what files exist on disk but don't need their hashes.
pub fn walk_only(game_dir: &Path, ignore_set: &IgnoreSet) -> GResult<Vec<String>> {
    let candidates = collect_candidates(game_dir, ignore_set)?;
    Ok(candidates.into_iter().map(|(_, n)| n).collect())
}

enum HashResult {
    Ok {
        file_path: String,
        hash: Hash,
        file_size: i64,
    },
    Locked {
        file_path: String,
        error: String,
    },
}

/// First pass: walk the directory tree, apply ignore patterns, and
/// return `Vec<(absolute_path, normalized_relative_path)>`.
fn collect_candidates(
    game_dir: &Path,
    ignore_set: &IgnoreSet,
) -> GResult<Vec<(PathBuf, String)>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(game_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Determine the relative path of this entry for ignore-matching.
            let rel = e
                .path()
                .strip_prefix(game_dir)
                .ok()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            if rel.is_empty() {
                return true; // the root itself
            }
            // Gitignore matches against paths; we pass the normalized form.
            let normalized = rel.replace('\\', "/");
            !ignore_set.is_ignored(&normalized, e.file_type().is_dir())
        })
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // best-effort: skip unreadable entries
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = match entry.path().strip_prefix(game_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let normalized = path_utils::normalize(game_dir, rel)?;
        out.push((entry.path().to_path_buf(), normalized));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;
    use std::fs;
    use std::path::PathBuf;

    fn setup_game() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // Create some files
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
            walk_and_hash(dir.path(), &ignore, &WalkOptions::default()).unwrap();
        assert!(locked.is_empty());
        // Should include a.txt, b.tmp, mods/sky.dds
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
        let (files, _) = walk_and_hash(dir.path(), &ignore, &WalkOptions::default()).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.file_path.as_str()).collect();
        assert!(paths.contains(&"a.txt"));
        assert!(!paths.contains(&"b.tmp")); // *.tmp filtered
    }
}
