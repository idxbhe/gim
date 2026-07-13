use crate::config::{env_data_dir_override, GimConfig, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::hashing::Hash;
use crate::ignore_mod;
use crate::locking;
use crate::output::{Colorizer, ProgressReporter};
use crate::output::format_size;
use crate::storage::Cas;
use crate::walker::{walk_and_hash, walk_only, WalkOptions};
use filetime::FileTime;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

pub fn run(c: &Colorizer, alias: String, sid: String, full: bool, threads: Option<usize>, dry_run: bool, progress: &ProgressReporter) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    let game = gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    if !game.game_dir.exists() { return Err(GError::GameDirMissing(game.game_dir.clone())); }
    let sdb_path = paths.snaps_db(&alias);
    let sdb = SnapsDb::open(&sdb_path)?;
    sdb.get_snapshot(&sid)?.ok_or_else(|| GError::SnapshotNotFound(sid.clone(), alias.clone()))?;
    let _lock = locking::acquire_game_lock(&alias, &sdb_path)?;
    let tm = sdb.files_for_snapshot(&sid)?;

    let cm: HashMap<String, crate::db::FileMeta> = if full {
        HashMap::new()
    } else {
        let ig = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;
        let cfg = GimConfig::load_game(&paths, &alias)?;
        let algorithm = cfg.hash_algorithm()?;
        let cfg_threads = cfg.hash_threads();
        let wo = WalkOptions { threads: threads.unwrap_or(cfg_threads), full_hash: false, algorithm, parallel: cfg.hash_parallel(), ..WalkOptions::default() };
        let (hashed, _) = walk_and_hash(&game.game_dir, &ig, Some(&tm), &wo, progress)?;
        hashed.into_iter().map(|f| (f.file_path, crate::db::FileMeta { hash: f.hash, file_size: f.file_size, modified_time: f.modified_time })).collect()
    };

    let mut tc: Vec<(String, Hash, i64, i64)> = Vec::new();
    let mut td: Vec<String> = Vec::new();
    if full { for (p, m) in &tm { tc.push((p.clone(), m.hash.clone(), m.file_size, m.modified_time)); } }
    else { for (p, m) in &tm { match cm.get(p) { None => tc.push((p.clone(), m.hash.clone(), m.file_size, m.modified_time)), Some(c2) if c2.hash != m.hash => tc.push((p.clone(), m.hash.clone(), m.file_size, m.modified_time)), _ => {} } } }
    for p in cm.keys() { if !tm.contains_key(p) { td.push(p.clone()); } }

    if dry_run {
        println!("dry run: would restore {} to {}", c.green(&alias), c.bold(&sid));
        if full { println!("  (--full mode)"); }
        println!("\n  to copy ({}):", tc.len());
        for (p, _, s, _) in &tc { println!("    + {} ({})", c.green(p), format_size(*s)); }
        println!("  to delete ({}):", td.len());
        for p in &td { println!("    - {}", c.red(p)); }
        return Ok(());
    }

    let cas = Cas::new(paths.objects_dir(&alias));
    let gd = game.game_dir.clone();
    if full {
        // For --full, also walk on-disk files to know what to delete.
        let ig = ignore_mod::build_for_game(&paths, &alias, &gd)?;
        let on_disk = walk_only(&gd, &ig, progress)?;
        td = on_disk.into_iter().filter(|p| !tm.contains_key(p)).collect();
    }

    progress.copy_start(tc.len());
    let results = crate::parallel::global().install(|| copy_all(&cas, &gd, &tc, progress));
    let mut errors = Vec::new();
    for r in results { if let Err(e) = r { errors.push(e); } }
    let copy_count = tc.len() as u64;
    progress.copy_done(copy_count);

    for p in &td { let abs = crate::path_utils::denormalize(&gd, p); if abs.exists() { if let Err(e) = fs::remove_file(&abs) { errors.push(format!("delete {p}: {e}")); } } }
    cleanup(&gd, &td);

    println!("restored {} to {}", c.green(&alias), c.bold(&sid));
    println!("  {} files copied, {} files deleted", tc.len(), td.len());
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
        loop {
            let n = match src.read(&mut buf) { Ok(0) => break, Ok(n) => n, Err(e) => return Err(format!("read {hash}: {e}")) };
            if let Err(e) = dst.write_all(&buf[..n]) { return Err(format!("write {path}: {e}")); }
        }
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
