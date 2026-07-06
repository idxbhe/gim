//! `gim add` — register a new game for tracking.

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::Colorizer;
use std::path::PathBuf;

pub fn run(
    colorizer: &Colorizer,
    alias: String,
    game_dir: PathBuf,
    title: Option<String>,
    data_dir: Option<PathBuf>,
) -> GResult<()> {
    validate_alias(&alias)?;

    let abs_game_dir = if game_dir.is_absolute() {
        game_dir.clone()
    } else {
        std::env::current_dir()?.join(&game_dir)
    };
    if !abs_game_dir.exists() {
        return Err(GError::GameDirMissing(abs_game_dir));
    }
    if !abs_game_dir.is_dir() {
        return Err(GError::GameDirNotDir(abs_game_dir));
    }

    let mut paths = Paths::from_env()?;
    if let Some(override_dir) = env_data_dir_override() {
        paths = paths.with_data_dir(override_dir);
    }
    paths.ensure_data_dir()?;

    let game_data_dir = match data_dir {
        Some(d) => d,
        None => paths.game_data_dir(&alias),
    };

    let title = title.unwrap_or_else(|| {
        abs_game_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&alias)
            .to_string()
    });

    let games_db = GamesDb::open(&paths.games_db)?;
    games_db.add(&alias, &title, &abs_game_dir, &game_data_dir)?;

    let objects_dir = game_data_dir.join("objects");
    std::fs::create_dir_all(&objects_dir)?;

    let snaps_db_path = game_data_dir.join("snaps.db");
    let _snaps_db = SnapsDb::open(&snaps_db_path)?;

    println!(
        "successfully added {} as {}",
        title,
        colorizer.green(&alias)
    );
    Ok(())
}

fn validate_alias(alias: &str) -> GResult<()> {
    if alias.is_empty() {
        return Err(GError::Other("alias cannot be empty".into()));
    }
    if alias.starts_with('.') {
        return Err(GError::Other(format!(
            "alias \"{alias}\" cannot start with a dot"
        )));
    }
    if !alias
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(GError::Other(format!(
            "alias \"{alias}\" contains invalid characters (allowed: A-Z a-z 0-9 . _ -)"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_alias() {
        assert!(validate_alias("").is_err());
    }

    #[test]
    fn rejects_leading_dot() {
        assert!(validate_alias(".hidden").is_err());
    }

    #[test]
    fn rejects_special_chars() {
        assert!(validate_alias("foo/bar").is_err());
        assert!(validate_alias("foo bar").is_err());
    }

    #[test]
    fn accepts_valid_aliases() {
        assert!(validate_alias("mario").is_ok());
        assert!(validate_alias("elder_scrolls_v").is_ok());
        assert!(validate_alias("game.1").is_ok());
    }
}
