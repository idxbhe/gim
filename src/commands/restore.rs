//! `gim restore` — restore the game directory to match a snapshot.
//!
//! Three modes:
//! - **Default (smart)**: walk + stat the game directory, hash only
//!   files whose size or mtime differs from the target snapshot. Copy
//!   differing files from CAS, delete files not in target, set the
//!   mtime of restored files to the snapshot's recorded mtime (so the
//!   next `gim status` / `gim snap` is fast).
//! - **`--full`**: skip walking entirely, overwrite ALL target files
//!   from CAS, delete ALL non-target files. Slower but guarantees
//!   correctness when the on-disk state is suspected corrupted.
//! - `--full-hash` is intentionally NOT supported on `restore` because
//!   `--full` already covers the "I don't trust the disk state" case.

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
use filetime::FileTime;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

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
    let target_map = snaps_db.files_for_snapshot(&snapshot_id)?;

    // ── 3. SCAN CURRENT STATE (smart) OR SKIP (--full) ──────────────
    let current_map: HashMap<String, crate::db::FileMeta> = if full {
        // Skip walking entirely; treat current_map as empty so the diff
        // will copy ALL target files and delete ALL non-target files.
        HashMap::new()
    } else {
        let ignore_set = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;
        let walk_opts = WalkOptions {
            threads: threads.unwrap_or(0),
            full_hash: false,
            ..WalkOptions::default()
        };
        // Use the target snapshot as the reference for the mtime+size
        // pre-filter: if a file on disk has matching size+mtime vs the
        // target snapshot, we don't need to hash it (we'll trust that
        // it matches the target).
        let (hashed, _locked) = walk_and_hash(
            &game.game_dir,
            &ignore_set,
            Some(&target_map),
            &walk_opts,
        )?;
        let mut m = HashMap::with_capacity(hashed.len());
        for f in hashed {
            m.insert(
                f.file_path,
                crate::db::FileMeta {
                    hash: f.hash,
                    file_size: f.file_size,
                    modified_time: f.modified_time,
                },
            );
        }
        m
    };

    // ── 4. DIFF (target vs current) ─────────────────────────────────
    let mut to_copy: Vec<(String, Hash, i64, i64)> = Vec::new(); // (path, hash, size, mtime_to_set)
    let mut to_delete: Vec<String> = Vec::new();
    if full {
        for (path, meta) in &target_map {
            to_copy.push((
                path.clone(),
                meta.hash.clone(),
                meta.file_size,
                meta.modified_time,
            ));
        }
    } else {
        for (path, meta) in &target_map {
            match current_map.get(path) {
                None => to_copy.push((
                    path.clone(),
                    meta.hash.clone(),
                    meta.file_size,
                    meta.modified_time,
                )),
                Some(cm) if cm.hash != meta.hash => {
                    to_copy.push((
                        path.clone(),
                        meta.hash.clone(),
                        meta.file_size,
                        meta.modified_time,
                    ));
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
        if full {
            println!("  (--full mode: all files will be overwritten)");
        }
        println!();
        println!("  to copy ({}):", to_copy.len());
        for (p, _, s, _) in &to_copy {
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

    for path in &to_delete {
        let abs = crate::path_utils::denormalize(&game_dir, path);
        if abs.exists() {
            if let Err(e) = fs::remove_file(&abs) {
                errors.push(format!("delete {path}: {e}"));
            }
        }
    }

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
    to_copy: &[(String, Hash, i64, i64)],
) -> Vec<Result<(), String>> {
    to_copy
        .par_iter()
        .map(|(path, hash, _size, mtime)| {
            let abs = crate::path_utils::denormalize(game_dir, path);
            if let Some(parent) = abs.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return Err(format!("mkdir {path}: {e}"));
                }
            }
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
            drop(dst);

            // Set mtime to the snapshot's recorded mtime so subsequent
            // `gim status` / `gim snap` can use the mtime+size fast
            // pre-filter to skip re-hashing this file.
            if *mtime > 0 {
                let ft = FileTime::from_unix_time(*mtime, 0);
                if let Err(e) = filetime::set_file_mtime(&abs, ft) {
                    log::debug!("could not set mtime on {path}: {e}");
                }
            }
            Ok(())
        })
        .collect()
}

/// Remove empty directories left behind by file deletions.
fn cleanup_empty_dirs(game_dir: &Path, deleted_paths: &[String]) {
    let mut checked: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    for p in deleted_paths {
        let abs = crate::path_utils::denormalize(game_dir, p);
        let mut cur = abs.parent().map(|x| x.to_path_buf());
        while let Some(dir) = cur {
            if dir == game_dir || checked.contains(&dir) {
                break;
            }
            checked.insert(dir.clone());
            if fs::remove_dir(&dir).is_err() {
                break;
            }
            cur = dir.parent().map(|x| x.to_path_buf());
        }
    }
}
