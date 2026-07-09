use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::GResult;
use crate::output::Colorizer;
use crate::output::format_timestamp;
use serde::Serialize;

#[derive(Serialize)] struct Gj { alias: String, title: String, game_dir: String, data_dir: String, added_at: i64, snapshot_count: usize }

pub fn run(c: &Colorizer, details: bool, json: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let games = GamesDb::open(&paths.games_db)?.list()?;
    if json {
        let mut out = Vec::new();
        for g in &games {
            let sc = SnapsDb::open(&g.data_dir.join("snaps.db")).ok().and_then(|db| db.list_snapshots().map(|v| v.len()).ok()).unwrap_or(0);
            out.push(Gj { alias: g.alias.clone(), title: g.title.clone(), game_dir: g.game_dir.to_string_lossy().into_owned(), data_dir: g.data_dir.to_string_lossy().into_owned(), added_at: g.added_at, snapshot_count: sc });
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if games.is_empty() { println!("no games tracked"); return Ok(()); }
    if details {
        for g in &games {
            println!("{}", c.bold(&g.alias));
            println!("  title:    {}", g.title);
            println!("  gameDir:  {}", g.game_dir.display());
            println!("  dataDir:  {}", g.data_dir.display());
            println!("  addedAt:  {}", format_timestamp(g.added_at * 1000));
            let sc = SnapsDb::open(&g.data_dir.join("snaps.db")).ok().and_then(|db| db.list_snapshots().map(|v| v.len()).ok()).unwrap_or(0);
            println!("  snaps:    {sc} snapshots\n");
        }
        return Ok(());
    }
    let ma = games.iter().map(|g| g.alias.len()).max().unwrap_or(0);
    let mt = games.iter().map(|g| g.title.len()).max().unwrap_or(0);
    for g in &games { println!("{}  {}  `{}`", c.green(&format!("{:<ma$}", g.alias)), format!("{:<mt$}", g.title), g.game_dir.display()); }
    Ok(())
}
