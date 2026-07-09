use crate::config::{env_data_dir_override, Paths};
use crate::db::GamesDb;
use crate::error::{GError, GResult};
use crate::ignore_mod::{build_for_game, IgnoreSet};
use crate::output::Colorizer;
use std::fs;
use std::io::Write;

pub fn run(_c: &Colorizer, alias: String, add: Option<String>, remove: Option<String>, list: bool, edit: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    let game = gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let pg = paths.per_game_gignore(&alias);
    if let Some(p) = add { let mut f = fs::OpenOptions::new().create(true).append(true).open(&pg)?; writeln!(f, "{p}")?; println!("added \"{p}\" to {alias}/.gignore"); return Ok(()); }
    if let Some(p) = remove { if !pg.exists() { println!("no .gignore for {alias}"); return Ok(()); } let c = fs::read_to_string(&pg)?; let kept: Vec<&str> = c.lines().filter(|l| l.trim() != p.trim()).collect(); fs::write(&pg, kept.join("\n") + "\n")?; println!("removed \"{p}\""); return Ok(()); }
    if list || (!edit && add.is_none() && remove.is_none()) { let set: IgnoreSet = build_for_game(&paths, &alias, &game.game_dir)?; println!("{alias} ignore patterns:\n"); for src in &set.sources { println!("  {}", src.label); for p in &src.patterns { println!("    {p}"); } println!(); } return Ok(()); }
    if edit { if !pg.exists() { fs::write(&pg, b"")?; } let ed = std::env::var("EDITOR").unwrap_or_else(|_| if cfg!(windows) { "notepad".into() } else { "vi".into() }); let st = std::process::Command::new(&ed).arg(&pg).status().map_err(|e| GError::Other(format!("editor \"{ed}\": {e}")))?; if !st.success() { return Err(GError::Other(format!("editor exited {st}"))); } }
    Ok(())
}
