//! `g diff` — compare two snapshots and show file differences.

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::Colorizer;
use crate::output::{format_size, format_size_compact};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize)]
struct DiffEntry {
    kind: String, // "added" | "deleted" | "modified"
    path: String,
    size: Option<i64>,
}

#[derive(Serialize)]
struct DiffJson {
    snapshot_a: String,
    snapshot_b: String,
    added: Vec<DiffEntry>,
    deleted: Vec<DiffEntry>,
    modified: Vec<DiffEntry>,
    net_size: i64,
}

pub fn run(
    colorizer: &Colorizer,
    alias: String,
    snapshot_a: String,
    snapshot_b: String,
    stat: bool,
    json: bool,
) -> GResult<()> {
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
    let _snap_a = snaps_db
        .get_snapshot(&snapshot_a)?
        .ok_or_else(|| GError::SnapshotNotFound(snapshot_a.clone(), alias.clone()))?;
    let _snap_b = snaps_db
        .get_snapshot(&snapshot_b)?
        .ok_or_else(|| GError::SnapshotNotFound(snapshot_b.clone(), alias.clone()))?;

    let map_a: HashMap<String, (String, i64)> = snaps_db
        .files_for_snapshot(&snapshot_a)?
        .into_iter()
        .map(|(p, (h, s))| (p, (h.0, s)))
        .collect();
    let map_b: HashMap<String, (String, i64)> = snaps_db
        .files_for_snapshot(&snapshot_b)?
        .into_iter()
        .map(|(p, (h, s))| (p, (h.0, s)))
        .collect();

    // Compute diff A → B
    let mut added: Vec<(String, i64)> = Vec::new();
    let mut modified: Vec<(String, i64, i64)> = Vec::new();
    let mut deleted: Vec<(String, i64)> = Vec::new();
    let mut net_size: i64 = 0;

    for (path, (hash_b, size_b)) in &map_b {
        match map_a.get(path) {
            None => {
                added.push((path.clone(), *size_b));
                net_size += size_b;
            }
            Some((hash_a, _)) if hash_a != hash_b => {
                // modified — net size is the difference
                let old_size = map_a.get(path).map(|(_, s)| *s).unwrap_or(0);
                modified.push((path.clone(), old_size, *size_b));
                net_size += *size_b - old_size;
            }
            _ => {}
        }
    }
    for (path, (_, size_a)) in &map_a {
        if !map_b.contains_key(path) {
            deleted.push((path.clone(), *size_a));
            net_size -= size_a;
        }
    }

    if json {
        let out = DiffJson {
            snapshot_a: snapshot_a.clone(),
            snapshot_b: snapshot_b.clone(),
            added: added.iter().map(|(p, s)| DiffEntry { kind: "added".into(), path: p.clone(), size: Some(*s) }).collect(),
            deleted: deleted.iter().map(|(p, s)| DiffEntry { kind: "deleted".into(), path: p.clone(), size: Some(*s) }).collect(),
            modified: modified.iter().map(|(p, _, s)| DiffEntry { kind: "modified".into(), path: p.clone(), size: Some(*s) }).collect(),
            net_size,
        };
        let json = serde_json::to_string_pretty(&out)?;
        println!("{json}");
        return Ok(());
    }

    if stat {
        let added_size: i64 = added.iter().map(|(_, s)| *s).sum();
        let modified_size: i64 = modified.iter().map(|(_, _, s)| *s).sum();
        let deleted_size: i64 = deleted.iter().map(|(_, s)| *s).sum();
        println!("  {} → {}", snapshot_a, snapshot_b);
        println!("  {} added    ({})", added.len(), format_size_compact(added_size));
        println!("  {} modified ({})", modified.len(), format_size_compact(modified_size));
        println!("  {} deleted  ({})", deleted.len(), format_size_compact(-deleted_size));
        println!("  ─────────────────────");
        println!("  net:       ({})", format_size_compact(net_size));
        return Ok(());
    }

    // Default: line-per-change
    println!("diff {} → {}", snapshot_a, snapshot_b);
    println!();
    for (p, s) in &added {
        println!(" {}     {} (+{})", colorizer.green("added"), p, format_size(*s));
    }
    for (p, _old, _new) in &modified {
        println!(" {}  {}", colorizer.yellow("modified"), p);
    }
    for (p, s) in &deleted {
        println!(" {}   {} (-{})", colorizer.red("deleted"), p, format_size(*s));
    }
    println!();
    println!(
        "{} added, {} modified, {} deleted | {} net",
        added.len(),
        modified.len(),
        deleted.len(),
        format_size_compact(net_size)
    );
    Ok(())
}
