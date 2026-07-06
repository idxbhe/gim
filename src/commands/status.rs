//! `gim status` — show file changes since the last snapshot.
//!
//! Uses the mtime+size fast pre-filter: only files whose size or mtime
//! differs from the last snapshot are re-hashed. Files whose size+mtime
//! match are assumed unchanged. Pass `--full-hash` to bypass the
//! pre-filter.

use crate::config::{env_data_dir_override, Paths};
use crate::db::{diff_states, GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::ignore_mod;
use crate::output::Colorizer;
use crate::walker::{walk_and_hash, WalkOptions};
use serde::Serialize;

#[derive(Serialize)]
struct StatusJson {
    alias: String,
    last_snapshot: Option<String>,
    modified: Vec<String>,
    added: Vec<String>,
    deleted: Vec<String>,
}

pub fn run(
    colorizer: &Colorizer,
    alias: String,
    threads: Option<usize>,
    json: bool,
    full_hash: bool,
) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(override_dir) = env_data_dir_override() {
        paths = paths.with_data_dir(override_dir);
    }
    paths.ensure_data_dir()?;

    let games_db = GamesDb::open(&paths.games_db)?;
    let game = games_db
        .get(&alias)?
        .ok_or_else(|| GError::AliasNotFound(alias.clone()))?;

    let snaps_db_path = paths.snaps_db(&alias);
    let snaps_db = SnapsDb::open(&snaps_db_path)?;
    let latest = snaps_db
        .latest_snapshot()?
        .ok_or_else(|| GError::NoSnapshots(alias.clone()))?;
    let last_snapshot_id = latest.snapshot_id.clone();
    let parent_files = snaps_db.files_for_snapshot(&last_snapshot_id)?;

    let ignore_set = ignore_mod::build_for_game(&paths, &alias, &game.game_dir)?;
    let walk_opts = WalkOptions {
        threads: threads.unwrap_or(0),
        full_hash,
        ..WalkOptions::default()
    };
    let reference = if full_hash { None } else { Some(&parent_files) };
    let (hashed, _locked) = walk_and_hash(&game.game_dir, &ignore_set, reference, &walk_opts)?;

    let mut current_map = std::collections::HashMap::with_capacity(hashed.len());
    for f in hashed {
        current_map.insert(
            f.file_path,
            crate::db::FileMeta {
                hash: f.hash,
                file_size: f.file_size,
                modified_time: f.modified_time,
            },
        );
    }

    let diff = diff_states(&parent_files, &current_map);

    if json {
        let out = StatusJson {
            alias: alias.clone(),
            last_snapshot: Some(last_snapshot_id.clone()),
            modified: diff.modified.iter().map(|f| f.file_path.clone()).collect(),
            added: diff.added.iter().map(|f| f.file_path.clone()).collect(),
            deleted: diff.deleted.clone(),
        };
        let json = serde_json::to_string_pretty(&out)?;
        println!("{json}");
        return Ok(());
    }

    println!(
        "{} status (vs last snapshot: {})",
        colorizer.bold(&alias),
        last_snapshot_id
    );
    if full_hash {
        println!("  (full-hash mode: every file was re-hashed)");
    }
    println!();
    if diff.added.is_empty() && diff.modified.is_empty() && diff.deleted.is_empty() {
        println!("  no changes");
        return Ok(());
    }
    for f in &diff.modified {
        println!(" {}  {}", colorizer.yellow("modified"), f.file_path);
    }
    for f in &diff.added {
        println!(" {}     {}", colorizer.green("added"), f.file_path);
    }
    for p in &diff.deleted {
        println!(" {}   {}", colorizer.red("deleted"), p);
    }
    println!();
    println!(
        "{} file(s) changed",
        diff.added.len() + diff.modified.len() + diff.deleted.len()
    );
    Ok(())
}
