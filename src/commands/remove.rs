//! `gim remove` — remove a game and all its associated data.

use crate::config::{env_data_dir_override, Paths};
use crate::db::GamesDb;
use crate::error::{GError, GResult};
use crate::output::Colorizer;

pub fn run(_colorizer: &Colorizer, alias: String, confirm: bool) -> GResult<()> {
    if !confirm {
        println!("warning: this will permanently delete all snapshots and data for {alias}");
        println!("use `gim remove {alias} --confirm` to proceed");
        return Ok(());
    }

    let mut paths = Paths::from_env()?;
    if let Some(override_dir) = env_data_dir_override() {
        paths = paths.with_data_dir(override_dir);
    }
    paths.ensure_data_dir()?;

    let games_db = GamesDb::open(&paths.games_db)?;
    let game = games_db
        .get(&alias)?
        .ok_or_else(|| GError::AliasNotFound(alias.clone()))?;

    let removed = games_db.remove(&alias)?;
    debug_assert!(removed);

    let data_dir = &game.data_dir;
    if data_dir.exists() {
        std::fs::remove_dir_all(data_dir)?;
    }

    println!("removed {alias} and all associated data");
    Ok(())
}
