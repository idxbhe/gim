//! `gim diff`

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::Colorizer;
use crate::output::{format_size, format_size_compact};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize)]
struct DiffEntry { kind: String, path: String, size: Option<i64> }
#[derive(Serialize)]
struct DiffJson { snapshot_a: String, snapshot_b: String, added: Vec<DiffEntry>, deleted: Vec<DiffEntry>, modified: Vec<DiffEntry>, net_size: i64 }

pub fn run(colorizer: &Colorizer, alias: String, a: String, b: String, stat: bool, json: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let games_db = GamesDb::open(&paths.games_db)?;
    games_db.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let snaps_db = SnapsDb::open(&paths.snaps_db(&alias))?;
    snaps_db.get_snapshot(&a)?.ok_or_else(|| GError::SnapshotNotFound(a.clone(), alias.clone()))?;
    snaps_db.get_snapshot(&b)?.ok_or_else(|| GError::SnapshotNotFound(b.clone(), alias.clone()))?;
    let map_a: HashMap<String, (String, i64)> = snaps_db.files_for_snapshot(&a)?.into_iter().map(|(p, m)| (p, (m.hash.0, m.file_size))).collect();
    let map_b: HashMap<String, (String, i64)> = snaps_db.files_for_snapshot(&b)?.into_iter().map(|(p, m)| (p, (m.hash.0, m.file_size))).collect();

    let mut added = Vec::new(); let mut modified = Vec::new(); let mut deleted = Vec::new(); let mut net: i64 = 0;
    for (p, (hb, sb)) in &map_b {
        match map_a.get(p) {
            None => { added.push((p.clone(), *sb)); net += sb; }
            Some((ha, _)) if ha != hb => { let old = map_a.get(p).map(|(_, s)| *s).unwrap_or(0); modified.push((p.clone(), old, *sb)); net += *sb - old; }
            _ => {}
        }
    }
    for (p, (_, sa)) in &map_a { if !map_b.contains_key(p) { deleted.push((p.clone(), *sa)); net -= sa; } }

    if json {
        let out = DiffJson { snapshot_a: a.clone(), snapshot_b: b.clone(), added: added.iter().map(|(p, s)| DiffEntry { kind: "added".into(), path: p.clone(), size: Some(*s) }).collect(), deleted: deleted.iter().map(|(p, s)| DiffEntry { kind: "deleted".into(), path: p.clone(), size: Some(*s) }).collect(), modified: modified.iter().map(|(p, _, s)| DiffEntry { kind: "modified".into(), path: p.clone(), size: Some(*s) }).collect(), net_size: net };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if stat {
        let asz: i64 = added.iter().map(|(_, s)| *s).sum();
        let msz: i64 = modified.iter().map(|(_, _, s)| *s).sum();
        let dsz: i64 = deleted.iter().map(|(_, s)| *s).sum();
        println!("  {} → {}", a, b);
        println!("  {} added    ({})", added.len(), format_size_compact(asz));
        println!("  {} modified ({})", modified.len(), format_size_compact(msz));
        println!("  {} deleted  ({})", deleted.len(), format_size_compact(-dsz));
        println!("  ─────────────────────");
        println!("  net:       ({})", format_size_compact(net));
        return Ok(());
    }
    println!("diff {} → {}\n", a, b);
    for (p, s) in &added { println!(" {}     {} (+{})", colorizer.green("added"), p, format_size(*s)); }
    for (p, _, _) in &modified { println!(" {}  {}", colorizer.yellow("modified"), p); }
    for (p, s) in &deleted { println!(" {}   {} (-{})", colorizer.red("deleted"), p, format_size(*s)); }
    println!("\n{} added, {} modified, {} deleted | {} net", added.len(), modified.len(), deleted.len(), format_size_compact(net));
    Ok(())
}
