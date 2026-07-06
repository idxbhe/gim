//! `gim list` — list all tracked games.

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::GResult;
use crate::output::Colorizer;
use crate::output::format_timestamp;
use serde::Serialize;

#[derive(Serialize)]
struct GameJson {
    alias: String,
    title: String,
    game_dir: String,
    data_dir: String,
    added_at: i64,
    snapshot_count: usize,
}

pub fn run(colorizer: &Colorizer, details: bool, json: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(override_dir) = env_data_dir_override() {
        paths = paths.with_data_dir(override_dir);
    }
    paths.ensure_data_dir()?;
    let games_db = GamesDb::open(&paths.games_db)?;
    let games = games_db.list()?;

    if json {
        let mut out = Vec::with_capacity(games.len());
        for g in &games {
            let snaps_db_path = g.data_dir.join("snaps.db");
            let snapshot_count = match SnapsDb::open(&snaps_db_path) {
                Ok(db) => db.list_snapshots().map(|v| v.len()).unwrap_or(0),
                Err(_) => 0,
            };
            out.push(GameJson {
                alias: g.alias.clone(),
                title: g.title.clone(),
                game_dir: g.game_dir.to_string_lossy().into_owned(),
                data_dir: g.data_dir.to_string_lossy().into_owned(),
                added_at: g.added_at,
                snapshot_count,
            });
        }
        let json = serde_json::to_string_pretty(&out)?;
        println!("{json}");
        return Ok(());
    }

    if games.is_empty() {
        println!("no games tracked — use `gim add <alias> <game_dir>` to add one");
        return Ok(());
    }

    if details {
        for g in &games {
            println!("{}", colorizer.bold(&g.alias));
            println!("  title:    {}", g.title);
            println!("  gameDir:  {}", g.game_dir.display());
            println!("  dataDir:  {}", g.data_dir.display());
            println!("  addedAt:  {}", format_timestamp(g.added_at * 1000));
            let snaps_db_path = g.data_dir.join("snaps.db");
            let count = match SnapsDb::open(&snaps_db_path) {
                Ok(db) => db.list_snapshots().map(|v| v.len()).unwrap_or(0),
                Err(_) => 0,
            };
            println!("  snaps:    {count} snapshots");
            println!();
        }
        return Ok(());
    }

    let max_alias = games.iter().map(|g| g.alias.len()).max().unwrap_or(0);
    let max_title = games.iter().map(|g| g.title.len()).max().unwrap_or(0);
    for g in &games {
        let alias_padded = format!("{:<width$}", g.alias, width = max_alias);
        let title_padded = format!("{:<width$}", g.title, width = max_title);
        println!(
            "{}  {}  `{}`",
            colorizer.green(&alias_padded),
            title_padded,
            g.game_dir.display()
        );
    }
    Ok(())
}
