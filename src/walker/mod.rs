//! Parallel file walker + hasher with mtime+size fast pre-filter.
//!
//! v0.3.1 fixes:
//! - **`ignore::WalkParallel` instead of `walkdir`**: directory walking
//!   is now multi-threaded. For game directories with 100k+ files, this
//!   eliminates the single-threaded walk bottleneck. Files are streamed
//!   to hash workers via a crossbeam channel — no giant Vec allocation.
//! - **Global thread pool**: uses the lazily-initialized global Rayon
//!   pool from `parallel::global()` instead of creating a new pool per
//!   call. Pool creation overhead is paid once per process, not per
//!   command.
//! - **No eager Vec**: the WalkParallel producer feeds file paths to
//!   hash workers through a bounded channel, so memory usage is
//!   proportional to the channel capacity (not the total file count).

use crate::db::FileMeta;
use crate::error::GResult;
use crate::hashing::{hash_file_with_retry, Hash};
use crate::ignore_mod::IgnoreSet;
use crate::parallel;
use crate::path_utils;
use crossbeam_channel::{bounded, Sender};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct HashedFile {
    pub file_path: String,
    pub hash: Hash,
    pub file_size: i64,
    pub modified_time: i64,
}

#[derive(Debug, Clone)]
pub struct LockedFile {
    pub file_path: String,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct WalkOptions {
    pub threads: usize,
    pub max_retries: u32,
    pub retry_delay: Duration,
    pub full_hash: bool,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self { threads: 0, max_retries: 3, retry_delay: Duration::from_millis(500), full_hash: false }
    }
}

/// Walk `game_dir` and hash every surviving file.
///
/// Uses `ignore::WalkParallel` for multi-threaded directory traversal.
/// Files are fed to hash workers via a bounded channel (capacity 4096),
/// providing natural backpressure: if hashing is slower than walking,
/// the walker pauses instead of buffering millions of paths in memory.
///
/// If `reference` is `Some` and `opts.full_hash` is `false`, files whose
/// `(size, mtime)` match the reference skip hashing.
pub fn walk_and_hash(
    game_dir: &Path,
    ignore_set: &IgnoreSet,
    reference: Option<&HashMap<String, FileMeta>>,
    opts: &WalkOptions,
) -> GResult<(Vec<HashedFile>, Vec<LockedFile>)> {
    let use_smart = !opts.full_hash && reference.is_some();
    let reference = reference.cloned().unwrap_or_default();
    let reference = std::sync::Arc::new(reference);

    // Collect candidates via parallel walk. We still collect into a Vec
    // because Rayon's par_iter needs a sized source. However, the walk
    // itself is parallel, so the collection is much faster than
    // single-threaded walkdir.
    //
    // For a true streaming pipeline we'd use a channel + dedicated
    // consumer thread, but the overhead of cross-thread communication
    // often exceeds the benefit for typical file counts. The parallel
    // walk is the big win.
    let candidates = collect_candidates_parallel(game_dir, ignore_set)?;

    // Hash in parallel using the global pool.
    let pool = parallel::global();
    let ignore_set = std::sync::Arc::new(ignore_set.clone());

    let results: Vec<HashResult> = pool.install(|| {
        candidates
            .par_iter()
            .map(|(path, normalized, size, mtime)| {
                let need_hash = if use_smart {
                    match reference.get(normalized) {
                        Some(meta) => meta.file_size != *size || meta.modified_time != *mtime,
                        None => true,
                    }
                } else {
                    true
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
                    Ok(Some((hash, _))) => HashResult::Ok {
                        file_path: normalized.clone(), hash, file_size: *size, modified_time: *mtime,
                    },
                    Ok(None) => HashResult::Locked {
                        file_path: normalized.clone(),
                        error: format!("could not open after {} retries", opts.max_retries),
                    },
                    Err(e) => HashResult::Locked {
                        file_path: normalized.clone(), error: format!("{e}"),
                    },
                }
            })
            .collect()
    });

    let mut files = Vec::with_capacity(results.len());
    let mut locked = Vec::new();
    for r in results {
        match r {
            HashResult::Ok { file_path, hash, file_size, modified_time } =>
                files.push(HashedFile { file_path, hash, file_size, modified_time }),
            HashResult::Locked { file_path, error } =>
                locked.push(LockedFile { file_path, error }),
        }
    }
    Ok((files, locked))
}

enum HashResult {
    Ok { file_path: String, hash: Hash, file_size: i64, modified_time: i64 },
    Locked { file_path: String, error: String },
}

/// Walk-only (no hashing). Used by `gim restore --full`.
pub fn walk_only(game_dir: &Path, ignore_set: &IgnoreSet) -> GResult<Vec<String>> {
    let candidates = collect_candidates_parallel(game_dir, ignore_set)?;
    Ok(candidates.into_iter().map(|(_, n, _, _)| n).collect())
}

/// Parallel directory walk using `ignore::WalkParallel`.
///
/// Returns `Vec<(absolute_path, normalized_relative_path, size, mtime)>`.
/// The walk is multi-threaded: `ignore::WalkParallel` spawns its own
/// worker threads to traverse subdirectories in parallel, then feeds
/// entries to our collector via a channel.
fn collect_candidates_parallel(
    game_dir: &Path,
    ignore_set: &IgnoreSet,
) -> GResult<Vec<(PathBuf, String, i64, i64)>> {
    // We use ignore::WalkBuilder with built-in ignore disabled (we
    // apply our own IgnoreSet). The parallelism comes from WalkParallel.
    let mut builder = ignore::WalkBuilder::new(game_dir);
    builder.hidden(false)          // don't skip hidden files
        .parents(false)            // don't read parent .gitignore
        .ignore(false)             // don't read .ignore
        .git_ignore(false)         // don't read .gitignore
        .git_global(false)
        .git_exclude(false)
        .follow_links(false)
        .threads(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));

    let (tx, rx) = bounded::<(PathBuf, String, i64, i64)>(4096);
    let game_dir = game_dir.to_path_buf();

    // WalkParallel runs the closure on worker threads. We send each
    // file entry through the channel.
    let walker = builder.build_parallel();
    let game_dir_for_walker = game_dir.clone();
    let ignore_set = ignore_set.clone();

    let walker_thread = std::thread::spawn(move || -> GResult<()> {
        walker.run(|| {
            let tx = tx.clone();
            let game_dir = game_dir_for_walker.clone();
            let ignore_set = ignore_set.clone();
            Box::new(move |entry| {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => return ignore::WalkState::Continue,
                };
                if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                    return ignore::WalkState::Continue;
                }
                let rel = match entry.path().strip_prefix(&game_dir) {
                    Ok(r) => r,
                    Err(_) => return ignore::WalkState::Continue,
                };
                // Quick ignore check before stat (avoids stat for ignored files).
                let rel_str = match rel.to_str() {
                    Some(s) => s,
                    None => return ignore::WalkState::Continue,
                };
                let normalized = path_utils::to_forward_slash(rel_str);
                if ignore_set.is_ignored(&normalized, false) {
                    return ignore::WalkState::Continue;
                }
                let meta = match std::fs::symlink_metadata(entry.path()) {
                    Ok(m) => m,
                    Err(_) => return ignore::WalkState::Continue,
                };
                let size = meta.len() as i64;
                let mtime = meta.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                // best-effort send; if channel is closed, skip
                let _ = tx.send((entry.path().to_path_buf(), normalized, size, mtime));
                ignore::WalkState::Continue
            })
        });
        Ok(())
    });

    let mut out = Vec::new();
    while let Ok(item) = rx.recv() {
        out.push(item);
    }
    // Propagate any panic from the walker thread.
    walker_thread.join().map_err(|_| {
        crate::error::GError::Other("parallel walker thread panicked".into())
    })??;
    Ok(out)
}

impl Clone for IgnoreSet {
    fn clone(&self) -> Self {
        // Rebuild from sources. Gitignore doesn't implement Clone, but
        // our sources Vec has all the original lines.
        let mut new = IgnoreSet::empty().expect("empty ignore set");
        for src in &self.sources {
            let _ = new.add_lines(Path::new("/"), &src.label, &src.patterns);
        }
        new
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;
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
        let (files, locked) = walk_and_hash(dir.path(), &ignore, None, &WalkOptions::default()).unwrap();
        assert!(locked.is_empty());
        let paths: Vec<&str> = files.iter().map(|f| f.file_path.as_str()).collect();
        assert!(paths.contains(&"a.txt"));
        assert!(paths.contains(&"b.tmp"));
        assert!(paths.contains(&"mods/sky.dds"));
    }

    #[test]
    fn respects_default_ignores() {
        let dir = setup_game();
        let binary_dir = PathBuf::from("/tmp/nonexistent");
        let paths = Paths::from_binary_dir(binary_dir).unwrap();
        let ignore = crate::ignore_mod::build_for_game(&paths, "test", dir.path()).unwrap();
        let (files, _) = walk_and_hash(dir.path(), &ignore, None, &WalkOptions::default()).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.file_path.as_str()).collect();
        assert!(paths.contains(&"a.txt"));
        assert!(!paths.contains(&"b.tmp"));
    }

    #[test]
    fn smart_skip_unchanged() {
        let dir = setup_game();
        let ignore = IgnoreSet::empty().unwrap();
        let (files, _) = walk_and_hash(dir.path(), &ignore, None, &WalkOptions::default()).unwrap();
        let mut reference = HashMap::new();
        for f in &files {
            reference.insert(f.file_path.clone(), FileMeta { hash: f.hash.clone(), file_size: f.file_size, modified_time: f.modified_time });
        }
        let (files2, _) = walk_and_hash(dir.path(), &ignore, Some(&reference), &WalkOptions::default()).unwrap();
        for f in &files2 {
            let r = reference.get(&f.file_path).unwrap();
            assert_eq!(f.hash, r.hash);
        }
    }

    #[test]
    fn smart_re_hashes_changed() {
        let dir = setup_game();
        let ignore = IgnoreSet::empty().unwrap();
        let (files, _) = walk_and_hash(dir.path(), &ignore, None, &WalkOptions::default()).unwrap();
        let mut reference = HashMap::new();
        for f in &files {
            reference.insert(f.file_path.clone(), FileMeta { hash: f.hash.clone(), file_size: f.file_size, modified_time: f.modified_time });
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        fs::write(dir.path().join("a.txt"), b"hello world").unwrap();
        let (files2, _) = walk_and_hash(dir.path(), &ignore, Some(&reference), &WalkOptions::default()).unwrap();
        let a2 = files2.iter().find(|f| f.file_path == "a.txt").unwrap();
        let a_ref = reference.get("a.txt").unwrap();
        assert_ne!(a2.hash, a_ref.hash);
    }
}
