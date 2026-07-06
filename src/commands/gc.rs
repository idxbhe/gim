//! `gim gc` — garbage-collect unreferenced objects.

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::Colorizer;
use crate::output::format_size;
use crate::storage::Cas;

pub fn run(colorizer: &Colorizer, alias: String, dry_run: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(override_dir) = env_data_dir_override() {
        paths = paths.with_data_dir(override_dir);
    }
    paths.ensure_data_dir()?;

    let games_db = GamesDb::open(&paths.games_db)?;
    let _game = games_db
        .get(&alias)?
        .ok_or_else(|| GError::AliasNotFound(alias.clone()))?;

    let snaps_db_path = paths.snaps_db(&alias);
    let snaps_db = SnapsDb::open(&snaps_db_path)?;

    let referenced = snaps_db.referenced_hashes()?;
    let cas = Cas::new(paths.objects_dir(&alias));
    let stored = cas.list_all_hashes()?;
    let orphaned: Vec<&String> = stored.iter().filter(|h| !referenced.contains(*h)).collect();
    let tmp_files = cas.list_tmp_files()?;

    let mut freed: i64 = 0;
    for h in &orphaned {
        if let Ok(meta) = std::fs::metadata(crate::path_utils::object_path(
            &paths.objects_dir(&alias),
            h,
        )) {
            freed += meta.len() as i64;
        }
    }
    for t in &tmp_files {
        if let Ok(meta) = std::fs::metadata(t) {
            freed += meta.len() as i64;
        }
    }

    if dry_run {
        println!("dry run: would garbage collect {}", colorizer.green(&alias));
        println!("  {} orphaned objects", orphaned.len());
        println!("  {} stray .tmp files", tmp_files.len());
        println!("  would free {}", format_size(freed));
        return Ok(());
    }

    let mut removed = 0usize;
    for h in &orphaned {
        if cas.delete(h.as_str())? {
            removed += 1;
        }
    }
    let mut tmp_removed = 0usize;
    for t in &tmp_files {
        if std::fs::remove_file(t).is_ok() {
            tmp_removed += 1;
        }
    }

    println!("garbage collected {}", colorizer.green(&alias));
    println!("  removed {removed} orphaned objects");
    if tmp_removed > 0 {
        println!("  removed {tmp_removed} stray .tmp files");
    }
    println!("  freed {}", format_size(freed));
    Ok(())
}
