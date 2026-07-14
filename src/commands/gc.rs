use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::{Colorizer, ProgressReporter};
use crate::output::format_size;
use crate::storage::Cas;

pub fn run(c: &Colorizer, alias: String, dry_run: bool, progress: &ProgressReporter) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let sdb = SnapsDb::open(&paths.snaps_db(&alias))?;
    let ref_h = sdb.referenced_hashes()?;
    let cas = Cas::new(paths.objects_dir(&alias));
    cas.cleanup_tmp_files()?;

    // ── Scan phase ──────────────────────────────────────────────────
    progress.scan_start();
    let stored = cas.list_all_hashes()?;
    let scan_count = stored.len() as u64;
    progress.scan_done(scan_count);

    let orph: Vec<&String> = stored.iter().filter(|h| !ref_h.contains(*h)).collect();
    let tmps = cas.list_tmp_files()?;
    let mut freed: i64 = 0;
    for h in &orph { if let Ok(m) = std::fs::metadata(crate::path_utils::object_path(&paths.objects_dir(&alias), h)) { freed += m.len() as i64; } }
    for t in &tmps { if let Ok(m) = std::fs::metadata(t) { freed += m.len() as i64; } }

    if dry_run {
        println!("dry run: would gc {}\n  {} orphaned objects\n  {} stray .tmp files\n  would free {}", c.green(&alias), orph.len(), tmps.len(), format_size(freed));
        return Ok(());
    }

    // ── Delete phase ────────────────────────────────────────────────
    let total_delete = orph.len() + tmps.len();
    progress.delete_start(total_delete);
    let mut removed = 0;
    for h in &orph {
        if cas.delete(h.as_str())? { removed += 1; }
        progress.delete_tick();
    }
    let mut tr = 0;
    for t in &tmps {
        if std::fs::remove_file(t).is_ok() { tr += 1; }
        progress.delete_tick();
    }
    let del_count = (removed + tr) as u64;
    progress.delete_done(del_count);

    println!("garbage collected {}", c.green(&alias));
    println!("  removed {removed} orphaned objects");
    if tr > 0 { println!("  removed {tr} stray .tmp files"); }
    println!("  freed {}", format_size(freed));
    Ok(())
}
