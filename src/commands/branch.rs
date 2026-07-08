//! `gim branch` — manage branches.

use crate::config::{env_data_dir_override, Paths};
use crate::db::{diff_states, GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::hashing::Hash;
use crate::ignore_mod;
use crate::locking;
use crate::output::Colorizer;
use crate::storage::Cas;
use crate::walker::{walk_and_hash, WalkOptions};
use filetime::FileTime;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

#[derive(Serialize)]
struct BranchJson { name: String, snapshot_id: String, created_at: i64, is_current: bool }
#[derive(Serialize)]
struct BranchListJson { alias: String, current: Option<String>, branches: Vec<BranchJson> }

pub fn run(colorizer: &Colorizer, alias: String, create: Option<String>, delete: Option<String>, switch: Option<String>, from: Option<String>, force: bool, json: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let games_db = GamesDb::open(&paths.games_db)?;
    let game = games_db.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let snaps_db_path = paths.snaps_db(&alias);
    let mut snaps_db = SnapsDb::open(&snaps_db_path)?;
    snaps_db.ensure_main_branch()?;

    if let Some(n) = create { return create_branch(&mut snaps_db, colorizer, &alias, &n, from); }
    if let Some(n) = delete { return delete_branch(&snaps_db, colorizer, &alias, &n); }
    if let Some(n) = switch { return switch_branch(&mut snaps_db, &paths, colorizer, &alias, &game.game_dir, &n, force); }
    list_branches(&snaps_db, colorizer, &alias, json)
}

fn list_branches(snaps_db: &SnapsDb, c: &Colorizer, alias: &str, json: bool) -> GResult<()> {
    let branches = snaps_db.list_branches()?;
    let cur = snaps_db.get_current_branch()?.map(|b| b.name);
    if json {
        let out = BranchListJson { alias: alias.to_string(), current: cur.clone(), branches: branches.iter().map(|b| BranchJson { name: b.name.clone(), snapshot_id: b.snapshot_id.clone(), created_at: b.created_at, is_current: cur.as_deref() == Some(b.name.as_str()) }).collect() };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if branches.is_empty() { println!("{} has no branches", c.bold(alias)); return Ok(()); }
    println!("{} branches:\n", c.bold(alias));
    let ma = branches.iter().map(|b| b.name.len()).max().unwrap_or(0);
    for b in &branches {
        let m = if cur.as_deref() == Some(b.name.as_str()) { c.green("*") } else { " ".to_string() };
        let n = if cur.as_deref() == Some(b.name.as_str()) { c.green(&format!("{:<ma$}", b.name)) } else { format!("{:<ma$}", b.name) };
        println!("  {m} {n}  →  {}", c.bold(&b.snapshot_id));
    }
    Ok(())
}

fn create_branch(snaps_db: &mut SnapsDb, c: &Colorizer, alias: &str, name: &str, from: Option<String>) -> GResult<()> {
    if name.is_empty() || name.starts_with('.') { return Err(GError::Other(format!("invalid branch name \"{name}\""))); }
    if !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.') { return Err(GError::Other(format!("invalid branch name \"{name}\""))); }
    if snaps_db.get_branch(name)?.is_some() { return Err(GError::BranchExists(name.to_string(), alias.to_string())); }
    let sid = match from {
        Some(s) => { snaps_db.get_snapshot(&s)?.ok_or_else(|| GError::SnapshotNotFound(s.clone(), alias.to_string()))?; s }
        None => snaps_db.get_current_branch()?.ok_or_else(|| GError::NoSnapshots(alias.to_string()))?.snapshot_id,
    };
    snaps_db.insert_branch(name, &sid)?;
    println!("created branch {} → {}", c.green(name), c.bold(&sid));
    Ok(())
}

fn delete_branch(snaps_db: &SnapsDb, c: &Colorizer, alias: &str, name: &str) -> GResult<()> {
    if name == "main" { return Err(GError::CannotDeleteMainBranch); }
    snaps_db.get_branch(name)?.ok_or_else(|| GError::BranchNotFound(name.to_string(), alias.to_string()))?;
    let cur = snaps_db.get_current_branch()?;
    if cur.as_ref().map(|b| b.name.as_str()) == Some(name) { return Err(GError::CannotDeleteCurrentBranch(name.to_string())); }
    snaps_db.delete_branch(name)?;
    println!("deleted branch {}", c.green(name));
    println!("  (snapshots retained — run `gim gc {alias}` to free space)");
    Ok(())
}

fn switch_branch(snaps_db: &mut SnapsDb, paths: &Paths, c: &Colorizer, alias: &str, game_dir: &Path, name: &str, force: bool) -> GResult<()> {
    let target = snaps_db.get_branch(name)?.ok_or_else(|| GError::BranchNotFound(name.to_string(), alias.to_string()))?;
    let cur = snaps_db.get_current_branch()?;
    if cur.as_ref().map(|b| b.name.as_str()) == Some(name) { println!("already on branch {}", c.green(name)); return Ok(()); }
    let _lock = locking::acquire_game_lock(alias, &paths.snaps_db(alias))?;

    if !force {
        if let Some(cur) = &cur {
            let parent_files = snaps_db.files_for_snapshot(&cur.snapshot_id)?;
            let ignore_set = ignore_mod::build_for_game(paths, alias, game_dir)?;
            let (hashed, _) = walk_and_hash(game_dir, &ignore_set, Some(&parent_files), &WalkOptions::default())?;
            let mut current_map = HashMap::with_capacity(hashed.len());
            for f in hashed { current_map.insert(f.file_path, crate::db::FileMeta { hash: f.hash, file_size: f.file_size, modified_time: f.modified_time }); }
            if diff_states(&parent_files, &current_map).total_changes() > 0 { return Err(GError::UncommittedChanges); }
        }
    }

    let target_map = snaps_db.files_for_snapshot(&target.snapshot_id)?;
    let cas = Cas::new(paths.objects_dir(alias));
    cas.ensure()?;
    let ignore_set = ignore_mod::build_for_game(paths, alias, game_dir)?;
    let on_disk = crate::walker::walk_only(game_dir, &ignore_set)?;
    let to_copy: Vec<(String, Hash, i64, i64)> = target_map.iter().map(|(p, m)| (p.clone(), m.hash.clone(), m.file_size, m.modified_time)).collect();
    let to_delete: Vec<String> = on_disk.into_iter().filter(|p| !target_map.contains_key(p)).collect();

    let results = crate::parallel::global().install(|| copy_all(&cas, game_dir, &to_copy));
    let mut errors = Vec::new();
    for r in results { if let Err(e) = r { errors.push(e); } }
    for p in &to_delete {
        let abs = crate::path_utils::denormalize(game_dir, p);
        if abs.exists() { if let Err(e) = fs::remove_file(&abs) { errors.push(format!("delete {p}: {e}")); } }
    }
    cleanup_empty_dirs(game_dir, &to_delete);
    snaps_db.set_current_branch(name)?;

    println!("switched to branch {}", c.green(name));
    println!("  restored {} files, deleted {} files (snapshot: {})", to_copy.len(), to_delete.len(), c.bold(&target.snapshot_id));
    if !errors.is_empty() { eprintln!("warning: {} error(s):", errors.len()); for e in &errors { eprintln!("  {e}"); } }
    Ok(())
}

fn copy_all(cas: &Cas, game_dir: &Path, to_copy: &[(String, Hash, i64, i64)]) -> Vec<Result<(), String>> {
    to_copy.par_iter().map(|(path, hash, _, mtime)| {
        let abs = crate::path_utils::denormalize(game_dir, path);
        if let Some(parent) = abs.parent() { if let Err(e) = fs::create_dir_all(parent) { return Err(format!("mkdir {path}: {e}")); } }
        let mut src = match cas.open(hash) { Ok(f) => f, Err(e) => return Err(format!("open {hash}: {e}")) };
        let mut dst = match fs::File::create(&abs) { Ok(f) => f, Err(e) => return Err(format!("create {path}: {e}")) };
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = match src.read(&mut buf) { Ok(0) => break, Ok(n) => n, Err(e) => return Err(format!("read {hash}: {e}")) };
            if let Err(e) = dst.write_all(&buf[..n]) { return Err(format!("write {path}: {e}")); }
        }
        let _ = dst.sync_all();
        drop(dst);
        if *mtime > 0 { let _ = filetime::set_file_mtime(&abs, FileTime::from_unix_time(*mtime, 0)); }
        Ok(())
    }).collect()
}

fn cleanup_empty_dirs(game_dir: &Path, deleted: &[String]) {
    let mut checked: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    for p in deleted {
        let abs = crate::path_utils::denormalize(game_dir, p);
        let mut cur = abs.parent().map(|x| x.to_path_buf());
        while let Some(dir) = cur {
            if dir == game_dir || checked.contains(&dir) { break; }
            checked.insert(dir.clone());
            if fs::remove_dir(&dir).is_err() { break; }
            cur = dir.parent().map(|x| x.to_path_buf());
        }
    }
}
