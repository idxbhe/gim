//! `gim restore`

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

pub fn run(colorizer: &Colorizer, alias: String, snapshot_id: String, full: bool, threads: Option<usize>, dry_run: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let games_db = GamesDb::open(&paths.games_db)?;
    let game = games_db.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    if !game.game_dir.exists() { return Err(GError::GameDirMissing(game.game_dir.clone())); }
    let snaps_db_path = paths.snaps_db(&alias);
    let snaps_db = SnapsDb::open(&snaps_db_path)?;
    snaps_db.get_snapshot(&snapshot_id)?.ok_or_else(|| GError::SnapshotNotFound(snapshot_id.clone(), alias.clone()))?;
    let _lock = locking::acquire_game_lock(&alias, &snaps_db_path)?;
    let target_map = snaps_db.files_for_snapshot(&snapshot_id)?;

    let current_map: HashMap<String, crate::db::FileMeta> = if full {
        HashMap::new()
    } else {
        let ignore_set = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;
        let walk_opts = WalkOptions { threads: threads.unwrap_or(0), full_hash: false, ..WalkOptions::default() };
        let (hashed, _) = walk_and_hash(&game.game_dir, &ignore_set, Some(&target_map), &walk_opts)?;
        hashed.into_iter().map(|f| (f.file_path, crate::db::FileMeta { hash: f.hash, file_size: f.file_size, modified_time: f.modified_time })).collect()
    };

    let mut to_copy: Vec<(String, Hash, i64, i64)> = Vec::new();
    let mut to_delete: Vec<String> = Vec::new();
    if full {
        for (p, m) in &target_map { to_copy.push((p.clone(), m.hash.clone(), m.file_size, m.modified_time)); }
    } else {
        for (p, m) in &target_map {
            match current_map.get(p) {
                None => to_copy.push((p.clone(), m.hash.clone(), m.file_size, m.modified_time)),
                Some(cm) if cm.hash != m.hash => to_copy.push((p.clone(), m.hash.clone(), m.file_size, m.modified_time)),
                _ => {}
            }
        }
    }
    for p in current_map.keys() { if !target_map.contains_key(p) { to_delete.push(p.clone()); } }

    if dry_run {
        println!("dry run: would restore {} to {}", colorizer.green(&alias), colorizer.bold(&snapshot_id));
        if full { println!("  (--full mode)"); }
        println!("\n  to copy ({}):", to_copy.len());
        for (p, _, s, _) in &to_copy { println!("    + {} ({})", colorizer.green(p), format_size(*s)); }
        println!("  to delete ({}):", to_delete.len());
        for p in &to_delete { println!("    - {}", colorizer.red(p)); }
        return Ok(());
    }

    let cas = Cas::new(paths.objects_dir(&alias));
    let game_dir = game.game_dir.clone();
    let copy_results = crate::parallel::global().install(|| copy_all(&cas, &game_dir, &to_copy));
    let mut errors = Vec::new();
    for r in copy_results { if let Err(e) = r { errors.push(e); } }
    for p in &to_delete {
        let abs = crate::path_utils::denormalize(&game_dir, p);
        if abs.exists() { if let Err(e) = fs::remove_file(&abs) { errors.push(format!("delete {p}: {e}")); } }
    }
    cleanup_empty_dirs(&game_dir, &to_delete);

    println!("restored {} to {}", colorizer.green(&alias), colorizer.bold(&snapshot_id));
    println!("  {} files copied, {} files deleted", to_copy.len(), to_delete.len());
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
