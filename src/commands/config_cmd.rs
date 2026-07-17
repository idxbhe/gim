//! `gim config` — get/set/list configuration.
//!
//! Without an alias, operates on the global config (`data/config`).
//! With an alias, operates on the per-game config (`data/[alias]/config`).
//!
//! When `hash.algorithm` is changed on a game that already has
//! snapshots, the user is prompted to confirm a full rehash. Use
//! `--yes` to skip the prompt.

use crate::config::{env_data_dir_override, GimConfig, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::hashing::HashAlgorithm;
use crate::locking;
use crate::output::Colorizer;
use crate::output::ProgressReporter;
use crate::storage::Cas;
use std::io::{self, BufRead, Write};

pub fn run(
    c: &Colorizer,
    alias: Option<String>,
    get: Option<String>,
    set: Option<Vec<String>>,
    unset: Option<String>,
    _list: bool,
    yes: bool,
    progress: &ProgressReporter,
) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;

    // ── --get ───────────────────────────────────────────────────────
    if let Some(key) = get {
        let cfg = load_config(&paths, alias.as_deref())?;
        println!("{}", cfg.get(&key));
        return Ok(());
    }

    // ── --set ───────────────────────────────────────────────────────
    if let Some(pair) = set {
        if pair.len() != 2 {
            return Err(GError::Other("--set requires KEY VALUE".into()));
        }
        let key = &pair[0];
        let value = &pair[1];

        // Validate key and value.
        crate::config::gim_config::validate_key(key)?;
        crate::config::gim_config::validate_value(key, value)?;

        // Special handling for hash.algorithm on a game with snapshots.
        if key == "hash.algorithm" {
            if let Some(ref alias) = alias {
                let games_db = GamesDb::open(&paths.games_db)?;
                if games_db.get(alias)?.is_some() {
                    let sdb = SnapsDb::open(&paths.snaps_db(alias))?;
                    let has_snaps = sdb.list_snapshots()?.len() > 0;
                    if has_snaps {
                        let old_algo = load_config(&paths, Some(alias))?.get("hash.algorithm");
                        let new_algo = value;
                        // Confirm rehash.
                        if !yes {
                            println!("warning: changing hash.algorithm from \"{old_algo}\" to \"{new_algo}\"");
                            println!("  this will REHASH all existing snapshots for game \"{alias}\"");
                            println!("  the game directory will be re-walked and all files re-hashed");
                            println!("  this may take a long time for large games");
                            println!();
                            print!("proceed? [y/N] ");
                            io::stdout().flush()?;
                            let mut input = String::new();
                            io::stdin().lock().read_line(&mut input)?;
                            if !input.trim().eq_ignore_ascii_case("y") {
                                println!("rehash cancelled");
                                return Err(GError::RehashCancelled);
                            }
                        }
                        // Perform rehash.
                        let new_algorithm: HashAlgorithm = value.parse()?;
                        rehash_game(&paths, alias, new_algorithm, progress)?;
                    }
                }
            }
        }

        // Set and save.
        let mut cfg = load_config(&paths, alias.as_deref())?;
        cfg.set(key, value);
        cfg.save()?;

        let scope = if alias.is_some() { format!("game \"{}\"", alias.unwrap()) } else { "global".to_string() };
        println!("set {key}={value} ({scope})");
        return Ok(());
    }

    // ── --unset ─────────────────────────────────────────────────────
    if let Some(key) = unset {
        let mut cfg = load_config(&paths, alias.as_deref())?;
        cfg.unset(&key);
        cfg.save()?;
        let scope = if alias.is_some() { format!("game \"{}\"", alias.as_ref().unwrap()) } else { "global".to_string() };
        println!("unset {key} ({scope})");
        return Ok(());
    }

    // ── --list (default) ────────────────────────────────────────────
    let cfg = load_config(&paths, alias.as_deref())?;
    let scope = if alias.is_some() { format!("game \"{}\"", alias.as_ref().unwrap()) } else { "global".to_string() };
    println!("config ({scope}):\n");
    for (key, val, is_default) in cfg.list_all() {
        let marker = if is_default { c.dim("(default)") } else { String::new() };
        let desc = config_value_description(&key, &val);
        println!("  {key} = {val} {marker}");
        println!("    {}", c.dim(&desc));
    }
    Ok(())
}

/// Load global or per-game config.
fn load_config(paths: &Paths, alias: Option<&str>) -> GResult<GimConfig> {
    match alias {
        Some(a) => GimConfig::load_game(paths, a),
        None => GimConfig::load_global(paths),
    }
}

/// Rehash all snapshots for a game with a new hash algorithm.
///
/// Strategy:
/// 1. Query all distinct hashes from the DB.
/// 2. For each hash: read object from CAS, rehash with new algorithm,
///    store to CAS under new hash.
/// 3. Update all `files.hash` rows in DB with new hashes.
/// 4. Delete old objects from CAS (will be cleaned by gc).
/// 5. Update per-game config.
fn rehash_game(
    paths: &Paths,
    alias: &str,
    new_algo: HashAlgorithm,
    progress: &ProgressReporter,
) -> GResult<()> {
    let sdb_path = paths.snaps_db(alias);
    let _lock = locking::acquire_game_lock(alias, &sdb_path)?;
    let mut sdb = SnapsDb::open(&sdb_path)?;
    let cas = Cas::new(paths.objects_dir(alias));
    cas.ensure()?;

    // Get all distinct hashes.
    let old_hashes: Vec<String> = sdb.referenced_hashes()?.into_iter().collect();
    if old_hashes.is_empty() {
        // No snapshots — nothing to rehash.
        return Ok(());
    }

    progress.scan_start();
    for _h in &old_hashes { progress.scan_tick(); }
    progress.scan_done(old_hashes.len() as u64);

    // Build hash mapping: old_hash → new_hash.
    progress.hash_start(old_hashes.len());
    let mut hash_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for old_hash in &old_hashes {
        // Read object from CAS.
        let old_h = crate::hashing::Hash(old_hash.clone());
        match cas.open(&old_h) {
            Ok(mut file) => {
                use std::io::Read;
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)?;
                // Rehash with new algorithm.
                let new_hash = crate::hashing::hash_bytes(&buf, new_algo);
                // Store to CAS under new hash.
                if !cas.exists(&new_hash) {
                    // Write to a temp file then store.
                    let tmp = std::env::temp_dir().join(format!("gim-rehash-{}-{}", std::process::id(), old_hash));
                    std::fs::write(&tmp, &buf)?;
                    cas.store_from(&tmp, &new_hash)?;
                    let _ = std::fs::remove_file(&tmp);
                }
                hash_map.insert(old_hash.clone(), new_hash.0);
            }
            Err(e) => {
                eprintln!("warning: could not read object {old_hash}: {e}");
            }
        }
        progress.hash_tick();
    }
    let hash_count = hash_map.len() as u64;
    progress.hash_done(hash_count);

    // Update DB: replace all old hashes with new hashes.
    progress.store_start(hash_map.len());
    let tx = sdb.transaction()?;
    for (old_hash, new_hash) in &hash_map {
        tx.execute(
            "UPDATE files SET hash = ?1 WHERE hash = ?2",
            rusqlite::params![new_hash, old_hash],
        )?;
        progress.store_tick();
    }
    tx.commit()?;
    progress.store_done(hash_map.len() as u64);

    // Update per-game config.
    let mut cfg = GimConfig::load_game(paths, alias)?;
    cfg.set("hash.algorithm", new_algo.as_str());
    cfg.save()?;

    println!("rehashed {} objects with {}", hash_map.len(), new_algo);
    println!("run `gim gc {alias}` to clean up old objects");
    Ok(())
}

/// Return a human-readable description of valid values for a config key.
fn config_value_description(key: &str, _current_val: &str) -> String {
    match key {
        "hash.algorithm" => "options: xxhash (fast, non-crypto) | blake3 (crypto, slower)".to_string(),
        "hash.threads" => "0 = auto (use all CPUs) | N = exact thread count".to_string(),
        "hash.parallel" => "true = parallel hashing (SSD) | false = sequential (HDD)".to_string(),
        "snapshot.auto_gc" => "true = auto gc after snap | false = manual gc only".to_string(),
        "snapshot.lock_retry" => "number of retries on locked files (0 = no retry)".to_string(),
        "compact.algorithm" => "options: lzx (best ratio) | xpress4k | xpress8k | xpress16k | ntfs (LZNT1) | none".to_string(),
        "compact.threads" => "0 = auto (use all CPUs) | N = exact thread count".to_string(),
        "compact.auto_pause" => "true = pause background compact when a tracked game is running | false = never pause".to_string(),
        _ => String::new(),
    }
}
