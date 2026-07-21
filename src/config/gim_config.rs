//! Gim configuration — git-like global + per-game config.
//!
//! Config files are simple `key=value` text files:
//! - **Global**: `data/config` — defaults for new games
//! - **Per-game**: `data/[alias]/config` — overrides global (set at
//!   `gim add` time, independent of global afterwards)
//!
//! Supported keys:
//! - `hash.algorithm` — `xxhash` (default) | `blake3`
//! - `hash.threads` — `0` (auto, default) | N
//! - `hash.parallel` — `true` (default) | `false`
//! - `snapshot.auto_gc` — `false` (default) | `true`
//! - `snapshot.lock_retry` — `3` (default)
//! - `compact.algorithm` — `lzx` (default) | `xpress4k` | `xpress8k` | `xpress16k` | `ntfs` | `none`
//! - `compact.threads` — `0` (auto, default) | N
//! - `compact.auto_pause` — `true` (default) | `false`
//! - `defrag.min_free_pct` — `15` (default) — refuse to run below this free space
//! - `defrag.fragment_threshold_pct` — `5` (default) — skip files below this fragmentation
//! - `defrag.max_extents` — `20` (default) — NTFS attribute-list safety cap
//! - `defrag.throttle_mb` — `500` (default) — sleep after this much I/O
//! - `defrag.throttle_sleep_ms` — `200` (default) — sleep duration
//! - `defrag.consolidate` — `true` (default) | `false`
//!
//! When `hash.algorithm` is changed on a game that already has
//! snapshots, the user is prompted to confirm a full rehash.

use crate::config::Paths;
use crate::error::{GError, GResult};
use crate::hashing::HashAlgorithm;
use crate::compact::CompactAlgorithm;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// All supported config keys with their defaults.
pub const DEFAULTS: &[(&str, &str)] = &[
    ("hash.algorithm", "xxhash"),
    ("hash.threads", "0"),
    ("hash.parallel", "true"),
    ("snapshot.auto_gc", "false"),
    ("snapshot.lock_retry", "3"),
    ("compact.algorithm", "lzx"),
    ("compact.threads", "0"),
    ("compact.auto_pause", "true"),
    ("defrag.min_free_pct", "15"),
    ("defrag.fragment_threshold_pct", "5"),
    ("defrag.max_extents", "20"),
    ("defrag.throttle_mb", "500"),
    ("defrag.throttle_sleep_ms", "200"),
    ("defrag.consolidate", "true"),
];

/// A config store — holds key/value pairs parsed from a config file.
#[derive(Debug, Clone)]
pub struct GimConfig {
    pub entries: HashMap<String, String>,
    pub source_path: PathBuf,
}

impl GimConfig {
    /// Load a config file. If it doesn't exist, returns an empty config
    /// (defaults are applied on get).
    pub fn load(path: &Path) -> GResult<Self> {
        let mut entries = HashMap::new();
        if path.exists() {
            let contents = std::fs::read_to_string(path)?;
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') { continue; }
                if let Some((key, val)) = line.split_once('=') {
                    entries.insert(key.trim().to_string(), val.trim().to_string());
                }
            }
        }
        Ok(Self { entries, source_path: path.to_path_buf() })
    }

    /// Load the global config (`data/config`).
    pub fn load_global(paths: &Paths) -> GResult<Self> {
        Self::load(&paths.data_dir.join("config"))
    }

    /// Load a per-game config (`data/[alias]/config`).
    pub fn load_game(paths: &Paths, alias: &str) -> GResult<Self> {
        Self::load(&paths.game_data_dir(alias).join("config"))
    }

    /// Get a value, falling back to defaults if not set.
    pub fn get(&self, key: &str) -> String {
        if let Some(v) = self.entries.get(key) {
            return v.clone();
        }
        for (k, v) in DEFAULTS {
            if *k == key { return v.to_string(); }
        }
        String::new()
    }

    /// Get a value as a parsed type.
    pub fn get_parsed<T: FromStr>(&self, key: &str) -> Option<T> {
        self.get(key).parse().ok()
    }

    /// Get the hash algorithm for this config.
    pub fn hash_algorithm(&self) -> GResult<HashAlgorithm> {
        self.get("hash.algorithm").parse()
    }

    /// Get the thread count (0 = auto).
    pub fn hash_threads(&self) -> usize {
        self.get_parsed("hash.threads").unwrap_or(0)
    }

    /// Whether to auto-gc after snap.
    pub fn auto_gc(&self) -> bool {
        self.get("snapshot.auto_gc") == "true"
    }

    /// Whether to hash files in parallel (true) or sequentially (false).
    /// Sequential is better for HDDs (avoids disk thrashing).
    pub fn hash_parallel(&self) -> bool {
        self.get("hash.parallel") == "true"
    }

    /// Lock retry count.
    pub fn lock_retry(&self) -> u32 {
        self.get_parsed("snapshot.lock_retry").unwrap_or(3)
    }

    /// Get the compact algorithm for this config.
    pub fn compact_algorithm(&self) -> GResult<CompactAlgorithm> {
        self.get("compact.algorithm").parse()
    }

    /// Get the compact thread count (0 = auto).
    pub fn compact_threads(&self) -> usize {
        self.get_parsed("compact.threads").unwrap_or(0)
    }

    /// Whether to auto-pause background compaction when a tracked game is running.
    pub fn compact_auto_pause(&self) -> bool {
        self.get("compact.auto_pause") == "true"
    }

    // ── defrag.* accessors ─────────────────────────────────────────

    /// Minimum free space percentage required for defrag (default 15).
    pub fn defrag_min_free_pct(&self) -> u8 {
        self.get_parsed::<u8>("defrag.min_free_pct").unwrap_or(15)
    }

    /// Fragmentation threshold below which files are skipped (default 5%).
    pub fn defrag_fragment_threshold_pct(&self) -> u8 {
        self.get_parsed::<u8>("defrag.fragment_threshold_pct").unwrap_or(5)
    }

    /// Max extents per file before we give up (default 20, NTFS attribute
    /// list safety).
    pub fn defrag_max_extents(&self) -> u32 {
        self.get_parsed::<u32>("defrag.max_extents").unwrap_or(20)
    }

    /// I/O throttle budget in MB (default 500).
    pub fn defrag_throttle_mb(&self) -> u64 {
        self.get_parsed::<u64>("defrag.throttle_mb").unwrap_or(500)
    }

    /// I/O throttle sleep duration in ms (default 200).
    pub fn defrag_throttle_sleep_ms(&self) -> u64 {
        self.get_parsed::<u64>("defrag.throttle_sleep_ms").unwrap_or(200)
    }

    /// Whether to run the consolidation phase (default true).
    pub fn defrag_consolidate(&self) -> bool {
        self.get("defrag.consolidate") == "true"
    }

    /// Set a key/value. Does NOT write to disk — call `save()` for that.
    pub fn set(&mut self, key: &str, value: &str) {
        self.entries.insert(key.to_string(), value.to_string());
    }

    /// Remove a key (fall back to default).
    pub fn unset(&mut self, key: &str) {
        self.entries.remove(key);
    }

    /// Save config to its source file.
    pub fn save(&self) -> GResult<()> {
        if let Some(parent) = self.source_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut contents = String::new();
        // Write in a stable order: sort by key.
        let mut keys: Vec<&String> = self.entries.keys().collect();
        keys.sort();
        for key in keys {
            let val = &self.entries[key];
            contents.push_str(&format!("{key}={val}\n"));
        }
        std::fs::write(&self.source_path, contents)?;
        Ok(())
    }

    /// Create a per-game config by copying the global config's
    /// hash.algorithm (and other relevant keys) at `gim add` time.
    pub fn create_from_global(paths: &Paths, alias: &str) -> GResult<Self> {
        let global = Self::load_global(paths)?;
        let mut game = Self::load_game(paths, alias)?;
        // Copy all keys from global that aren't already set in game.
        for (key, val) in &global.entries {
            if !game.entries.contains_key(key) {
                game.entries.insert(key.clone(), val.clone());
            }
        }
        // Ensure hash.algorithm is set (from global or default).
        if !game.entries.contains_key("hash.algorithm") {
            game.entries.insert(
                "hash.algorithm".to_string(),
                global.get("hash.algorithm"),
            );
        }
        game.save()?;
        Ok(game)
    }

    /// List all keys with their effective values (including defaults).
    pub fn list_all(&self) -> Vec<(String, String, bool)> {
        let mut out = Vec::new();
        for (key, default) in DEFAULTS {
            let (val, is_default) = if let Some(v) = self.entries.get(*key) {
                (v.clone(), false)
            } else {
                (default.to_string(), true)
            };
            out.push((key.to_string(), val, is_default));
        }
        // Also include any custom keys not in DEFAULTS.
        for (key, val) in &self.entries {
            if !DEFAULTS.iter().any(|(k, _)| k == key) {
                out.push((key.clone(), val.clone(), false));
            }
        }
        out
    }
}

/// Validate that a config key is known.
pub fn validate_key(key: &str) -> GResult<()> {
    if DEFAULTS.iter().any(|(k, _)| *k == key) {
        Ok(())
    } else {
        Err(GError::Other(format!(
            "unknown config key \"{key}\" (supported: {})",
            DEFAULTS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
        )))
    }
}

/// Validate that a value is acceptable for a given key.
pub fn validate_value(key: &str, value: &str) -> GResult<()> {
    match key {
        "hash.algorithm" => {
            value.parse::<HashAlgorithm>()?;
            Ok(())
        }
        "hash.threads" => {
            value.parse::<usize>().map_err(|_| GError::Other(format!(
                "invalid hash.threads value \"{value}\" (expected non-negative integer)"
            )))?;
            Ok(())
        }
        "hash.parallel" => {
            if value != "true" && value != "false" {
                return Err(GError::Other(format!(
                    "invalid hash.parallel value \"{value}\" (expected true|false)"
                )));
            }
            Ok(())
        }
        "snapshot.auto_gc" => {
            if value != "true" && value != "false" {
                return Err(GError::Other(format!(
                    "invalid snapshot.auto_gc value \"{value}\" (expected true|false)"
                )));
            }
            Ok(())
        }
        "snapshot.lock_retry" => {
            value.parse::<u32>().map_err(|_| GError::Other(format!(
                "invalid snapshot.lock_retry value \"{value}\" (expected non-negative integer)"
            )))?;
            Ok(())
        }
        "compact.algorithm" => {
            value.parse::<CompactAlgorithm>()?;
            Ok(())
        }
        "compact.threads" => {
            value.parse::<usize>().map_err(|_| GError::Other(format!(
                "invalid compact.threads value \"{value}\" (expected non-negative integer)"
            )))?;
            Ok(())
        }
        "compact.auto_pause" => {
            if value != "true" && value != "false" {
                return Err(GError::Other(format!(
                    "invalid compact.auto_pause value \"{value}\" (expected true|false)"
                )));
            }
            Ok(())
        }
        "defrag.min_free_pct" => {
            let n: u8 = value.parse().map_err(|_| GError::Other(format!(
                "invalid defrag.min_free_pct value \"{value}\" (expected 0-100)")))?;
            if n > 100 {
                return Err(GError::Other(format!(
                    "defrag.min_free_pct {n} out of range (0-100)")));
            }
            Ok(())
        }
        "defrag.fragment_threshold_pct" => {
            let n: u8 = value.parse().map_err(|_| GError::Other(format!(
                "invalid defrag.fragment_threshold_pct value \"{value}\" (expected 0-100)")))?;
            if n > 100 {
                return Err(GError::Other(format!(
                    "defrag.fragment_threshold_pct {n} out of range (0-100)")));
            }
            Ok(())
        }
        "defrag.max_extents" => {
            let n: u32 = value.parse().map_err(|_| GError::Other(format!(
                "invalid defrag.max_extents value \"{value}\" (expected non-negative integer)")))?;
            // NTFS attribute-list hard limit is ~30; we cap at 30 to
            // leave headroom for MFT metadata growth.
            if n == 0 || n > 30 {
                return Err(GError::Other(format!(
                    "defrag.max_extents {n} out of range (1-30)")));
            }
            Ok(())
        }
        "defrag.throttle_mb" => {
            value.parse::<u64>().map_err(|_| GError::Other(format!(
                "invalid defrag.throttle_mb value \"{value}\" (expected non-negative integer)")))?;
            Ok(())
        }
        "defrag.throttle_sleep_ms" => {
            value.parse::<u64>().map_err(|_| GError::Other(format!(
                "invalid defrag.throttle_sleep_ms value \"{value}\" (expected non-negative integer)")))?;
            Ok(())
        }
        "defrag.consolidate" => {
            if value != "true" && value != "false" {
                return Err(GError::Other(format!(
                    "invalid defrag.consolidate value \"{value}\" (expected true|false)")));
            }
            Ok(())
        }
        _ => Ok(()), // unknown keys pass through
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_applied() {
        let cfg = GimConfig { entries: HashMap::new(), source_path: PathBuf::new() };
        assert_eq!(cfg.get("hash.algorithm"), "xxhash");
        assert_eq!(cfg.get("hash.threads"), "0");
        assert_eq!(cfg.get("snapshot.auto_gc"), "false");
        assert_eq!(cfg.get("snapshot.lock_retry"), "3");
        assert_eq!(cfg.get("compact.algorithm"), "lzx");
        assert_eq!(cfg.get("compact.threads"), "0");
        assert_eq!(cfg.get("compact.auto_pause"), "true");
        assert_eq!(cfg.get("defrag.min_free_pct"), "15");
        assert_eq!(cfg.get("defrag.fragment_threshold_pct"), "5");
        assert_eq!(cfg.get("defrag.max_extents"), "20");
        assert_eq!(cfg.get("defrag.throttle_mb"), "500");
        assert_eq!(cfg.get("defrag.throttle_sleep_ms"), "200");
        assert_eq!(cfg.get("defrag.consolidate"), "true");
    }

    #[test]
    fn override_works() {
        let mut entries = HashMap::new();
        entries.insert("hash.algorithm".to_string(), "blake3".to_string());
        entries.insert("compact.algorithm".to_string(), "ntfs".to_string());
        let cfg = GimConfig { entries, source_path: PathBuf::new() };
        assert_eq!(cfg.get("hash.algorithm"), "blake3");
        assert_eq!(cfg.hash_algorithm().unwrap(), HashAlgorithm::Blake3);
        assert_eq!(cfg.get("compact.algorithm"), "ntfs");
        assert_eq!(cfg.compact_algorithm().unwrap(), CompactAlgorithm::Ntfs);
    }

    #[test]
    fn validate_known_keys() {
        assert!(validate_key("hash.algorithm").is_ok());
        assert!(validate_key("hash.threads").is_ok());
        assert!(validate_key("snapshot.auto_gc").is_ok());
        assert!(validate_key("compact.algorithm").is_ok());
        assert!(validate_key("compact.threads").is_ok());
        assert!(validate_key("compact.auto_pause").is_ok());
        assert!(validate_key("defrag.min_free_pct").is_ok());
        assert!(validate_key("defrag.fragment_threshold_pct").is_ok());
        assert!(validate_key("defrag.max_extents").is_ok());
        assert!(validate_key("defrag.throttle_mb").is_ok());
        assert!(validate_key("defrag.throttle_sleep_ms").is_ok());
        assert!(validate_key("defrag.consolidate").is_ok());
        assert!(validate_key("unknown.key").is_err());
    }

    #[test]
    fn validate_values() {
        assert!(validate_value("hash.algorithm", "xxhash").is_ok());
        assert!(validate_value("hash.algorithm", "blake3").is_ok());
        assert!(validate_value("hash.algorithm", "invalid").is_err());
        assert!(validate_value("hash.threads", "4").is_ok());
        assert!(validate_value("hash.threads", "abc").is_err());
        assert!(validate_value("snapshot.auto_gc", "true").is_ok());
        assert!(validate_value("snapshot.auto_gc", "maybe").is_err());
        assert!(validate_value("compact.algorithm", "lzx").is_ok());
        assert!(validate_value("compact.algorithm", "ntfs").is_ok());
        assert!(validate_value("compact.algorithm", "gzip").is_err());
        assert!(validate_value("compact.threads", "4").is_ok());
        assert!(validate_value("compact.threads", "abc").is_err());
        assert!(validate_value("compact.auto_pause", "true").is_ok());
        assert!(validate_value("compact.auto_pause", "maybe").is_err());
        // defrag.* keys
        assert!(validate_value("defrag.min_free_pct", "15").is_ok());
        assert!(validate_value("defrag.min_free_pct", "0").is_ok());
        assert!(validate_value("defrag.min_free_pct", "100").is_ok());
        assert!(validate_value("defrag.min_free_pct", "101").is_err());
        assert!(validate_value("defrag.min_free_pct", "abc").is_err());
        assert!(validate_value("defrag.fragment_threshold_pct", "5").is_ok());
        assert!(validate_value("defrag.fragment_threshold_pct", "200").is_err());
        assert!(validate_value("defrag.max_extents", "20").is_ok());
        assert!(validate_value("defrag.max_extents", "1").is_ok());
        assert!(validate_value("defrag.max_extents", "0").is_err());
        assert!(validate_value("defrag.max_extents", "31").is_err());
        assert!(validate_value("defrag.throttle_mb", "500").is_ok());
        assert!(validate_value("defrag.throttle_mb", "abc").is_err());
        assert!(validate_value("defrag.throttle_sleep_ms", "200").is_ok());
        assert!(validate_value("defrag.throttle_sleep_ms", "-1").is_err());
        assert!(validate_value("defrag.consolidate", "true").is_ok());
        assert!(validate_value("defrag.consolidate", "false").is_ok());
        assert!(validate_value("defrag.consolidate", "maybe").is_err());
    }

    #[test]
    fn defrag_accessors() {
        let cfg = GimConfig { entries: HashMap::new(), source_path: PathBuf::new() };
        assert_eq!(cfg.defrag_min_free_pct(), 15);
        assert_eq!(cfg.defrag_fragment_threshold_pct(), 5);
        assert_eq!(cfg.defrag_max_extents(), 20);
        assert_eq!(cfg.defrag_throttle_mb(), 500);
        assert_eq!(cfg.defrag_throttle_sleep_ms(), 200);
        assert!(cfg.defrag_consolidate());
    }
}
