use crate::config::{env_data_dir_override, GimConfig, Paths};
use crate::db::{diff_states, GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::hashing::Hash;
use crate::ignore_mod;
use crate::locking;
use crate::output::{Colorizer, ProgressReporter};
use crate::storage::Cas;
use crate::walker::{walk_and_hash, walk_only, WalkOptions};
use filetime::FileTime;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

#[derive(Serialize)] struct Bj { name: String, snapshot_id: String, created_at: i64, is_current: bool }
#[derive(Serialize)] struct Blj { alias: String, current: Option<String>, branches: Vec<Bj> }

pub fn run(c: &Colorizer, alias: String, create: Option<String>, delete: Option<String>, switch: Option<String>, from: Option<String>, force: bool, json: bool, progress: &ProgressReporter) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    let game = gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let sdb_path = paths.snaps_db(&alias);
    let mut sdb = SnapsDb::open(&sdb_path)?;
    sdb.ensure_main_branch()?;
    if let Some(n) = create { return create_b(&mut sdb, c, &alias, &n, from); }
    if let Some(n) = delete { return delete_b(&sdb, c, &alias, &n); }
    if let Some(n) = switch { return switch_b(&mut sdb, &paths, c, &alias, &game.game_dir, &n, force, progress); }
    list_b(&sdb, c, &alias, json)
}

fn list_b(sdb: &SnapsDb, c: &Colorizer, alias: &str, json: bool) -> GResult<()> {
    let br = sdb.list_branches()?;
    let cur = sdb.get_current_branch()?.map(|b| b.name);
    if json {
        let out = Blj { alias: alias.to_string(), current: cur.clone(), branches: br.iter().map(|b| Bj { name: b.name.clone(), snapshot_id: b.snapshot_id.clone(), created_at: b.created_at, is_current: cur.as_deref() == Some(b.name.as_str()) }).collect() };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if br.is_empty() { println!("{} has no branches", c.bold(alias)); return Ok(()); }
    println!("{} branches:\n", c.bold(alias));
    let ma = br.iter().map(|b| b.name.len()).max().unwrap_or(0);
    for b in &br {
        let m = if cur.as_deref() == Some(b.name.as_str()) { c.green("*") } else { " ".to_string() };
        let n = if cur.as_deref() == Some(b.name.as_str()) { c.green(&format!("{:<ma$}", b.name)) } else { format!("{:<ma$}", b.name) };
        println!("  {m} {n}  →  {}", c.bold(&b.snapshot_id));
    }
    Ok(())
}

fn create_b(sdb: &mut SnapsDb, c: &Colorizer, alias: &str, name: &str, from: Option<String>) -> GResult<()> {
    if name.is_empty() || name.starts_with('.') { return Err(GError::Other(format!("invalid branch \"{name}\""))); }
    if !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.') { return Err(GError::Other(format!("invalid branch \"{name}\""))); }
    if sdb.get_branch(name)?.is_some() { return Err(GError::BranchExists(name.to_string(), alias.to_string())); }
    let sid = match from { Some(s) => { sdb.get_snapshot(&s)?.ok_or_else(|| GError::SnapshotNotFound(s.clone(), alias.to_string()))?; s } None => sdb.get_current_branch()?.ok_or_else(|| GError::NoSnapshots(alias.to_string()))?.snapshot_id };
    sdb.insert_branch(name, &sid)?;
    println!("created branch {} → {}", c.green(name), c.bold(&sid));
    Ok(())
}

fn delete_b(sdb: &SnapsDb, c: &Colorizer, alias: &str, name: &str) -> GResult<()> {
    if name == "main" { return Err(GError::CannotDeleteMainBranch); }
    sdb.get_branch(name)?.ok_or_else(|| GError::BranchNotFound(name.to_string(), alias.to_string()))?;
    let cur = sdb.get_current_branch()?;
    if cur.as_ref().map(|b| b.name.as_str()) == Some(name) { return Err(GError::CannotDeleteCurrentBranch(name.to_string())); }
    sdb.delete_branch(name)?;
    println!("deleted branch {}", c.green(name));
    Ok(())
}

fn switch_b(sdb: &mut SnapsDb, paths: &Paths, c: &Colorizer, alias: &str, gd: &Path, name: &str, force: bool, progress: &ProgressReporter) -> GResult<()> {
    let target = sdb.get_branch(name)?.ok_or_else(|| GError::BranchNotFound(name.to_string(), alias.to_string()))?;
    let cur = sdb.get_current_branch()?;
    if cur.as_ref().map(|b| b.name.as_str()) == Some(name) { println!("already on branch {}", c.green(name)); return Ok(()); }
    let _lock = locking::acquire_game_lock(alias, &paths.snaps_db(alias))?;
    if !force {
        if let Some(cur) = &cur {
            let pf = sdb.files_for_snapshot(&cur.snapshot_id)?;
            let ig = ignore_mod::build_for_game(paths, alias, gd)?;
            let cfg = GimConfig::load_game(paths, alias)?;
            let algorithm = cfg.hash_algorithm()?;
            let wo = WalkOptions { algorithm, parallel: cfg.hash_parallel(), ..WalkOptions::default() };
            let (hashed, _) = walk_and_hash(gd, &ig, Some(&pf), &wo, progress, None)?;
            let mut cm = HashMap::with_capacity(hashed.len());
            for f in hashed { cm.insert(f.file_path, crate::db::FileMeta { hash: f.hash, file_size: f.file_size, modified_time: f.modified_time }); }
            if diff_states(&pf, &cm).total_changes() > 0 { return Err(GError::UncommittedChanges); }
        }
    }
    let tm = sdb.files_for_snapshot(&target.snapshot_id)?;
    let cas = Cas::new(paths.objects_dir(alias));
    cas.ensure()?;
    let ig = ignore_mod::build_for_game(paths, alias, gd)?;
    progress.walk_start();
    let on_disk = walk_only(gd, &ig, progress)?;
    let walk_count = on_disk.len() as u64;
    progress.walk_done(walk_count);
    let tc: Vec<(String, Hash, i64, i64)> = tm.iter().map(|(p, m)| (p.clone(), m.hash.clone(), m.file_size, m.modified_time)).collect();
    let td: Vec<String> = on_disk.iter().filter(|p| !tm.contains_key(*p)).cloned().collect();
    progress.copy_start(tc.len());
    let results = crate::parallel::global().install(|| copy_all(&cas, gd, &tc, progress));
    let mut errors = Vec::new();
    for r in results { if let Err(e) = r { errors.push(e); } }
    let copy_count = tc.len() as u64;
    progress.copy_done(copy_count);
    for p in &td { let abs = crate::path_utils::denormalize(gd, p); if abs.exists() { if let Err(e) = fs::remove_file(&abs) { errors.push(format!("delete {p}: {e}")); } } }
    cleanup(gd, &td);
    sdb.set_current_branch(name)?;
    println!("switched to branch {}", c.green(name));
    println!("  restored {} files, deleted {} files (snapshot: {})", tc.len(), td.len(), c.bold(&target.snapshot_id));
    if !errors.is_empty() { eprintln!("warning: {} error(s):", errors.len()); for e in &errors { eprintln!("  {e}"); } }
    Ok(())
}

fn copy_all(cas: &Cas, gd: &Path, tc: &[(String, Hash, i64, i64)], progress: &ProgressReporter) -> Vec<Result<(), String>> {
    tc.par_iter().map(|(path, hash, _, mtime)| {
        let abs = crate::path_utils::denormalize(gd, path);
        if let Some(p) = abs.parent() { if let Err(e) = fs::create_dir_all(p) { return Err(format!("mkdir {path}: {e}")); } }
        let mut src = match cas.open(hash) { Ok(f) => f, Err(e) => return Err(format!("open {hash}: {e}")) };
        let mut dst = match fs::File::create(&abs) { Ok(f) => f, Err(e) => return Err(format!("create {path}: {e}")) };
        let mut buf = vec![0u8; 1024 * 1024];
        loop { let n = match src.read(&mut buf) { Ok(0) => break, Ok(n) => n, Err(e) => return Err(format!("read {hash}: {e}")) }; if let Err(e) = dst.write_all(&buf[..n]) { return Err(format!("write {path}: {e}")); } }
        let _ = dst.sync_all(); drop(dst);
        if *mtime > 0 { let _ = filetime::set_file_mtime(&abs, FileTime::from_unix_time(*mtime, 0)); }
        progress.copy_tick();
        Ok(())
    }).collect()
}

fn cleanup(gd: &Path, deleted: &[String]) {
    let mut checked: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    for p in deleted {
        let abs = crate::path_utils::denormalize(gd, p);
        let mut cur = abs.parent().map(|x| x.to_path_buf());
        while let Some(dir) = cur {
            if dir == gd || checked.contains(&dir) { break; }
            checked.insert(dir.clone());
            if fs::remove_dir(&dir).is_err() { break; }
            cur = dir.parent().map(|x| x.to_path_buf());
        }
    }
}
