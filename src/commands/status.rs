use crate::config::{env_data_dir_override, GimConfig, Paths};
use crate::db::{diff_states, GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::ignore_mod;
use crate::output::{Colorizer, ProgressReporter};
use crate::walker::{walk_and_hash, WalkOptions};
use serde::Serialize;

#[derive(Serialize)] struct Sj { alias: String, branch: Option<String>, last_snapshot: Option<String>, modified: Vec<String>, added: Vec<String>, deleted: Vec<String> }

pub fn run(c: &Colorizer, alias: String, threads: Option<usize>, json: bool, full_hash: bool, progress: &ProgressReporter) -> GResult<()> {
    // Show a spinner IMMEDIATELY so the user sees feedback on Enter.
    if !json {
        progress.phase_start("preparing", 0);
    }

    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    let game = gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let sdb = SnapsDb::open(&paths.snaps_db(&alias))?;
    sdb.ensure_main_branch()?;
    let cur = sdb.get_current_branch()?.ok_or_else(|| GError::NoSnapshots(alias.clone()))?;
    let pf = sdb.files_for_snapshot(&cur.snapshot_id)?;
    let ig = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;

    if !json {
        progress.phase_cancel();
    }

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
    let (hashed, _) = walk_and_hash(&game.game_dir, &ig, ref_map, &wo, progress)?;
    let mut cm = std::collections::HashMap::with_capacity(hashed.len());
    for f in hashed { cm.insert(f.file_path, crate::db::FileMeta { hash: f.hash, file_size: f.file_size, modified_time: f.modified_time }); }
    let diff = diff_states(&pf, &cm);

    if json {
        let out = Sj { alias: alias.clone(), branch: Some(cur.name.clone()), last_snapshot: Some(cur.snapshot_id.clone()), modified: diff.modified.iter().map(|f| f.file_path.clone()).collect(), added: diff.added.iter().map(|f| f.file_path.clone()).collect(), deleted: diff.deleted.clone() };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("On branch {}", c.green(&cur.name));
    println!("last snapshot: {}", c.dim(&cur.snapshot_id));
    if full_hash { println!("  (full-hash mode)"); }
    println!();
    if diff.total_changes() == 0 { println!("nothing to snapshot, working tree clean"); return Ok(()); }
    println!("Changes:");
    println!();
    for f in &diff.modified { println!("        {}: {}", c.status_label("modified"), f.file_path); }
    for f in &diff.added { println!("        {}: {}", c.status_label("added"), f.file_path); }
    for p in &diff.deleted { println!("        {}: {}", c.status_label("deleted"), p); }
    println!("\n  {} file(s) changed", diff.total_changes());
    Ok(())
}
