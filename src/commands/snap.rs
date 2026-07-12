use crate::config::{env_data_dir_override, Paths};
use crate::db::{diff_states, FileEntry, FileMeta, GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::ignore_mod;
use crate::locking;
use crate::output::{Colorizer, ProgressReporter};
use crate::output::format_size;
use crate::storage::Cas;
use crate::walker::{walk_and_hash, WalkOptions};
use std::collections::HashMap;

pub fn run(c: &Colorizer, alias: String, custom_id: Option<String>, msg: Option<String>, threads: Option<usize>, dry_run: bool, full_hash: bool, progress: &ProgressReporter) -> GResult<()> {
    // ── Show a spinner IMMEDIATELY so the user sees feedback the
    //    moment they press Enter. The DB/lock setup below takes a few
    //    hundred ms, and without this the terminal looks frozen.
    progress.phase_start("preparing", 0);

    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    let game = gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    if !game.game_dir.exists() { progress.phase_done(""); return Err(GError::GameDirMissing(game.game_dir.clone())); }
    if !game.game_dir.is_dir() { progress.phase_done(""); return Err(GError::GameDirNotDir(game.game_dir.clone())); }
    let sdb_path = paths.snaps_db(&alias);
    let mut sdb = SnapsDb::open(&sdb_path)?;
    let _lock = locking::acquire_game_lock(&alias, &sdb_path)?;
    sdb.ensure_main_branch()?;
    let cur = sdb.get_current_branch()?;
    let pid = cur.as_ref().map(|b| b.snapshot_id.clone());
    let cbn = cur.as_ref().map(|b| b.name.clone());

    let sid = match custom_id {
        Some(id) => { validate_id(&id)?; if sdb.get_snapshot(&id)?.is_some() { progress.phase_done(""); return Err(GError::SnapshotIdExists(id, alias.clone())); } id }
        None => match &pid {
            None => "original".to_string(),
            Some(_) => { let base = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string(); let mut cand = base.clone(); let mut s = 2u32; while sdb.get_snapshot(&cand)?.is_some() { cand = format!("{base}-{s}"); s += 1; } cand }
        },
    };

    let ig = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;
    let pf = match &pid { Some(p) => sdb.files_for_snapshot(p)?, None => HashMap::new() };

    // Preparing done — clear the spinner before walk phase starts.
    progress.phase_done("");

    let wo = WalkOptions { threads: threads.unwrap_or(0), full_hash, ..WalkOptions::default() };
    let ref_map = if full_hash { None } else { Some(&pf) };
    let (hashed, locked) = walk_and_hash(&game.game_dir, &ig, ref_map, &wo, progress)?;

    let mut cs = HashMap::with_capacity(hashed.len());
    for f in &hashed { cs.insert(f.file_path.clone(), FileMeta { hash: f.hash.clone(), file_size: f.file_size, modified_time: f.modified_time }); }
    let diff = diff_states(&pf, &cs);

    if dry_run { print_dry(&alias, &sid, &diff, &locked, c, full_hash); return Ok(()); }
    if diff.total_changes() == 0 { println!("no changes detected, snapshot skipped"); return Ok(()); }

    let cas = Cas::new(paths.objects_dir(&alias));
    cas.ensure()?;
    let mut written = Vec::new();
    let nm: Vec<&crate::walker::HashedFile> = hashed.iter().filter(|f| match pf.get(&f.file_path) { None => true, Some(pm) => pm.hash != f.hash }).collect();
    let to_store: Vec<&&crate::walker::HashedFile> = nm.iter().filter(|f| !cas.exists(&f.hash)).collect();
    progress.store_start(to_store.len());
    for f in &to_store {
        let abs = crate::path_utils::denormalize(&game.game_dir, &f.file_path);
        match cas.store_from(&abs, &f.hash) {
            Ok(()) => { written.push((f.hash.clone(), f.file_size)); progress.store_tick(); }
            Err(e) => {
                for (h, _) in &written { let _ = cas.delete(h.as_str()); }
                progress.phase_done("");
                return Err(e);
            }
        }
    }
    let store_count = written.len() as u64;
    progress.store_done(store_count);

    let txr: GResult<()> = {
        let tx = sdb.transaction()?;
        SnapsDb::insert_snap(&tx, &sid, pid.as_deref(), SnapsDb::now_ms(), msg.as_deref(), cs.len() as i64, diff.added_size())?;
        let all: Vec<FileEntry> = hashed.iter().map(|f| FileEntry { file_path: f.file_path.clone(), hash: f.hash.clone(), file_size: f.file_size, modified_time: f.modified_time }).collect();
        SnapsDb::insert_files(&tx, &sid, &all)?;
        SnapsDb::insert_deleted_files(&tx, &sid, &diff.deleted)?;
        match &cbn {
            Some(n) => { tx.execute("UPDATE branches SET snapshotId = ?1 WHERE name = ?2", rusqlite::params![sid, n])?; }
            None => { tx.execute("INSERT INTO branches (name, snapshotId) VALUES ('main', ?1)", rusqlite::params![sid])?; tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('current_branch', 'main')", [])?; }
        }
        tx.commit()?; Ok(())
    };
    if let Err(e) = txr { for (h, _) in &written { let _ = cas.delete(h.as_str()); } return Err(e); }

    let bl = cbn.as_deref().unwrap_or("main");
    println!("snapshotted {} as {}", c.green(&alias), c.bold(&sid));
    println!("  {} files tracked, {} new/modified, {} deleted, added {}  (branch: {})", cs.len(), diff.added.len() + diff.modified.len(), diff.deleted.len(), format_size(diff.added_size()), c.cyan(bl));
    if !locked.is_empty() { println!("\nwarning: {} file(s) could not be read:", locked.len()); for lf in &locked { println!("  {}", lf.file_path); } }
    Ok(())
}

fn print_dry(alias: &str, id: &str, diff: &crate::db::Diff, locked: &[crate::walker::LockedFile], c: &Colorizer, fh: bool) {
    println!("dry run: would snapshot {alias} as {}", c.bold(id));
    if fh { println!("  (full-hash mode)"); }
    println!("\n  added ({}):", diff.added.len());
    for f in &diff.added { println!("    + {} ({})", c.green(&f.file_path), format_size(f.file_size)); }
    println!("  modified ({}):", diff.modified.len());
    for f in &diff.modified { println!("    ~ {}", c.yellow(&f.file_path)); }
    println!("  deleted ({}):", diff.deleted.len());
    for p in &diff.deleted { println!("    - {}", c.red(p)); }
    println!("  unchanged: {}", diff.unchanged.len());
    if !locked.is_empty() { println!("\n  warning: {} locked", locked.len()); }
    let _ = (id, c);
}

fn validate_id(id: &str) -> GResult<()> {
    if id.is_empty() || id.starts_with('.') { return Err(GError::InvalidSnapshotId(id.to_string())); }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') { return Err(GError::InvalidSnapshotId(id.to_string())); }
    Ok(())
}
