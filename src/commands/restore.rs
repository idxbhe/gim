//! `g restore` — restore the game directory to match a specific snapshot.
//!
//! Per spec:
//! - Without `--full`: hashes current game directory to determine minimal
//!   set of changes.
//! - With `--full`: skips current-state hashing, overwrites all files from
//!   the target snapshot.
//!
//! Pipeline:
//! 1. VALIDATE
//! 2. BUILD TARGET STATE (query files table for target snapshot)
//! 3. SCAN & HASH CURRENT STATE (skipped if `--full`)
//! 4. DIFF (target vs current — O(n+m))
//! 5. PREVIEW (if `--dry-run`)
//! 6. EXECUTE (parallel copy + delete)
//! 7. REPORT

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::hashing::Hash;
use crate::ignore_mod;
use crate::locking;
use crate::output::Colorizer;
use crate::output::format_size;
use crate::storage::Cas;
use crate::walker::{walk_and_hash, WalkOptions};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub fn run(
    colorizer: &Colorizer,
    alias: String,
    snapshot_id: String,
    full: bool,
    threads: Option<usize>,
    dry_run: bool,
) -> GResult<()> {
    // ── 1. VALIDATE ─────────────────────────────────────────────────
    let mut paths = Paths::from_env()?;
    if let Some(override_dir) = env_data_dir_override() {
        paths = paths.with_data_dir(override_dir);
    }
    paths.ensure_data_dir()?;

    let games_db = GamesDb::open(&paths.games_db)?;
    let game = games_db
        .get(&alias)?
        .ok_or_else(|| GError::AliasNotFound(alias.clone()))?;

    if !game.game_dir.exists() {
        return Err(GError::GameDirMissing(game.game_dir.clone()));
    }

    let snaps_db_path = paths.snaps_db(&alias);
    let snaps_db = SnapsDb::open(&snaps_db_path)?;
    let _snap = snaps_db
        .get_snapshot(&snapshot_id)?
        .ok_or_else(|| GError::SnapshotNotFound(snapshot_id.clone(), alias.clone()))?;

    let _lock = locking::acquire_game_lock(&alias, &snaps_db_path)?;

    // ── 2. BUILD TARGET STATE ───────────────────────────────────────
    let target_map: HashMap<String, (Hash, i64)> =
        snaps_db.files_for_snapshot(&snapshot_id)?;

    // ── 3. SCAN & HASH CURRENT STATE (skipped if --full) ────────────
    // In --full mode we still walk the directory to learn which files
    // exist on disk (so we can delete the ones not in target_map), but
    // we skip hashing since we'll overwrite everything anyway.
    let current_map: HashMap<String, (Hash, i64)> = if full {
        let ignore_set = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;
        let paths_on_disk = crate::walker::walk_only(&game.game_dir, &ignore_set)?;
        // Use a sentinel hash so that every file in current_map is
        // treated as "differs from target" by the diff logic — but
        // we filter `to_copy` separately below so this only affects
        // `to_delete` (files in current but not in target).
        paths_on_disk
            .into_iter()
            .map(|p| (p, (Hash("__sentinel_full__".into()), 0)))
            .collect()
    } else {
        let ignore_set = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;
        let walk_opts = WalkOptions {
            threads: threads.unwrap_or(0),
            ..WalkOptions::default()
        };
        let (hashed, _locked) = walk_and_hash(&game.game_dir, &ignore_set, &walk_opts)?;
        let mut m = HashMap::with_capacity(hashed.len());
        for f in hashed {
            m.insert(f.file_path, (f.hash, f.file_size));
        }
        m
    };

    // ── 4. DIFF (target vs current) ─────────────────────────────────
    // For restore: we want to_copy (in target but missing or different
    // in current) and to_delete (in current but not in target).
    let mut to_copy: Vec<(String, Hash, i64)> = Vec::new();
    let mut to_delete: Vec<String> = Vec::new();
    if full {
        // In --full mode, copy ALL target files unconditionally (we
        // cannot trust the current on-disk state).
        for (path, (hash, size)) in &target_map {
            to_copy.push((path.clone(), hash.clone(), *size));
        }
    } else {
        for (path, (hash, size)) in &target_map {
            match current_map.get(path) {
                None => to_copy.push((path.clone(), hash.clone(), *size)),
                Some((ch, _)) if ch != hash => {
                    to_copy.push((path.clone(), hash.clone(), *size))
                }
                _ => {} // unchanged
            }
        }
    }
    for path in current_map.keys() {
        if !target_map.contains_key(path) {
            to_delete.push(path.clone());
        }
    }

    // ── 5. PREVIEW (if --dry-run) ───────────────────────────────────
    if dry_run {
        println!(
            "dry run: would restore {} to {}",
            colorizer.green(&alias),
            colorizer.bold(&snapshot_id)
        );
        println!();
        println!("  to copy ({}):", to_copy.len());
        for (p, _, s) in &to_copy {
            println!("    + {} ({})", colorizer.green(p), format_size(*s));
        }
        println!("  to delete ({}):", to_delete.len());
        for p in &to_delete {
            println!("    - {}", colorizer.red(p));
        }
        return Ok(());
    }

    // ── 6. EXECUTE (parallel) ───────────────────────────────────────
    let cas = Cas::new(paths.objects_dir(&alias));
    let game_dir = game.game_dir.clone();

    // Copy in parallel
    let pool = if let Some(n) = threads {
        Some(
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .map_err(|e| GError::Other(format!("cannot build thread pool: {e}")))?,
        )
    } else {
        None
    };

    let copy_results: Vec<Result<(), String>> = match &pool {
        Some(p) => p.install(|| copy_all(&cas, &game_dir, &to_copy)),
        None => copy_all(&cas, &game_dir, &to_copy),
    };
    let mut errors: Vec<String> = Vec::new();
    for r in copy_results {
        if let Err(e) = r {
            errors.push(e);
        }
    }

    // Delete (serial — filesystem operations are usually fast for delete)
    for path in &to_delete {
        let abs = crate::path_utils::denormalize(&game_dir, path);
        if abs.exists() {
            if let Err(e) = fs::remove_file(&abs) {
                errors.push(format!("delete {path}: {e}"));
            }
        }
    }

    // Clean up empty directories left by deleted files
    cleanup_empty_dirs(&game_dir, &to_delete);

    // ── 7. REPORT ───────────────────────────────────────────────────
    println!(
        "restored {} to {}",
        colorizer.green(&alias),
        colorizer.bold(&snapshot_id)
    );
    println!(
        "  {} files copied, {} files deleted",
        to_copy.len(),
        to_delete.len()
    );
    if !errors.is_empty() {
        eprintln!("warning: {} error(s) during restore:", errors.len());
        for e in &errors {
            eprintln!("  {e}");
        }
    }
    Ok(())
}

fn copy_all(
    cas: &Cas,
    game_dir: &Path,
    to_copy: &[(String, Hash, i64)],
) -> Vec<Result<(), String>> {
    to_copy
        .par_iter()
        .map(|(path, hash, _size)| {
            let abs = crate::path_utils::denormalize(game_dir, path);
            // Ensure parent directories exist
            if let Some(parent) = abs.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return Err(format!("mkdir {path}: {e}"));
                }
            }
            // Copy from CAS to game dir
            let mut src = match cas.open(hash) {
                Ok(f) => f,
                Err(e) => return Err(format!("open object {hash}: {e}")),
            };
            let mut dst = match fs::File::create(&abs) {
                Ok(f) => f,
                Err(e) => return Err(format!("create {path}: {e}")),
            };
            let mut buf = vec![0u8; 1024 * 1024];
            loop {
                let n = match src.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => return Err(format!("read object {hash}: {e}")),
                };
                if let Err(e) = dst.write_all(&buf[..n]) {
                    return Err(format!("write {path}: {e}"));
                }
            }
            let _ = dst.sync_all();
            Ok(())
        })
        .collect()
}

/// Remove empty directories left behind by file deletions. Walks up
/// from each deleted file's parent until it hits a non-empty directory
/// or the game root.
fn cleanup_empty_dirs(game_dir: &Path, deleted_paths: &[String]) {
    let mut checked: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for p in deleted_paths {
        let abs = crate::path_utils::denormalize(game_dir, p);
        let mut cur = abs.parent().map(|x| x.to_path_buf());
        while let Some(dir) = cur {
            if dir == game_dir || checked.contains(&dir) {
                break;
            }
            checked.insert(dir.clone());
            // Try to remove; if it's non-empty, the OS will error and we stop.
            if fs::remove_dir(&dir).is_err() {
                break;
            }
            cur = dir.parent().map(|x| x.to_path_buf());
        }
    }
}
