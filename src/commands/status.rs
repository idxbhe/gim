//! `gim status`

use crate::config::{env_data_dir_override, Paths};
use crate::db::{diff_states, GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::ignore_mod;
use crate::output::Colorizer;
use crate::walker::{walk_and_hash, WalkOptions};
use serde::Serialize;

#[derive(Serialize)]
struct StatusJson { alias: String, branch: Option<String>, last_snapshot: Option<String>, modified: Vec<String>, added: Vec<String>, deleted: Vec<String> }

pub fn run(colorizer: &Colorizer, alias: String, threads: Option<usize>, json: bool, full_hash: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let games_db = GamesDb::open(&paths.games_db)?;
    let game = games_db.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let snaps_db_path = paths.snaps_db(&alias);
    let snaps_db = SnapsDb::open(&snaps_db_path)?;
    snaps_db.ensure_main_branch()?;
    let cur = snaps_db.get_current_branch()?.ok_or_else(|| GError::NoSnapshots(alias.clone()))?;
    let parent_files = snaps_db.files_for_snapshot(&cur.snapshot_id)?;
    let ignore_set = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;
    let walk_opts = WalkOptions { threads: threads.unwrap_or(0), full_hash, ..WalkOptions::default() };
    let reference = if full_hash { None } else { Some(&parent_files) };
    let (hashed, _) = walk_and_hash(&game.game_dir, &ignore_set, reference, &walk_opts)?;
    let mut current_map = std::collections::HashMap::with_capacity(hashed.len());
    for f in hashed { current_map.insert(f.file_path, crate::db::FileMeta { hash: f.hash, file_size: f.file_size, modified_time: f.modified_time }); }
    let diff = diff_states(&parent_files, &current_map);

    if json {
        let out = StatusJson { alias: alias.clone(), branch: Some(cur.name.clone()), last_snapshot: Some(cur.snapshot_id.clone()), modified: diff.modified.iter().map(|f| f.file_path.clone()).collect(), added: diff.added.iter().map(|f| f.file_path.clone()).collect(), deleted: diff.deleted.clone() };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("{} status (branch: {}, last: {})", colorizer.bold(&alias), colorizer.cyan(&cur.name), cur.snapshot_id);
    if full_hash { println!("  (full-hash mode)"); }
    println!();
    if diff.total_changes() == 0 { println!("  no changes"); return Ok(()); }
    for f in &diff.modified { println!(" {}  {}", colorizer.yellow("modified"), f.file_path); }
    for f in &diff.added { println!(" {}     {}", colorizer.green("added"), f.file_path); }
    for p in &diff.deleted { println!(" {}   {}", colorizer.red("deleted"), p); }
    println!("\n{} file(s) changed", diff.total_changes());
    Ok(())
}
