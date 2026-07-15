//! Parallel file walker + hasher with mtime+size pre-filter + progress.
//!
//! Supports per-snap filtering via `SnapFilter` (`--exclude` / `--include-only`).
//! Files filtered out by SnapFilter are NOT walked — the snap command
//! inherits them from the parent snapshot as "unchanged".

use crate::db::FileMeta;
use crate::error::{GError, GResult};
use crate::hashing::{hash_file_with_retry, Hash, HashAlgorithm};
use crate::ignore_mod::IgnoreSet;
use crate::output::ProgressReporter;
use crate::parallel;
use crate::path_utils;
use crossbeam_channel::bounded;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct HashedFile { pub file_path: String, pub hash: Hash, pub file_size: i64, pub modified_time: i64 }
#[derive(Debug, Clone)]
pub struct LockedFile { pub file_path: String, pub error: String }

#[derive(Debug, Clone)]
pub struct WalkOptions {
    pub threads: usize,
    pub max_retries: u32,
    pub retry_delay: Duration,
    pub full_hash: bool,
    pub algorithm: HashAlgorithm,
    pub parallel: bool,
}
impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            threads: 0, max_retries: 3, retry_delay: Duration::from_millis(500),
            full_hash: false, algorithm: HashAlgorithm::Xxhash, parallel: true,
        }
    }
}

/// Per-snap filter for `--exclude` and `--include-only` flags.
///
/// Uses gitignore-style pattern matching (same syntax as `.gignore`).
/// Applied AFTER the permanent `.gignore` filter.
///
/// Files skipped by this filter are NOT lost — the snap command
/// inherits them from the parent snapshot as "unchanged".
pub struct SnapFilter {
    exclude: Option<Gitignore>,
    include_only: Option<Gitignore>,
}

impl SnapFilter {
    pub fn new(exclude_patterns: &[String], include_only_patterns: &[String]) -> GResult<Self> {
        let exclude = if exclude_patterns.is_empty() { None } else { Some(build_gitignore(exclude_patterns)?) };
        let include_only = if include_only_patterns.is_empty() { None } else { Some(build_gitignore(include_only_patterns)?) };
        Ok(Self { exclude, include_only })
    }

    pub fn is_empty(&self) -> bool { self.exclude.is_none() && self.include_only.is_none() }

    /// Check if a file path should be walked. Returns false if excluded
    /// or not matching include_only.
    ///
    /// For directory patterns (e.g. `Config/`), we check ALL ancestor
    /// directories of the file path with `is_dir=true`. Without this,
    /// a pattern like `Config/` would not match individual files inside
    /// the directory (e.g. `Config/settings.ini`) because gitignore
    /// directory patterns only match the directory itself, not its
    /// contents.
    pub fn should_walk(&self, path: &str) -> bool {
        // Check the file itself (is_dir=false).
        if let Some(ref ex) = self.exclude {
            if matches!(ex.matched(Path::new(path), false), ignore::Match::Ignore(_)) { return false; }
        }
        if let Some(ref inc) = self.include_only {
            if !matches!(inc.matched(Path::new(path), false), ignore::Match::Ignore(_)) { return false; }
        }

        // Check ancestor directories for directory patterns.
        // E.g. for path "WellingtonGame/Config/DefaultSystemSettings.ini",
        // check ancestors:
        //   "WellingtonGame" (is_dir=true)
        //   "WellingtonGame/Config" (is_dir=true) ← matches "WellingtonGame/Config/"
        //
        // This is needed because gitignore directory patterns (trailing /)
        // only match with is_dir=true, not is_dir=false.
        let parts: Vec<&str> = path.split('/').collect();
        for i in 1..parts.len() {
            let ancestor: String = parts[..i].join("/");
            if let Some(ref ex) = self.exclude {
                if matches!(ex.matched(Path::new(&ancestor), true), ignore::Match::Ignore(_)) {
                    return false;
                }
            }
            // For include_only, we do NOT check ancestors — the file
            // must match the include_only pattern itself. A pattern like
            // `mods/*` matches files directly, not via directory ancestors.
        }

        true
    }

    /// Should this directory be pruned (entire subtree skipped)?
    /// Only prunes on `--exclude` directory matches. For `--include_only`,
    /// we don't prune (a subdir might contain matching files).
    pub fn should_skip_dir(&self, dir: &str) -> bool {
        if let Some(ref ex) = self.exclude {
            if matches!(ex.matched(Path::new(dir), true), ignore::Match::Ignore(_)) { return true; }
        }
        false
    }
}

/// Build a Gitignore matcher from pattern strings.
fn build_gitignore(patterns: &[String]) -> GResult<Gitignore> {
    let mut builder = GitignoreBuilder::new("");
    for p in patterns {
        builder.add_line(None, p).map_err(|e| GError::Other(format!("invalid pattern \"{p}\": {e}")))?;
    }
    builder.build().map_err(|e| GError::Other(format!("gitignore build: {e}")))
}

/// Walk + hash with progress reporting.
pub fn walk_and_hash(
    game_dir: &Path,
    ignore_set: &IgnoreSet,
    reference: Option<&HashMap<String, FileMeta>>,
    opts: &WalkOptions,
    progress: &ProgressReporter,
    snap_filter: Option<&SnapFilter>,
) -> GResult<(Vec<HashedFile>, Vec<LockedFile>)> {
    let use_smart = !opts.full_hash && reference.is_some();
    let reference = Arc::new(reference.cloned().unwrap_or_default());

    progress.walk_start();
    let candidates = collect_candidates_parallel(game_dir, ignore_set, progress, snap_filter)?;
    let walk_count = candidates.len() as u64;
    progress.walk_done(walk_count);

    progress.hash_start(candidates.len());
    let results: Vec<HashResult> = if opts.parallel {
        let pool = parallel::global();
        pool.install(|| {
            candidates.par_iter().map(|(path, normalized, size, mtime)| {
                hash_one(path, normalized, *size, *mtime, &reference, opts, use_smart, progress)
            }).collect()
        })
    } else {
        candidates.iter().map(|(path, normalized, size, mtime)| {
            hash_one(path, normalized, *size, *mtime, &reference, opts, use_smart, progress)
        }).collect()
    };
    let hash_count = results.len() as u64;
    progress.hash_done(hash_count);

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

fn hash_one(
    path: &Path, normalized: &str, size: i64, mtime: i64,
    reference: &HashMap<String, FileMeta>, opts: &WalkOptions,
    use_smart: bool, progress: &ProgressReporter,
) -> HashResult {
    let need_hash = if use_smart {
        match reference.get(normalized) {
            Some(m) => m.file_size != size || m.modified_time != mtime,
            None => true,
        }
    } else { true };

    let result = if !need_hash {
        let m = reference.get(normalized).expect("checked");
        HashResult::Ok { file_path: normalized.to_string(), hash: m.hash.clone(), file_size: size, modified_time: mtime }
    } else {
        match hash_file_with_retry(path, opts.algorithm, opts.max_retries, opts.retry_delay) {
            Ok(Some((h, _))) => HashResult::Ok { file_path: normalized.to_string(), hash: h, file_size: size, modified_time: mtime },
            Ok(None) => HashResult::Locked { file_path: normalized.to_string(), error: format!("locked after {} retries", opts.max_retries) },
            Err(e) => HashResult::Locked { file_path: normalized.to_string(), error: format!("{e}") },
        }
    };
    progress.hash_tick();
    result
}

/// Walk-only (no hashing). Used by `gim restore --full`.
pub fn walk_only(game_dir: &Path, ignore_set: &IgnoreSet, progress: &ProgressReporter) -> GResult<Vec<String>> {
    progress.walk_start();
    let candidates = collect_candidates_parallel(game_dir, ignore_set, progress, None)?;
    let count = candidates.len() as u64;
    progress.walk_done(count);
    Ok(candidates.into_iter().map(|(_, n, _, _)| n).collect())
}

fn collect_candidates_parallel(
    game_dir: &Path,
    ignore_set: &IgnoreSet,
    progress: &ProgressReporter,
    snap_filter: Option<&SnapFilter>,
) -> GResult<Vec<(PathBuf, String, i64, i64)>> {
    // We filter AFTER collecting candidates. This is simpler and avoids
    // the Gitignore clone problem (Gitignore doesn't implement Clone,
    // and WalkParallel needs 'static closures). The stat overhead for
    // filtered files is negligible compared to hashing.
    // hashing. And it avoids the Gitignore clone problem entirely.

    let mut builder = ignore::WalkBuilder::new(game_dir);
    builder.hidden(false).parents(false).ignore(false).git_ignore(false).git_global(false).git_exclude(false).follow_links(false)
        .threads(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));

    let (tx, rx) = bounded::<(PathBuf, String, i64, i64)>(4096);
    let game_dir2 = game_dir.to_path_buf();
    let ignore_set2 = ignore_set.clone();
    let walker = builder.build_parallel();

    let wt = std::thread::spawn(move || -> GResult<()> {
        walker.run(|| {
            let tx = tx.clone();
            let gd = game_dir2.clone();
            let ig = ignore_set2.clone();
            Box::new(move |entry| {
                let entry = match entry { Ok(e) => e, Err(_) => return ignore::WalkState::Continue };
                let ft = match entry.file_type() { Some(ft) => ft, None => return ignore::WalkState::Continue };
                let rel = match entry.path().strip_prefix(&gd) { Ok(r) => r, Err(_) => return ignore::WalkState::Continue };
                let rel_str = match rel.to_str() { Some(s) => s, None => return ignore::WalkState::Continue };
                let norm = path_utils::to_forward_slash(rel_str);

                if ft.is_dir() {
                    if !norm.is_empty() && ig.is_ignored(&norm, true) {
                        return ignore::WalkState::Skip;
                    }
                    return ignore::WalkState::Continue;
                }

                if ft.is_file() {
                    if ig.is_ignored(&norm, false) {
                        return ignore::WalkState::Continue;
                    }
                    let meta = match std::fs::symlink_metadata(entry.path()) { Ok(m) => m, Err(_) => return ignore::WalkState::Continue };
                    let size = meta.len() as i64;
                    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                    let _ = tx.send((entry.path().to_path_buf(), norm, size, mtime));
                }
                ignore::WalkState::Continue
            })
        });
        Ok(())
    });

    let mut out = Vec::new();
    while let Ok(item) = rx.recv() {
        // Apply snap_filter AFTER collection. This is simpler and
        // avoids the Gitignore clone problem. The stat overhead for
        // filtered files is negligible compared to hashing.
        if let Some(ref filter) = snap_filter {
            if !filter.should_walk(&item.1) {
                continue;
            }
        }
        out.push(item);
        progress.walk_tick();
    }
    wt.join().map_err(|_| crate::error::GError::Other("walker thread panicked".into()))??;
    Ok(out)
}

#[cfg(test)]
mod pattern_tests {
    use super::*;

    #[test]
    fn exclude_dir_pattern_matches_files_inside() {
        let filter = SnapFilter::new(
            &["WellingtonGame/Config/".to_string()],
            &[],
        ).unwrap();
        // File inside the excluded directory should NOT be walked.
        assert!(!filter.should_walk("WellingtonGame/Config/DefaultSystemSettings.ini"));
        assert!(!filter.should_walk("WellingtonGame/Config/subdir/file.txt"));
        // File outside should be walked.
        assert!(filter.should_walk("WellingtonGame/game.exe"));
        assert!(filter.should_walk("other.txt"));
    }

    #[test]
    fn exclude_glob_pattern_matches() {
        let filter = SnapFilter::new(
            &["*.log".to_string()],
            &[],
        ).unwrap();
        assert!(!filter.should_walk("debug.log"));
        assert!(!filter.should_walk("logs/error.log"));
        assert!(filter.should_walk("game.exe"));
    }

    #[test]
    fn include_only_matches() {
        let filter = SnapFilter::new(
            &[],
            &["mods/*".to_string()],
        ).unwrap();
        assert!(filter.should_walk("mods/sky.dds"));
        assert!(!filter.should_walk("game.exe"));
    }
}
