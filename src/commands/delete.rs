use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::locking;
use crate::output::Colorizer;

pub fn run(c: &Colorizer, alias: String, sid: String, dry_run: bool, force: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let sdb_path = paths.snaps_db(&alias);
    let mut sdb = SnapsDb::open(&sdb_path)?;
    let snap = sdb.get_snapshot(&sid)?.ok_or_else(|| GError::SnapshotNotFound(sid.clone(), alias.clone()))?;
    let _lock = locking::acquire_game_lock(&alias, &sdb_path)?;
    let br = sdb.branches_for_snapshot(&sid)?;
    if !br.is_empty() { return Err(GError::SnapshotReferencedByBranch(sid, br.len(), br.join(", "))); }
    let is_root = sid == "original" || snap.parent_snap_id.is_none();
    if is_root && !force {
        let n = sdb.children_of(&sid)?.len();
        println!("warning: \"{}\" is root. {n} child(ren) will become new roots.\nuse `gim delete {alias} {sid} --force`", sid);
        return Ok(());
    }
    let children = sdb.children_of(&sid)?;
    let np = snap.parent_snap_id.clone();
    if dry_run {
        println!("dry run: would delete {}", c.bold(&sid));
        if !children.is_empty() { println!("  re-parent {} child(ren) to {}", children.len(), np.as_deref().unwrap_or("(root)")); for c2 in &children { println!("    {c2}"); } }
        println!("  delete file + deleted_files rows\n  delete snaps row\n\n  note: run `gim gc {alias}` to free orphans");
        return Ok(());
    }
    let tx = sdb.transaction()?;
    for ch in &children { let n = tx.execute("UPDATE snaps SET parentSnapId = ?1 WHERE snapshotId = ?2", rusqlite::params![np, ch])?; debug_assert_eq!(n, 1); }
    let fd = tx.execute("DELETE FROM files WHERE snapshotId = ?1", rusqlite::params![sid])?;
    let dd = tx.execute("DELETE FROM deleted_files WHERE snapshotId = ?1", rusqlite::params![sid])?;
    let sd = tx.execute("DELETE FROM snaps WHERE snapshotId = ?1", rusqlite::params![sid])?;
    if sd == 0 { tx.rollback()?; return Err(GError::SnapshotNotFound(sid.clone(), alias.clone())); }
    tx.commit()?;
    println!("deleted {}", c.bold(&sid));
    if !children.is_empty() { println!("  re-parented {} child(ren) to {}", children.len(), np.as_deref().unwrap_or("(root)")); }
    println!("  removed {fd} file rows, {dd} deleted-file rows\n  run `gim gc {alias}` to free space");
    Ok(())
}
