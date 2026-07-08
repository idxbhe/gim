//! `gim snap`

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

pub fn run(colorizer: &Colorizer, alias: String, custom_id: Option<String>, msg: Option<String>, threads: Option<usize>, dry_run: bool, full_hash: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let games_db = GamesDb::open(&paths.games_db)?;
    let game = games_db.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    if !game.game_dir.exists() { return Err(GError::GameDirMissing(game.game_dir.clone())); }
    if !game.game_dir.is_dir() { return Err(GError::GameDirNotDir(game.game_dir.clone())); }
    let snaps_db_path = paths.snaps_db(&alias);
    let mut snaps_db = SnapsDb::open(&snaps_db_path)?;
    let _lock = locking::acquire_game_lock(&alias, &snaps_db_path)?;
    snaps_db.ensure_main_branch()?;
    let current_branch = snaps_db.get_current_branch()?;
    let parent_id = current_branch.as_ref().map(|b| b.snapshot_id.clone());
    let current_branch_name = current_branch.as_ref().map(|b| b.name.clone());

    let snapshot_id = match custom_id {
        Some(id) => { validate_id(&id)?; if snaps_db.get_snapshot(&id)?.is_some() { return Err(GError::SnapshotIdExists(id, alias.clone())); } id }
        None => match &parent_id {
            None => "original".to_string(),
            Some(_) => {
                let base = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
                let mut cand = base.clone();
                let mut suf = 2u32;
                while snaps_db.get_snapshot(&cand)?.is_some() { cand = format!("{base}-{suf}"); suf += 1; }
                cand
            }
        },
    };

    let ignore_set = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;
    let parent_files = match &parent_id { Some(pid) => snaps_db.files_for_snapshot(pid)?, None => HashMap::new() };
    let walk_opts = WalkOptions { threads: threads.unwrap_or(0), full_hash, ..WalkOptions::default() };
    let reference = if full_hash { None } else { Some(&parent_files) };
    let (hashed_files, locked_files) = walk_and_hash(&game.game_dir, &ignore_set, reference, &walk_opts)?;

    let mut current_state = HashMap::with_capacity(hashed_files.len());
    for f in &hashed_files {
        current_state.insert(f.file_path.clone(), FileMeta { hash: f.hash.clone(), file_size: f.file_size, modified_time: f.modified_time });
    }
    let diff = diff_states(&parent_files, &current_state);

    if dry_run { print_dry(&alias, &snapshot_id, &diff, &locked_files, colorizer, full_hash); return Ok(()); }
    if diff.total_changes() == 0 { println!("no changes detected, snapshot skipped"); return Ok(()); }

    let cas = Cas::new(paths.objects_dir(&alias));
    cas.ensure()?;
    let mut written = Vec::new();
    let new_or_mod: Vec<&crate::walker::HashedFile> = hashed_files.iter().filter(|f| match parent_files.get(&f.file_path) { None => true, Some(pm) => pm.hash != f.hash }).collect();
    for f in &new_or_mod {
        if !cas.exists(&f.hash) {
            let abs = crate::path_utils::denormalize(&game.game_dir, &f.file_path);
            match cas.store_from(&abs, &f.hash) {
                Ok(()) => written.push((f.hash.clone(), f.file_size)),
                Err(e) => { for (h, _) in &written { let _ = cas.delete(h.as_str()); } return Err(e); }
            }
        }
    }

    let tx_result: GResult<()> = {
        let tx = snaps_db.transaction()?;
        SnapsDb::insert_snap(&tx, &snapshot_id, parent_id.as_deref(), SnapsDb::now_ms(), msg.as_deref(), current_state.len() as i64, diff.added_size())?;
        let all: Vec<FileEntry> = hashed_files.iter().map(|f| FileEntry { file_path: f.file_path.clone(), hash: f.hash.clone(), file_size: f.file_size, modified_time: f.modified_time }).collect();
        SnapsDb::insert_files(&tx, &snapshot_id, &all)?;
        SnapsDb::insert_deleted_files(&tx, &snapshot_id, &diff.deleted)?;
        match &current_branch_name {
            Some(name) => { tx.execute("UPDATE branches SET snapshotId = ?1 WHERE name = ?2", rusqlite::params![snapshot_id, name])?; }
            None => {
                tx.execute("INSERT INTO branches (name, snapshotId) VALUES ('main', ?1)", rusqlite::params![snapshot_id])?;
                tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('current_branch', 'main')", [])?;
            }
        }
        tx.commit()?;
        Ok(())
    };
    if let Err(e) = tx_result { for (h, _) in &written { let _ = cas.delete(h.as_str()); } return Err(e); }

    let branch_label = current_branch_name.as_deref().unwrap_or("main");
    println!("snapshotted {} as {}", colorizer.green(&alias), colorizer.bold(&snapshot_id));
    println!("  {} files tracked, {} new/modified, {} deleted, added {}  (branch: {})", current_state.len(), diff.added.len() + diff.modified.len(), diff.deleted.len(), format_size(diff.added_size()), colorizer.cyan(branch_label));
    if !locked_files.is_empty() {
        println!("\nwarning: {} file(s) could not be read:", locked_files.len());
        for lf in &locked_files { println!("  {}", lf.file_path); }
    }
    Ok(())
}

fn print_dry(alias: &str, id: &str, diff: &crate::db::Diff, locked: &[crate::walker::LockedFile], c: &Colorizer, full_hash: bool) {
    println!("dry run: would snapshot {alias} as {}", c.bold(id));
    if full_hash { println!("  (full-hash mode)"); }
    println!("\n  added ({}):", diff.added.len());
    for f in &diff.added { println!("    + {} ({})", c.green(&f.file_path), format_size(f.file_size)); }
    println!("  modified ({}):", diff.modified.len());
    for f in &diff.modified { println!("    ~ {}", c.yellow(&f.file_path)); }
    println!("  deleted ({}):", diff.deleted.len());
    for p in &diff.deleted { println!("    - {}", c.red(p)); }
    println!("  unchanged: {}", diff.unchanged.len());
    if !locked.is_empty() { println!("\n  warning: {} locked", locked.len()); }
}

fn validate_id(id: &str) -> GResult<()> {
    if id.is_empty() || id.starts_with('.') { return Err(GError::InvalidSnapshotId(id.to_string())); }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') { return Err(GError::InvalidSnapshotId(id.to_string())); }
    Ok(())
}

// Suppress unused warning for Hash import (used in type annotations in other commands)
#[allow(unused_imports)]
use crate::hashing::Hash as _Hash;
