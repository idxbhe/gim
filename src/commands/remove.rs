//! `gim remove`

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
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let games_db = GamesDb::open(&paths.games_db)?;
    let game = games_db.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    games_db.remove(&alias)?;
    if game.data_dir.exists() { std::fs::remove_dir_all(&game.data_dir)?; }
    println!("removed {alias} and all associated data");
    Ok(())
}
