use crate::config::{env_data_dir_override, Paths};
use crate::db::GamesDb;
use crate::error::{GError, GResult};
use crate::output::Colorizer;

pub fn run(_c: &Colorizer, alias: String, confirm: bool) -> GResult<()> {
    if !confirm { println!("warning: will delete all data for {alias}\nuse `gim remove {alias} --confirm`"); return Ok(()); }
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    let g = gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    gdb.remove(&alias)?;
    if g.data_dir.exists() { std::fs::remove_dir_all(&g.data_dir)?; }
    println!("removed {alias} and all associated data");
    Ok(())
}
