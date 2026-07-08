//! `gim gc`

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::Colorizer;
use crate::output::format_size;
use crate::storage::Cas;

pub fn run(colorizer: &Colorizer, alias: String, dry_run: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let games_db = GamesDb::open(&paths.games_db)?;
    games_db.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let snaps_db = SnapsDb::open(&paths.snaps_db(&alias))?;
    let referenced = snaps_db.referenced_hashes()?;
    let cas = Cas::new(paths.objects_dir(&alias));
    let stored = cas.list_all_hashes()?;
    let orphaned: Vec<&String> = stored.iter().filter(|h| !referenced.contains(*h)).collect();
    let tmps = cas.list_tmp_files()?;
    let mut freed: i64 = 0;
    for h in &orphaned { if let Ok(m) = std::fs::metadata(crate::path_utils::object_path(&paths.objects_dir(&alias), h)) { freed += m.len() as i64; } }
    for t in &tmps { if let Ok(m) = std::fs::metadata(t) { freed += m.len() as i64; } }
    if dry_run {
        println!("dry run: would gc {}", colorizer.green(&alias));
        println!("  {} orphaned objects", orphaned.len());
        println!("  {} stray .tmp files", tmps.len());
        println!("  would free {}", format_size(freed));
        return Ok(());
    }
    let mut removed = 0;
    for h in &orphaned { if cas.delete(h.as_str())? { removed += 1; } }
    let mut tr = 0;
    for t in &tmps { if std::fs::remove_file(t).is_ok() { tr += 1; } }
    println!("garbage collected {}", colorizer.green(&alias));
    println!("  removed {removed} orphaned objects");
    if tr > 0 { println!("  removed {tr} stray .tmp files"); }
    println!("  freed {}", format_size(freed));
    Ok(())
}
