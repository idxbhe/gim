//! `gim log`

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::Colorizer;
use crate::output::format_size_compact;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize)]
struct SnapJson { snapshot_id: String, parent_snap_id: Option<String>, timestamp: i64, message: Option<String>, file_count: i64, added_size: i64, branches: Vec<String> }

pub fn run(colorizer: &Colorizer, alias: String, oneline: bool, json: bool, n: Option<usize>) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let games_db = GamesDb::open(&paths.games_db)?;
    games_db.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let snaps_db = SnapsDb::open(&paths.snaps_db(&alias))?;
    let mut snaps = snaps_db.list_snapshots()?;
    if let Some(n) = n { snaps.truncate(n); }
    let mut by_snap: HashMap<String, Vec<String>> = HashMap::new();
    for b in snaps_db.list_branches()? { by_snap.entry(b.snapshot_id.clone()).or_default().push(b.name.clone()); }
    let cur = snaps_db.get_current_branch()?.map(|b| b.name);

    if json {
        let out: Vec<SnapJson> = snaps.iter().map(|s| SnapJson { snapshot_id: s.snapshot_id.clone(), parent_snap_id: s.parent_snap_id.clone(), timestamp: s.timestamp, message: s.message.clone(), file_count: s.file_count, added_size: s.added_size, branches: by_snap.get(&s.snapshot_id).cloned().unwrap_or_default() }).collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if snaps.is_empty() { println!("{} has no snapshots", colorizer.bold(&alias)); return Ok(()); }
    if oneline {
        for s in &snaps {
            let msg = s.message.clone().unwrap_or_default();
            let br = by_snap.get(&s.snapshot_id).cloned().unwrap_or_default();
            let bm = fmt_branches(&br, &cur);
            println!("{}  {:<30}  {} files {}  {}", colorizer.bold(&s.snapshot_id), msg, s.file_count, format_size_compact(s.added_size), bm);
        }
        return Ok(());
    }
    println!("{} snapshot history:\n", colorizer.bold(&alias));
    for s in &snaps {
        let br = by_snap.get(&s.snapshot_id).cloned().unwrap_or_default();
        let bm = fmt_branches(&br, &cur);
        let marker = if !bm.is_empty() { format!("  {}", bm) } else { String::new() };
        println!("  {}{}", colorizer.bold(&s.snapshot_id), marker);
        println!("  │  {}", s.message.clone().unwrap_or_else(|| "(no message)".into()));
        println!("  │  {} files | {}\n  │", s.file_count, format_size_compact(s.added_size));
    }
    Ok(())
}

fn fmt_branches(branches: &[String], current: &Option<String>) -> String {
    if branches.is_empty() { return String::new(); }
    let parts: Vec<String> = branches.iter().map(|b| if current.as_deref() == Some(b.as_str()) { format!("*{b}") } else { b.clone() }).collect();
    format!("[{}]", parts.join(", "))
}
