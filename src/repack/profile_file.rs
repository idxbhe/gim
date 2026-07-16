//! Compression profile files — TOML format with two sections.
//!
//! ```toml
//! [precomp]
//! # xtool precompression settings (Layer 1)
//!
//! [compress]
//! # Main compression settings (Layer 2)
//! ```
//!
//! Files stored in `[bin_dir]/xtool/profiles/*.toml`

use crate::error::{GError, GResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const PROFILE_EXT: &str = ".toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileFile {
    pub name: String,
    pub description: String,

    #[serde(default)]
    pub precomp: PrecompConfig,

    #[serde(default)]
    pub compress: CompressConfig,
}

/// Layer 1: xtool precompression configuration.
/// Simple — gim auto-detects best codec combination by default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecompConfig {
    /// xtool codecs. "auto" = use all available codecs.
    /// Or specify: "zstd+zlib+kraken+mermaid+preflate"
    #[serde(default = "default_codecs")]
    pub codecs: String,

    /// Chunk size for xtool scanning.
    #[serde(default = "default_chunk_size")]
    pub chunk_size: String,

    /// Scan depth. 0=fast, 1=catch nested archives.
    #[serde(default)]
    pub depth: u32,

    /// Enable stream database for faster processing.
    #[serde(default = "default_true")]
    pub dbase: bool,

    /// Enable stream deduplication.
    #[serde(default = "default_true")]
    pub dedup: bool,

    /// Low memory mode (slower, less RAM).
    #[serde(default)]
    pub low_memory: bool,

    /// Skip stream verification (faster, risky).
    #[serde(default)]
    pub skip_verify: bool,

    /// Threads. "0" = auto (CPU-1).
    #[serde(default = "default_threads")]
    pub threads: String,
}

impl Default for PrecompConfig {
    fn default() -> Self {
        Self {
            codecs: default_codecs(),
            chunk_size: default_chunk_size(),
            depth: 0,
            dbase: true,
            dedup: true,
            low_memory: false,
            skip_verify: false,
            threads: default_threads(),
        }
    }
}

/// Layer 2: Main compression configuration.
/// Determines final output size and unpack speed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressConfig {
    /// Algorithm: "zstd", "lzma", "lz4"
    #[serde(default = "default_algorithm")]
    pub algorithm: String,

    /// Compression level.
    /// zstd: 1-22, lzma: 1-9, lz4: 1-12
    #[serde(default = "default_level")]
    pub level: u32,

    /// Threads. "0" = auto (CPU-1).
    #[serde(default = "default_threads")]
    pub threads: String,
}

impl Default for CompressConfig {
    fn default() -> Self {
        Self {
            algorithm: default_algorithm(),
            level: default_level(),
            threads: default_threads(),
        }
    }
}

// Default value functions
fn default_codecs() -> String { "auto".to_string() }
fn default_chunk_size() -> String { "64mb".to_string() }
fn default_true() -> bool { true }
fn default_threads() -> String { "0".to_string() }
fn default_algorithm() -> String { "zstd".to_string() }
fn default_level() -> u32 { 19 }

impl ProfileFile {
    pub fn load(path: &Path) -> GResult<Self> {
        let contents = std::fs::read_to_string(path)?;
        toml::from_str(&contents).map_err(|e| {
            GError::Other(format!("failed to parse profile {}: {}", path.display(), e))
        })
    }

    pub fn load_by_name(profiles_dir: &Path, name: &str) -> GResult<Self> {
        let with_ext = profiles_dir.join(format!("{}{}", name, PROFILE_EXT));
        if with_ext.exists() { return Self::load(&with_ext); }
        let as_is = profiles_dir.join(name);
        if as_is.exists() { return Self::load(&as_is); }
        Err(GError::Other(format!(
            "profile \"{}\" not found in {}. Use: gim repack --list-profiles",
            name, profiles_dir.display()
        )))
    }

    pub fn list_all(profiles_dir: &Path) -> GResult<Vec<(String, Self)>> {
        let mut out = Vec::new();
        if !profiles_dir.exists() { return Ok(out); }
        for entry in std::fs::read_dir(profiles_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() { continue; }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(), None => continue,
            };
            if !name.ends_with(PROFILE_EXT) { continue; }
            match Self::load(&path) {
                Ok(profile) => out.push((name, profile)),
                Err(e) => log::warn!("skipping invalid profile {}: {}", path.display(), e),
            }
        }
        out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        Ok(out)
    }

    pub fn ensure_dir(profiles_dir: &Path) -> GResult<()> {
        std::fs::create_dir_all(profiles_dir)?;
        Ok(())
    }

    /// Build xtool precomp argument list.
    /// "auto" codecs expands to all available scanners.
    pub fn xtool_encode_args(&self, threads_override: Option<usize>) -> Vec<String> {
        let threads_str = match threads_override {
            Some(t) => t.to_string(),
            None => self.precomp.threads.clone(),
        };

        // "auto" = all scanner codecs
        let codecs = if self.precomp.codecs == "auto" {
            "zstd+zlib+lz4+kraken+mermaid+preflate"
        } else {
            &self.precomp.codecs
        };

        let mut args = vec![
            "precomp".to_string(),
            format!("-m{}", codecs),
            format!("-c{}", self.precomp.chunk_size),
            format!("-t{}", threads_str),
        ];

        if self.precomp.depth > 0 {
            args.push(format!("-d{}", self.precomp.depth));
        }
        if self.precomp.dbase {
            args.push("--dbase".to_string());
        }
        if self.precomp.dedup {
            args.push("--dedup=dedup.bin".to_string());
        }
        if self.precomp.low_memory {
            args.push("-lm".to_string());
        }
        if self.precomp.skip_verify {
            args.push("-s".to_string());
        }

        // stdin/stdout
        args.push("-".to_string());
        args.push("-".to_string());

        args
    }

    /// Build xtool decode argument list.
    pub fn xtool_decode_args(&self, threads_override: Option<usize>) -> Vec<String> {
        let threads_str = match threads_override {
            Some(t) => t.to_string(),
            None => self.precomp.threads.clone(),
        };
        let mut args = vec![
            "decode".to_string(),
            format!("-t{}", threads_str),
        ];
        if self.precomp.dedup {
            args.push("--dedup=dedup.bin".to_string());
        }
        args
    }

    /// Short summary for --list-profiles.
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("compress: {}@{}", self.compress.algorithm, self.compress.level),
        ];
        if self.precomp.codecs == "auto" {
            parts.push("precomp: auto".to_string());
        } else {
            parts.push(format!("precomp: {}", self.precomp.codecs));
        }
        if self.precomp.dedup { parts.push("dedup".to_string()); }
        parts.join(", ")
    }
}
