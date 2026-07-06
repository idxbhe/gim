//! `g remove` — remove a game and all its associated data.
//!
//! Per spec:
//! - `--confirm` flag is required. Without it, print a warning and exit.
//! - Deletes the game record from `games.db`.
//! - Recursively deletes the entire `data/[alias]/` directory.
//! - This action is **irreversible**.

use crate::config::{env_data_dir_override, Paths};
use crate::db::GamesDb;
use crate::error::{GError, GResult};
use crate::output::Colorizer;

pub fn run(
    _colorizer: &Colorizer,
    alias: String,
    confirm: bool,
) -> GResult<()> {
    if !confirm {
        println!("warning: this will permanently delete all snapshots and data for {alias}");
        println!("use `g remove {alias} --confirm` to proceed");
        return Ok(());
    }

    let mut paths = Paths::from_env()?;
    if let Some(override_dir) = env_data_dir_override() {
        paths = paths.with_data_dir(override_dir);
    }
    paths.ensure_data_dir()?;

    let games_db = GamesDb::open(&paths.games_db)?;
    let game = games_db.get(&alias)?;
    let game = match game {
        Some(g) => g,
        None => return Err(GError::AliasNotFound(alias.clone())),
    };

    // Delete the game record from games.db
    let removed = games_db.remove(&alias)?;
    debug_assert!(removed);

    // Recursively delete the data directory
    let data_dir = &game.data_dir;
    if data_dir.exists() {
        std::fs::remove_dir_all(data_dir)?;
    }

    println!("removed {alias} and all associated data");
    Ok(())
}
