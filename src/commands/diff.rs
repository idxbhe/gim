use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::Colorizer;
use crate::output::{format_size, format_size_compact};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize)] struct De { kind: String, path: String, size: Option<i64> }
#[derive(Serialize)] struct Dj { snapshot_a: String, snapshot_b: String, added: Vec<De>, deleted: Vec<De>, modified: Vec<De>, net_size: i64 }

pub fn run(c: &Colorizer, alias: String, a: String, b: String, stat: bool, json: bool) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let sdb = SnapsDb::open(&paths.snaps_db(&alias))?;
    sdb.get_snapshot(&a)?.ok_or_else(|| GError::SnapshotNotFound(a.clone(), alias.clone()))?;
    sdb.get_snapshot(&b)?.ok_or_else(|| GError::SnapshotNotFound(b.clone(), alias.clone()))?;
    let ma: HashMap<String, (String, i64)> = sdb.files_for_snapshot(&a)?.into_iter().map(|(p, m)| (p, (m.hash.0, m.file_size))).collect();
    let mb: HashMap<String, (String, i64)> = sdb.files_for_snapshot(&b)?.into_iter().map(|(p, m)| (p, (m.hash.0, m.file_size))).collect();
    let mut added = Vec::new(); let mut modified = Vec::new(); let mut deleted = Vec::new(); let mut net: i64 = 0;
    for (p, (hb, sb)) in &mb { match ma.get(p) { None => { added.push((p.clone(), *sb)); net += sb; } Some((ha, _)) if ha != hb => { let old = ma.get(p).map(|(_, s)| *s).unwrap_or(0); modified.push((p.clone(), old, *sb)); net += *sb - old; } _ => {} } }
    for (p, (_, sa)) in &ma { if !mb.contains_key(p) { deleted.push((p.clone(), *sa)); net -= sa; } }
    if json {
        let out = Dj { snapshot_a: a.clone(), snapshot_b: b.clone(), added: added.iter().map(|(p, s)| De { kind: "added".into(), path: p.clone(), size: Some(*s) }).collect(), deleted: deleted.iter().map(|(p, s)| De { kind: "deleted".into(), path: p.clone(), size: Some(*s) }).collect(), modified: modified.iter().map(|(p, _, s)| De { kind: "modified".into(), path: p.clone(), size: Some(*s) }).collect(), net_size: net };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if stat {
        let asz: i64 = added.iter().map(|(_, s)| *s).sum(); let msz: i64 = modified.iter().map(|(_, _, s)| *s).sum(); let dsz: i64 = deleted.iter().map(|(_, s)| *s).sum();
        println!("  {} → {}\n  {} added    ({})\n  {} modified ({})\n  {} deleted  ({})\n  ─────────────────────\n  net:       ({})", a, b, added.len(), format_size_compact(asz), modified.len(), format_size_compact(msz), deleted.len(), format_size_compact(-dsz), format_size_compact(net));
        return Ok(());
    }
    println!("diff {} → {}\n", a, b);
    for (p, s) in &added { println!(" {}     {} (+{})", c.green("added"), p, format_size(*s)); }
    for (p, _, _) in &modified { println!(" {}  {}", c.yellow("modified"), p); }
    for (p, s) in &deleted { println!(" {}   {} (-{})", c.red("deleted"), p, format_size(*s)); }
    println!("\n{} added, {} modified, {} deleted | {} net", added.len(), modified.len(), deleted.len(), format_size_compact(net));
    Ok(())
}
