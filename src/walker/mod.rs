//! Parallel file walker + hasher with mtime+size pre-filter + progress.
//!
//! The walker accepts an optional [`ProgressReporter`] that displays:
//! - A spinner during the walk phase (unknown total).
//! - A progress bar during the hash phase (known total, with percentage).
//!
//! Progress output goes to stderr (via indicatif), so stdout (including
//! `--json` output) is never corrupted.

use crate::db::FileMeta;
use crate::error::GResult;
use crate::hashing::{hash_file_with_retry, Hash};
use crate::ignore_mod::IgnoreSet;
use crate::output::ProgressReporter;
use crate::parallel;
use crate::path_utils;
use crossbeam_channel::bounded;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct HashedFile { pub file_path: String, pub hash: Hash, pub file_size: i64, pub modified_time: i64 }
#[derive(Debug, Clone)]
pub struct LockedFile { pub file_path: String, pub error: String }

#[derive(Debug, Clone)]
pub struct WalkOptions { pub threads: usize, pub max_retries: u32, pub retry_delay: Duration, pub full_hash: bool }
impl Default for WalkOptions {
    fn default() -> Self { Self { threads: 0, max_retries: 3, retry_delay: Duration::from_millis(500), full_hash: false } }
}

/// Walk + hash with progress reporting.
///
/// `progress` controls the visual feedback. When `progress.enabled()` is
/// false, all tick calls are no-ops (zero overhead).
pub fn walk_and_hash(
    game_dir: &Path,
    ignore_set: &IgnoreSet,
    reference: Option<&HashMap<String, FileMeta>>,
    opts: &WalkOptions,
    progress: &ProgressReporter,
) -> GResult<(Vec<HashedFile>, Vec<LockedFile>)> {
    let use_smart = !opts.full_hash && reference.is_some();
    let reference = std::sync::Arc::new(reference.cloned().unwrap_or_default());

    // ── Walk phase (parallel, spinner) ──────────────────────────────
    progress.walk_start();
    let candidates = collect_candidates_parallel(game_dir, ignore_set, progress)?;
    let walk_count = candidates.len() as u64;
    progress.walk_done(walk_count);

    // ── Hash phase (parallel, progress bar) ─────────────────────────
    progress.hash_start(candidates.len());
    let pool = parallel::global();
    let results: Vec<HashResult> = pool.install(|| {
        candidates
            .par_iter()
            .map(|(path, normalized, size, mtime)| {
                let need_hash = if use_smart {
                    match reference.get(normalized) {
                        Some(m) => m.file_size != *size || m.modified_time != *mtime,
                        None => true,
                    }
                } else { true };

                let result = if !need_hash {
                    let m = reference.get(normalized).expect("checked");
                    HashResult::Ok { file_path: normalized.clone(), hash: m.hash.clone(), file_size: *size, modified_time: *mtime }
                } else {
                    match hash_file_with_retry(path, opts.max_retries, opts.retry_delay) {
                        Ok(Some((h, _))) => HashResult::Ok { file_path: normalized.clone(), hash: h, file_size: *size, modified_time: *mtime },
                        Ok(None) => HashResult::Locked { file_path: normalized.clone(), error: format!("locked after {} retries", opts.max_retries) },
                        Err(e) => HashResult::Locked { file_path: normalized.clone(), error: format!("{e}") },
                    }
                };
                // Tick AFTER processing — shows files completed, not started.
                progress.hash_tick();
                result
            })
            .collect()
    });
    let hash_count = results.len() as u64;
    progress.hash_done(hash_count);

    let mut files = Vec::with_capacity(results.len());
    let mut locked = Vec::new();
    for r in results {
        match r {
            HashResult::Ok { file_path, hash, file_size, modified_time } => files.push(HashedFile { file_path, hash, file_size, modified_time }),
            HashResult::Locked { file_path, error } => locked.push(LockedFile { file_path, error }),
        }
    }
    Ok((files, locked))
}

enum HashResult {
    Ok { file_path: String, hash: Hash, file_size: i64, modified_time: i64 },
    Locked { file_path: String, error: String },
}

/// Walk-only (no hashing). Used by `gim restore --full`.
pub fn walk_only(game_dir: &Path, ignore_set: &IgnoreSet, progress: &ProgressReporter) -> GResult<Vec<String>> {
    progress.walk_start();
    let candidates = collect_candidates_parallel(game_dir, ignore_set, progress)?;
    let count = candidates.len() as u64;
    progress.walk_done(count);
    Ok(candidates.into_iter().map(|(_, n, _, _)| n).collect())
}

fn collect_candidates_parallel(
    game_dir: &Path,
    ignore_set: &IgnoreSet,
    progress: &ProgressReporter,
) -> GResult<Vec<(PathBuf, String, i64, i64)>> {
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
                if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) { return ignore::WalkState::Continue; }
                let rel = match entry.path().strip_prefix(&gd) { Ok(r) => r, Err(_) => return ignore::WalkState::Continue };
                let rel_str = match rel.to_str() { Some(s) => s, None => return ignore::WalkState::Continue };
                let norm = path_utils::to_forward_slash(rel_str);
                if ig.is_ignored(&norm, false) { return ignore::WalkState::Continue; }
                let meta = match std::fs::symlink_metadata(entry.path()) { Ok(m) => m, Err(_) => return ignore::WalkState::Continue };
                let size = meta.len() as i64;
                let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                let _ = tx.send((entry.path().to_path_buf(), norm, size, mtime));
                ignore::WalkState::Continue
            })
        });
        Ok(())
    });

    let mut out = Vec::new();
    while let Ok(item) = rx.recv() {
        out.push(item);
        progress.walk_tick();
    }
    wt.join().map_err(|_| crate::error::GError::Other("walker thread panicked".into()))??;
    Ok(out)
}
