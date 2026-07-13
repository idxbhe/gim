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
//! - `snapshot.auto_gc` — `false` (default) | `true`
//! - `snapshot.lock_retry` — `3` (default)
//!
//! When `hash.algorithm` is changed on a game that already has
//! snapshots, the user is prompted to confirm a full rehash.

use crate::config::Paths;
use crate::error::{GError, GResult};
use crate::hashing::HashAlgorithm;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// All supported config keys with their defaults.
pub const DEFAULTS: &[(&str, &str)] = &[
    ("hash.algorithm", "xxhash"),
    ("hash.threads", "0"),
    ("snapshot.auto_gc", "false"),
    ("snapshot.lock_retry", "3"),
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

    /// Lock retry count.
    pub fn lock_retry(&self) -> u32 {
        self.get_parsed("snapshot.lock_retry").unwrap_or(3)
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
    }

    #[test]
    fn override_works() {
        let mut entries = HashMap::new();
        entries.insert("hash.algorithm".to_string(), "blake3".to_string());
        let cfg = GimConfig { entries, source_path: PathBuf::new() };
        assert_eq!(cfg.get("hash.algorithm"), "blake3");
        assert_eq!(cfg.hash_algorithm().unwrap(), HashAlgorithm::Blake3);
    }

    #[test]
    fn validate_known_keys() {
        assert!(validate_key("hash.algorithm").is_ok());
        assert!(validate_key("hash.threads").is_ok());
        assert!(validate_key("snapshot.auto_gc").is_ok());
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
    }
}
