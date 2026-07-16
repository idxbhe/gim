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

    // ── Handle branches pointing to this snapshot ──────────────────
    // Instead of refusing, auto-move branches to the parent snapshot.
    let branches = sdb.branches_for_snapshot(&sid)?;
    let parent_id = snap.parent_snap_id.clone();

    if !branches.is_empty() {
        if parent_id.is_none() && !force {
            println!("warning: \"{}\" is referenced by {} branch(es): {}", sid, branches.len(), branches.join(", "));
            println!("  this snapshot has no parent — branches cannot be auto-moved.");
            println!("  use `gim delete {alias} {sid} --force` to delete anyway (branches will point to nothing).");
            return Ok(());
        }

        if dry_run {
            println!("dry run: would delete {}", c.bold(&sid));
            if !branches.is_empty() {
                let target = parent_id.as_deref().unwrap_or("(deleted)");
                println!("  move {} branch(es) to {}", branches.len(), target);
                for b in &branches { println!("    {b} → {target}"); }
            }
            let children = sdb.children_of(&sid)?;
            if !children.is_empty() {
                println!("  re-parent {} child(ren) to {}", children.len(), parent_id.as_deref().unwrap_or("(root)"));
            }
            println!("  delete file + deleted_files rows");
            println!("  delete snaps row");
            println!("\n  note: run `gim gc {alias}` to free orphans");
            return Ok(());
        }

        // Move branches to parent.
        for bname in &branches {
            if let Some(ref pid) = parent_id {
                sdb.update_branch_snapshot(bname, pid)?;
                println!("  moved branch {} → {}", c.green(bname), c.bold(pid));
            } else {
                // No parent — branch points to nothing. This is a
                // degenerate state but we allow it with --force.
                log::warn!("branch {} has no parent to move to", bname);
            }
        }
    } else {
        // No branches reference this snapshot — check if root.
        let is_root = sid == "original" || snap.parent_snap_id.is_none();
        if is_root && !force {
            let n = sdb.children_of(&sid)?.len();
            println!("warning: \"{}\" is root. {n} child(ren) will become new roots.", sid);
            println!("use `gim delete {alias} {sid} --force`");
            return Ok(());
        }

        if dry_run {
            let children = sdb.children_of(&sid)?;
            println!("dry run: would delete {}", c.bold(&sid));
            if !children.is_empty() {
                println!("  re-parent {} child(ren) to {}", children.len(), parent_id.as_deref().unwrap_or("(root)"));
            }
            println!("  delete file + deleted_files rows");
            println!("  delete snaps row");
            println!("\n  note: run `gim gc {alias}` to free orphans");
            return Ok(());
        }
    }

    // ── Re-parent children ─────────────────────────────────────────
    let children = sdb.children_of(&sid)?;
    let np = snap.parent_snap_id.clone();

    let tx = sdb.transaction()?;
    for ch in &children {
        let n = tx.execute("UPDATE snaps SET parentSnapId = ?1 WHERE snapshotId = ?2", rusqlite::params![np, ch])?;
        debug_assert_eq!(n, 1);
    }
    let fd = tx.execute("DELETE FROM files WHERE snapshotId = ?1", rusqlite::params![sid])?;
    let dd = tx.execute("DELETE FROM deleted_files WHERE snapshotId = ?1", rusqlite::params![sid])?;
    let sd = tx.execute("DELETE FROM snaps WHERE snapshotId = ?1", rusqlite::params![sid])?;
    if sd == 0 { tx.rollback()?; return Err(GError::SnapshotNotFound(sid.clone(), alias.clone())); }
    tx.commit()?;

    println!("deleted {}", c.bold(&sid));
    if !children.is_empty() {
        println!("  re-parented {} child(ren) to {}", children.len(), np.as_deref().unwrap_or("(root)"));
    }
    println!("  removed {fd} file rows, {dd} deleted-file rows");
    println!("  run `gim gc {alias}` to free space");
    Ok(())
}
