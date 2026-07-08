//! `gim add`

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::Colorizer;
use std::path::PathBuf;

pub fn run(colorizer: &Colorizer, alias: String, game_dir: PathBuf, title: Option<String>, data_dir: Option<PathBuf>) -> GResult<()> {
    validate_alias(&alias)?;
    let abs_game_dir = if game_dir.is_absolute() { game_dir } else { std::env::current_dir()?.join(&game_dir) };
    if !abs_game_dir.exists() { return Err(GError::GameDirMissing(abs_game_dir)); }
    if !abs_game_dir.is_dir() { return Err(GError::GameDirNotDir(abs_game_dir)); }
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let game_data_dir = data_dir.unwrap_or_else(|| paths.game_data_dir(&alias));
    let title = title.unwrap_or_else(|| abs_game_dir.file_name().and_then(|n| n.to_str()).unwrap_or(&alias).to_string());
    let games_db = GamesDb::open(&paths.games_db)?;
    games_db.add(&alias, &title, &abs_game_dir, &game_data_dir)?;
    std::fs::create_dir_all(game_data_dir.join("objects"))?;
    let _ = SnapsDb::open(&game_data_dir.join("snaps.db"))?;
    println!("successfully added {} as {}", title, colorizer.green(&alias));
    Ok(())
}

fn validate_alias(alias: &str) -> GResult<()> {
    if alias.is_empty() { return Err(GError::Other("alias cannot be empty".into())); }
    if alias.starts_with('.') { return Err(GError::Other(format!("alias \"{alias}\" cannot start with a dot"))); }
    if !alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
        return Err(GError::Other(format!("alias \"{alias}\" contains invalid characters")));
    }
    Ok(())
}
