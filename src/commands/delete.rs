//! `gim delete` — delete a snapshot with re-parenting.

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::locking;
use crate::output::Colorizer;

pub fn run(colorizer: &Colorizer, alias: String, snapshot_id: String, dry_run: bool, force: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let games_db = GamesDb::open(&paths.games_db)?;
    games_db.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let snaps_db_path = paths.snaps_db(&alias);
    let mut snaps_db = SnapsDb::open(&snaps_db_path)?;
    let snap = snaps_db.get_snapshot(&snapshot_id)?.ok_or_else(|| GError::SnapshotNotFound(snapshot_id.clone(), alias.clone()))?;
    let _lock = locking::acquire_game_lock(&alias, &snaps_db_path)?;

    let branches = snaps_db.branches_for_snapshot(&snapshot_id)?;
    if !branches.is_empty() { return Err(GError::SnapshotReferencedByBranch(snapshot_id, branches.len(), branches.join(", "))); }

    let is_root = snap.snapshot_id == "original" || snap.parent_snap_id.is_none();
    if is_root && !force {
        let n = snaps_db.children_of(&snapshot_id)?.len();
        println!("warning: \"{}\" is a root snapshot. Deleting will make {} child(ren) become new roots.", snapshot_id, n);
        println!("use `gim delete {alias} {snapshot_id} --force` to proceed");
        return Ok(());
    }

    let children = snaps_db.children_of(&snapshot_id)?;
    let new_parent = snap.parent_snap_id.clone();

    if dry_run {
        println!("dry run: would delete snapshot {}", colorizer.bold(&snapshot_id));
        if !children.is_empty() {
            println!("  re-parent {} child(ren) to {}", children.len(), new_parent.as_deref().unwrap_or("(root)"));
            for c in &children { println!("    {c}"); }
        }
        println!("  delete all file + deleted_files rows for this snapshot");
        println!("  delete the snaps row");
        println!("\n  note: run `gim gc {alias}` to free orphaned objects");
        return Ok(());
    }

    let tx = snaps_db.transaction()?;
    for child in &children {
        let n = tx.execute("UPDATE snaps SET parentSnapId = ?1 WHERE snapshotId = ?2", rusqlite::params![new_parent, child])?;
        debug_assert_eq!(n, 1);
    }
    let fd = tx.execute("DELETE FROM files WHERE snapshotId = ?1", rusqlite::params![snapshot_id])?;
    let dd = tx.execute("DELETE FROM deleted_files WHERE snapshotId = ?1", rusqlite::params![snapshot_id])?;
    let sd = tx.execute("DELETE FROM snaps WHERE snapshotId = ?1", rusqlite::params![snapshot_id])?;
    if sd == 0 { tx.rollback()?; return Err(GError::SnapshotNotFound(snapshot_id.clone(), alias.clone())); }
    tx.commit()?;

    println!("deleted snapshot {}", colorizer.bold(&snapshot_id));
    if !children.is_empty() { println!("  re-parented {} child(ren) to {}", children.len(), new_parent.as_deref().unwrap_or("(root)")); }
    println!("  removed {fd} file rows, {dd} deleted-file rows");
    println!("  run `gim gc {alias}` to free disk space");
    Ok(())
}
