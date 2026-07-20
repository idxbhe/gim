use crate::config::{env_data_dir_override, GimConfig, Paths};
use crate::db::{diff_states, FileEntry, FileMeta, GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::ignore_mod;
use crate::locking;
use crate::output::{Colorizer, ProgressReporter};
use crate::output::format_size;
use crate::storage::Cas;
use crate::walker::{walk_and_hash, SnapFilter, WalkOptions};
use std::collections::HashMap;

pub fn run(
    c: &Colorizer,
    alias: String,
    custom_id: Option<String>,
    msg: Option<String>,
    threads: Option<usize>,
    dry_run: bool,
    full_hash: bool,
    exclude_patterns: Vec<String>,
    include_only_patterns: Vec<String>,
    progress: &ProgressReporter,
) -> GResult<()> {
    progress.phase_start("preparing", 0);

    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    let game = gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    if !game.game_dir.exists() { progress.phase_cancel(); return Err(GError::GameDirMissing(game.game_dir.clone())); }
    if !game.game_dir.is_dir() { progress.phase_cancel(); return Err(GError::GameDirNotDir(game.game_dir.clone())); }
    let sdb_path = paths.snaps_db(&alias);
    let mut sdb = SnapsDb::open(&sdb_path)?;
    let _lock = locking::acquire_game_lock(&alias, &sdb_path)?;
    sdb.ensure_main_branch()?;
    let cur = sdb.get_current_branch()?;
    let pid = cur.as_ref().map(|b| b.snapshot_id.clone());
    let cbn = cur.as_ref().map(|b| b.name.clone());

    let sid = match custom_id {
        Some(id) => { validate_id(&id)?; if sdb.get_snapshot(&id)?.is_some() { progress.phase_cancel(); return Err(GError::SnapshotIdExists(id, alias.clone())); } id }
        None => match &pid {
            None => "original".to_string(),
            Some(_) => { let base = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string(); let mut cand = base.clone(); let mut s = 2u32; while sdb.get_snapshot(&cand)?.is_some() { cand = format!("{base}-{s}"); s += 1; } cand }
        },
    };

    let ig = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;
    let pf = match &pid { Some(p) => sdb.files_for_snapshot(p)?, None => HashMap::new() };

    progress.phase_cancel();

    // Build per-snap filter from --exclude and --include-only patterns.
    let snap_filter = SnapFilter::new(&exclude_patterns, &include_only_patterns)?;
    let has_filter = !snap_filter.is_empty();

    let cfg = GimConfig::load_game(&paths, &alias)?;
    let algorithm = cfg.hash_algorithm()?;
    let cfg_threads = cfg.hash_threads();

    let wo = WalkOptions {
        threads: threads.unwrap_or(cfg_threads),
        full_hash,
        algorithm,
        parallel: cfg.hash_parallel(),
        ..WalkOptions::default()
    };
    let ref_map = if full_hash { None } else { Some(&pf) };
    // Walk only files that pass the snap filter. Files filtered out
    // will be inherited from the parent snapshot as "unchanged".
    let (hashed, locked) = walk_and_hash(&game.game_dir, &ig, ref_map, &wo, progress, if has_filter { Some(&snap_filter) } else { None })?;

    // Build current_state from walked files only.
    let mut cs = HashMap::with_capacity(hashed.len());
    for f in &hashed {
        cs.insert(f.file_path.clone(), FileMeta {
            hash: f.hash.clone(), file_size: f.file_size, modified_time: f.modified_time,
        });
    }

    // ── Inheritance logic ───────────────────────────────────────────
    // If a snap filter is active, files that were filtered out (not
    // walked) must be inherited from the parent snapshot as "unchanged".
    // Without this, the diff would mark them as "deleted" — which is
    // wrong. The user didn't delete them; they just excluded them from
    // this snap's walk.
    //
    // For each file in the parent snapshot:
    // - If it was walked (in cs) → use the walked hash (normal diff)
    // - If it was NOT walked AND was filtered out by snap_filter →
    //   inherit from parent as unchanged
    // - If it was NOT walked AND was NOT filtered out → it was truly
    //   deleted from disk → mark as deleted
    let diff = if has_filter {
        // Build the complete current state: walked files + inherited files.
        let mut full_cs = HashMap::with_capacity(cs.len() + pf.len());
        // Add walked files.
        for (path, meta) in &cs {
            full_cs.insert(path.clone(), meta.clone());
        }
        // Inherit non-walked parent files that were filtered out.
        for (path, meta) in &pf {
            if !full_cs.contains_key(path) {
                // This parent file was not walked. Was it filtered out
                // by the snap filter, or was it truly deleted?
                if !snap_filter.should_walk(path) {
                    // Filtered out — inherit from parent as unchanged.
                    full_cs.insert(path.clone(), meta.clone());
                }
                // If should_walk(path) is true but the file wasn't walked,
                // it means the file doesn't exist on disk → truly deleted.
                // Don't add it to full_cs → diff will detect it as deleted.
            }
        }
        diff_states(&pf, &full_cs)
    } else {
        // No filter — normal diff.
        diff_states(&pf, &cs)
    };

    if dry_run { print_dry(&alias, &sid, &diff, &locked, c, full_hash, has_filter); return Ok(()); }
    if diff.total_changes() == 0 {
        println!("no changes detected, snapshot skipped");
        return Ok(());
    }

    let cas = Cas::new(paths.objects_dir(&alias));
    cas.ensure()?;
    let mut written = Vec::new();

    // Only store NEW or MODIFIED objects to CAS.
    // Inherited files don't need storage — their objects already exist.
    let to_store: Vec<&crate::walker::HashedFile> = hashed
        .iter()
        .filter(|f| match pf.get(&f.file_path) {
            None => true,
            Some(pm) => pm.hash != f.hash,
        })
        .filter(|f| !cas.exists(&f.hash))
        .collect();
    progress.store_start(to_store.len());
    for f in &to_store {
        let abs = crate::path_utils::denormalize(&game.game_dir, &f.file_path);
        match cas.store_from(&abs, &f.hash) {
            Ok(()) => { written.push((f.hash.clone(), f.file_size)); progress.store_tick(); }
            Err(e) => {
                for (h, _) in &written { let _ = cas.delete(h.as_str()); }
                progress.phase_cancel();
                return Err(e);
            }
        }
    }
    let store_count = written.len() as u64;
    progress.store_done(store_count);

    // ── Build the complete file list for the snapshot ──────────────
    // The snapshot must contain ALL files: walked + inherited.
    // This ensures `gim restore` can restore the complete game state.
    let txr: GResult<()> = {
        let tx = sdb.transaction()?;
        // Build the full file list.
        let mut all_files: Vec<FileEntry> = if has_filter {
            // Walked files + inherited parent files.
            let mut out: Vec<FileEntry> = hashed.iter().map(|f| FileEntry {
                file_path: f.file_path.clone(), hash: f.hash.clone(),
                file_size: f.file_size, modified_time: f.modified_time,
            }).collect();
            // Add inherited files (parent files not walked, filtered out).
            for (path, meta) in &pf {
                if !cs.contains_key(path) && !snap_filter.should_walk(path) {
                    out.push(FileEntry {
                        file_path: path.clone(), hash: meta.hash.clone(),
                        file_size: meta.file_size, modified_time: meta.modified_time,
                    });
                }
            }
            out
        } else {
            hashed.iter().map(|f| FileEntry {
                file_path: f.file_path.clone(), hash: f.hash.clone(),
                file_size: f.file_size, modified_time: f.modified_time,
            }).collect()
        };

        // Sort for deterministic storage order.
        all_files.sort_by(|a, b| a.file_path.cmp(&b.file_path));

        SnapsDb::insert_snap(&tx, &sid, pid.as_deref(), SnapsDb::now_ms(), msg.as_deref(), all_files.len() as i64, diff.added_size())?;
        SnapsDb::insert_files(&tx, &sid, &all_files)?;
        SnapsDb::insert_deleted_files(&tx, &sid, &diff.deleted)?;
        match &cbn {
            Some(n) => { tx.execute("UPDATE branches SET snapshotId = ?1 WHERE name = ?2", rusqlite::params![sid, n])?; }
            None => { tx.execute("INSERT INTO branches (name, snapshotId) VALUES ('main', ?1)", rusqlite::params![sid])?; tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('current_branch', 'main')", [])?; }
        }
        tx.commit()?; Ok(())
    };
    if let Err(e) = txr { for (h, _) in &written { let _ = cas.delete(h.as_str()); } return Err(e); }

    let bl = cbn.as_deref().unwrap_or("main");
    let total_files = if has_filter {
        // Count walked + inherited.
        cs.len() + pf.iter().filter(|(p, _)| !cs.contains_key(*p) && !snap_filter.should_walk(p)).count()
    } else {
        cs.len()
    };
    println!("snapshotted {} as {}", c.green(&alias), c.bold(&sid));
    println!("  {} files tracked, {} new/modified, {} deleted, added {}  (branch: {})", total_files, diff.added.len() + diff.modified.len(), diff.deleted.len(), format_size(diff.added_size()), c.cyan(bl));
    if has_filter {
        let inherited = total_files - cs.len();
        println!("  {} files inherited (filtered from this snap)", c.dim(&inherited.to_string()));
    }
    if !locked.is_empty() { println!("\nwarning: {} file(s) could not be read:", locked.len()); for lf in &locked { println!("  {}", lf.file_path); } }
    Ok(())
}

fn print_dry(alias: &str, id: &str, diff: &crate::db::Diff, locked: &[crate::walker::LockedFile], c: &Colorizer, fh: bool, has_filter: bool) {
    println!("dry run: would snapshot {alias} as {}", c.bold(id));
    if fh { println!("  (full-hash mode)"); }
    if has_filter { println!("  (per-snap filter active)"); }
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
    // Snapshot IDs are pure DB labels (never used as filesystem paths — CAS
    // keys are content hashes, not IDs), so Windows reserved names like CON/
    // PRN are irrelevant. We still bound length and reject path-like values
    // (`..`, `/`, `\`) to keep IDs clean and prevent any future misuse.
    if id.len() > 128 { return Err(GError::InvalidSnapshotId(id.to_string())); }
    if id.contains("..") || id.contains('/') || id.contains('\\') { return Err(GError::InvalidSnapshotId(id.to_string())); }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') { return Err(GError::InvalidSnapshotId(id.to_string())); }
    Ok(())
}
