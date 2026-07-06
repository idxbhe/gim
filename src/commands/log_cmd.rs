//! `gim log` — show snapshot history for a game.

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::Colorizer;
use crate::output::format_size_compact;
use serde::Serialize;

#[derive(Serialize)]
struct SnapJson {
    snapshot_id: String,
    parent_snap_id: Option<String>,
    timestamp: i64,
    message: Option<String>,
    file_count: i64,
    added_size: i64,
}

pub fn run(
    colorizer: &Colorizer,
    alias: String,
    oneline: bool,
    json: bool,
    n: Option<usize>,
) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(override_dir) = env_data_dir_override() {
        paths = paths.with_data_dir(override_dir);
    }
    paths.ensure_data_dir()?;

    let games_db = GamesDb::open(&paths.games_db)?;
    let _game = games_db
        .get(&alias)?
        .ok_or_else(|| GError::AliasNotFound(alias.clone()))?;

    let snaps_db_path = paths.snaps_db(&alias);
    let snaps_db = SnapsDb::open(&snaps_db_path)?;
    let mut snapshots = snaps_db.list_snapshots()?;
    if let Some(n) = n {
        snapshots.truncate(n);
    }

    if json {
        let out: Vec<SnapJson> = snapshots
            .iter()
            .map(|s| SnapJson {
                snapshot_id: s.snapshot_id.clone(),
                parent_snap_id: s.parent_snap_id.clone(),
                timestamp: s.timestamp,
                message: s.message.clone(),
                file_count: s.file_count,
                added_size: s.added_size,
            })
            .collect();
        let json = serde_json::to_string_pretty(&out)?;
        println!("{json}");
        return Ok(());
    }

    if snapshots.is_empty() {
        println!("{} has no snapshots", colorizer.bold(&alias));
        return Ok(());
    }

    if oneline {
        for (i, s) in snapshots.iter().enumerate() {
            let msg = s.message.clone().unwrap_or_default();
            let marker = if i == 0 { "(latest)" } else { "" };
            println!(
                "{}  {:<40}  {} files {} {}",
                colorizer.bold(&s.snapshot_id),
                msg,
                s.file_count,
                format_size_compact(s.added_size),
                colorizer.dim(marker),
            );
        }
        return Ok(());
    }

    println!("{} snapshot history:", colorizer.bold(&alias));
    println!();
    for (i, s) in snapshots.iter().enumerate() {
        let marker = if i == 0 { "  (latest)" } else { "" };
        println!("  {}{}", colorizer.bold(&s.snapshot_id), marker);
        let msg = s.message.clone().unwrap_or_else(|| "(no message)".into());
        println!("  │  {msg}");
        println!(
            "  │  {} files | {}",
            s.file_count,
            format_size_compact(s.added_size)
        );
        if i < snapshots.len() - 1 {
            println!("  │");
        }
    }
    Ok(())
}
