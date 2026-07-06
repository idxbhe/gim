//! `gim snap` — take a snapshot of the game directory.
//!
//! Pipeline (v0.2 with mtime+size pre-filter):
//! 1. VALIDATE — alias, gameDir, advisory lock.
//! 2. WALK+STAT — walk + apply ignore patterns, collect (path, size,
//!    mtime) for each surviving file. No file content I/O in this pass.
//! 3. SMART HASH — for each file:
//!    - Not in parent snapshot → hash (new file).
//!    - In parent, but size or mtime differs → hash to verify.
//!    - In parent AND size+mtime match → SKIP hashing, reuse parent's hash.
//!    If `--full-hash` was passed, hash everything unconditionally.
//! 4. DIFF — against parent snapshot's file map (hash-based).
//! 5. PREVIEW — if `--dry-run`, print diff and exit.
//! 6. STORE — atomic transaction: copy new/modified objects to CAS,
//!    insert into `files` (with mtime) and `deleted_files`, insert
//!    `snaps` row.
//! 7. REPORT.

use crate::config::{env_data_dir_override, Paths};
use crate::db::{diff_states, FileEntry, FileMeta, GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::hashing::Hash;
use crate::ignore_mod;
use crate::locking;
use crate::output::Colorizer;
use crate::output::format_size;
use crate::storage::Cas;
use crate::walker::{walk_and_hash, WalkOptions};
use std::collections::HashMap;

pub fn run(
    colorizer: &Colorizer,
    alias: String,
    custom_id: Option<String>,
    msg: Option<String>,
    threads: Option<usize>,
    dry_run: bool,
    full_hash: bool,
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
    if !game.game_dir.is_dir() {
        return Err(GError::GameDirNotDir(game.game_dir.clone()));
    }

    let snaps_db_path = paths.snaps_db(&alias);
    let mut snaps_db = SnapsDb::open(&snaps_db_path)?;

    let _lock = locking::acquire_game_lock(&alias, &snaps_db_path)?;

    // ── Resolve snapshot ID & parent ────────────────────────────────
    let latest = snaps_db.latest_snapshot()?;
    let parent_id = latest.as_ref().map(|s| s.snapshot_id.clone());

    let snapshot_id = match custom_id {
        Some(id) => {
            validate_snapshot_id(&id)?;
            if snaps_db.get_snapshot(&id)?.is_some() {
                return Err(GError::SnapshotIdExists(id, alias.clone()));
            }
            id
        }
        None => match &latest {
            None => "original".to_string(),
            Some(_) => chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string(),
        },
    };

    // ── 2. WALK + 3. SMART HASH ──────────────────────────────────────
    let ignore_set = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;
    let parent_files: HashMap<String, FileMeta> = match &parent_id {
        Some(pid) => snaps_db.files_for_snapshot(pid)?,
        None => HashMap::new(),
    };
    let walk_opts = WalkOptions {
        threads: threads.unwrap_or(0),
        full_hash,
        ..WalkOptions::default()
    };
    // Smart walk: pass parent_files as reference. If full_hash is true,
    // the walker will ignore the reference and hash everything.
    let reference = if full_hash { None } else { Some(&parent_files) };
    let (hashed_files, locked_files) =
        walk_and_hash(&game.game_dir, &ignore_set, reference, &walk_opts)?;

    // Build current_state map: path → (hash, size, mtime)
    let mut current_state: HashMap<String, FileMeta> = HashMap::with_capacity(hashed_files.len());
    for f in &hashed_files {
        current_state.insert(
            f.file_path.clone(),
            FileMeta {
                hash: f.hash.clone(),
                file_size: f.file_size,
                modified_time: f.modified_time,
            },
        );
    }

    // ── 4. DIFF against parent snapshot ─────────────────────────────
    let diff = diff_states(&parent_files, &current_state);

    // ── 5. PREVIEW (if --dry-run) ───────────────────────────────────
    if dry_run {
        print_dry_run(&alias, &snapshot_id, &diff, &locked_files, colorizer, full_hash);
        return Ok(());
    }

    if diff.total_changes() == 0 {
        println!("no changes detected, snapshot skipped");
        return Ok(());
    }

    // ── 6. STORE (atomic transaction) ───────────────────────────────
    let cas = Cas::new(paths.objects_dir(&alias));
    cas.ensure()?;

    // 6a. Copy new/modified objects to CAS.
    let mut written_objects: Vec<(Hash, i64)> = Vec::new();
    let new_or_modified: Vec<&crate::walker::HashedFile> = hashed_files
        .iter()
        .filter(|f| match parent_files.get(&f.file_path) {
            None => true,
            Some(pm) => pm.hash != f.hash,
        })
        .collect();

    for f in &new_or_modified {
        if !cas.exists(&f.hash) {
            let abs_path = crate::path_utils::denormalize(&game.game_dir, &f.file_path);
            match cas.store_from(&abs_path, &f.hash) {
                Ok(()) => written_objects.push((f.hash.clone(), f.file_size)),
                Err(e) => {
                    for (h, _) in &written_objects {
                        let _ = cas.delete(h.as_str());
                    }
                    return Err(e);
                }
            }
        }
    }

    // 6b. Insert snapshot record + files + deleted_files inside one tx.
    let tx_result: GResult<()> = {
        let tx = snaps_db.transaction()?;
        SnapsDb::insert_snap(
            &tx,
            &snapshot_id,
            parent_id.as_deref(),
            SnapsDb::now_ms(),
            msg.as_deref(),
            current_state.len() as i64,
            diff.added_size(),
        )?;

        let all_files: Vec<FileEntry> = hashed_files
            .iter()
            .map(|f| FileEntry {
                file_path: f.file_path.clone(),
                hash: f.hash.clone(),
                file_size: f.file_size,
                modified_time: f.modified_time,
            })
            .collect();
        SnapsDb::insert_files(&tx, &snapshot_id, &all_files)?;
        SnapsDb::insert_deleted_files(&tx, &snapshot_id, &diff.deleted)?;
        tx.commit()?;
        Ok(())
    };

    if let Err(e) = tx_result {
        for (h, _) in &written_objects {
            let _ = cas.delete(h.as_str());
        }
        return Err(e);
    }

    // ── 7. REPORT ───────────────────────────────────────────────────
    let added_size = diff.added_size();
    let new_or_modified_count = diff.added.len() + diff.modified.len();
    println!(
        "snapshotted {} as {}",
        colorizer.green(&alias),
        colorizer.bold(&snapshot_id)
    );
    println!(
        "  {} files tracked, {} new/modified, {} deleted, added {}",
        current_state.len(),
        new_or_modified_count,
        diff.deleted.len(),
        format_size(added_size)
    );

    if !locked_files.is_empty() {
        println!();
        println!(
            "warning: {} file(s) could not be read (may be locked by another process):",
            locked_files.len()
        );
        for lf in &locked_files {
            println!("  {}", lf.file_path);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn print_dry_run(
    alias: &str,
    snapshot_id: &str,
    diff: &crate::db::Diff,
    locked_files: &[crate::walker::LockedFile],
    colorizer: &Colorizer,
    full_hash: bool,
) {
    println!(
        "dry run: would snapshot {alias} as {id}",
        id = colorizer.bold(snapshot_id)
    );
    if full_hash {
        println!("  (full-hash mode: every file was re-hashed)");
    }
    println!();
    println!("  added ({}):", diff.added.len());
    for f in &diff.added {
        println!("    + {} ({})", colorizer.green(&f.file_path), format_size(f.file_size));
    }
    println!("  modified ({}):", diff.modified.len());
    for f in &diff.modified {
        println!("    ~ {}", colorizer.yellow(&f.file_path));
    }
    println!("  deleted ({}):", diff.deleted.len());
    for p in &diff.deleted {
        println!("    - {}", colorizer.red(p));
    }
    println!("  unchanged: {}", diff.unchanged.len());
    println!();
    if !locked_files.is_empty() {
        println!(
            "  warning: {} file(s) locked (will be excluded):",
            locked_files.len()
        );
        for lf in locked_files {
            println!("    ! {}", lf.file_path);
        }
    }
}

fn validate_snapshot_id(id: &str) -> GResult<()> {
    if id.is_empty() || id.starts_with('.') {
        return Err(GError::InvalidSnapshotId(id.to_string()));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(GError::InvalidSnapshotId(id.to_string()));
    }
    Ok(())
}
