//! `gim ignore` — manage ignore patterns for a game.

use crate::config::{env_data_dir_override, Paths};
use crate::db::GamesDb;
use crate::error::{GError, GResult};
use crate::ignore_mod::{build_for_game, IgnoreSet};
use crate::output::Colorizer;
use std::fs;
use std::io::Write;

pub fn run(
    _colorizer: &Colorizer,
    alias: String,
    add: Option<String>,
    remove: Option<String>,
    list: bool,
    edit: bool,
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

    let per_game_gignore = paths.per_game_gignore(&alias);

    if let Some(p) = add {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&per_game_gignore)?;
        writeln!(file, "{p}")?;
        println!("added pattern \"{p}\" to {alias}/.gignore");
        return Ok(());
    }

    if let Some(p) = remove {
        if !per_game_gignore.exists() {
            println!("no .gignore for {alias} — nothing to remove");
            return Ok(());
        }
        let contents = fs::read_to_string(&per_game_gignore)?;
        let kept: Vec<&str> = contents
            .lines()
            .filter(|line| line.trim() != p.trim())
            .collect();
        fs::write(&per_game_gignore, kept.join("\n") + "\n")?;
        println!("removed pattern \"{p}\" from {alias}/.gignore");
        return Ok(());
    }

    if list || (!edit && add.is_none() && remove.is_none()) {
        let set: IgnoreSet = build_for_game(&paths, &alias, &game.game_dir)?;
        println!("{alias} ignore patterns (global + per-game + in-game):");
        println!();
        for src in &set.sources {
            println!("  {}", src.label);
            for p in &src.patterns {
                println!("    {p}");
            }
            println!();
        }
        return Ok(());
    }

    if edit {
        if !per_game_gignore.exists() {
            fs::write(&per_game_gignore, b"")?;
        }
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".into()
            } else {
                "vi".into()
            }
        });
        let status = std::process::Command::new(&editor)
            .arg(&per_game_gignore)
            .status()
            .map_err(|e| GError::Other(format!("cannot launch editor \"{editor}\": {e}")))?;
        if !status.success() {
            return Err(GError::Other(format!(
                "editor \"{editor}\" exited with status {status}"
            )));
        }
    }
    Ok(())
}
