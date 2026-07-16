use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::Colorizer;
use crate::output::format_size_compact;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize)] struct Sj { snapshot_id: String, parent_snap_id: Option<String>, timestamp: i64, message: Option<String>, file_count: i64, added_size: i64, branches: Vec<String> }

pub fn run(c: &Colorizer, alias: String, oneline: bool, json: bool, n: Option<usize>) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let sdb = SnapsDb::open(&paths.snaps_db(&alias))?;
    sdb.ensure_main_branch()?;
    let mut snaps = sdb.list_snapshots()?;
    if let Some(n) = n { snaps.truncate(n); }
    let mut by_snap: HashMap<String, Vec<String>> = HashMap::new();
    for b in sdb.list_branches()? { by_snap.entry(b.snapshot_id.clone()).or_default().push(b.name.clone()); }
    let cur = sdb.get_current_branch()?.map(|b| b.name);
    if json {
        let out: Vec<Sj> = snaps.iter().map(|s| Sj { snapshot_id: s.snapshot_id.clone(), parent_snap_id: s.parent_snap_id.clone(), timestamp: s.timestamp, message: s.message.clone(), file_count: s.file_count, added_size: s.added_size, branches: by_snap.get(&s.snapshot_id).cloned().unwrap_or_default() }).collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if snaps.is_empty() { println!("{} has no snapshots", c.bold(&alias)); return Ok(()); }
    if oneline {
        for s in &snaps { let msg = s.message.clone().unwrap_or_default(); let br = by_snap.get(&s.snapshot_id).cloned().unwrap_or_default(); println!("{}  {:<30}  {} {}  {}", c.yellow(&s.snapshot_id), msg, c.dim(&format!("{} files", s.file_count)), c.dim(&format_size_compact(s.added_size)), fmt_b(&br, &cur, c)); }
        return Ok(());
    }
    println!("{} snapshot history:\n", c.bold(&alias));
    for s in &snaps {
        let br = by_snap.get(&s.snapshot_id).cloned().unwrap_or_default();
        let bm = fmt_b(&br, &cur, c);
        // Only show (HEAD) for the snapshot pointed to by the current branch.
        let is_head = cur.as_ref().and_then(|cn| {
            sdb.get_branch(cn).ok().flatten()
        }).map(|b| b.snapshot_id == s.snapshot_id).unwrap_or(false);
        let head_marker = if is_head { format!(" {}", c.dim("(HEAD)")) } else { String::new() };
        let branch_marker = if !bm.is_empty() { format!("  {}", bm) } else { String::new() };
        println!("{}{}{}", c.yellow(&s.snapshot_id), branch_marker, head_marker);
        println!("{} {}", c.dim("│"), s.message.clone().unwrap_or_else(|| "(no message)".into()));
        println!("{} {} {} {}\n", c.dim("│"), c.dim(&format!("{} files", s.file_count)), c.dim("|"), c.dim(&format_size_compact(s.added_size)));
    }
    Ok(())
}

fn fmt_b(br: &[String], cur: &Option<String>, c: &Colorizer) -> String {
    if br.is_empty() { return String::new(); }
    let parts: Vec<String> = br.iter().map(|b| {
        if cur.as_deref() == Some(b.as_str()) { format!("*{}", c.green(b)) } else { c.dim(b).to_string() }
    }).collect();
    format!("[{}]", parts.join(", "))
}
