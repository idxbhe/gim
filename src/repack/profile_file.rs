//! File-based compression profiles.
//!
//! Profiles are TOML files stored in `[bin_dir]/xtool/profiles/`.
//! Each file defines a complete xtool configuration that can be
//! referenced by name in `gim repack --profile <name>`.
//!
//! Built-in profiles:
//! - `zstd.gimprofile` — uses zstd codec
//! - `lz4.gimprofile` — uses lz4 codec
//! - `oodle.gimprofile` — uses oodle codec
//!
//! Users can create custom profiles by copying a built-in and modifying it.

use crate::error::{GError, GResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Profile file extension.
pub const PROFILE_EXT: &str = ".gimprofile";

/// A compression profile loaded from a TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileFile {
    /// Display name shown in `gim repack --list-profiles`.
    pub name: String,
    /// Short description shown in profile listing.
    pub description: String,

    /// xtool codec string (e.g. "zstd", "zstd+preflate", "lz4+oodle").
    /// Multiple codecs separated by "+".
    /// Available: zlib, zstd, lz4, lzo, oodle, preflate, reflate
    /// oodle sub-codecs: kraken, mermaid, selkie, hydra, leviathan, lzna
    /// Example: "zstd+preflate" or "lz4+kraken"
    pub codecs: String,

    /// Default compression level (1-22).
    /// Passed to xtool as :l<N> suffix on each codec.
    /// Valid ranges per codec:
    ///   zlib:    1-9
    ///   zstd:    1-22
    ///   lz4:     1-12
    ///   oodle:   1-8 (varies by sub-codec)
    ///   preflate: N/A (no level)
    ///   reflate:  N/A (no level)
    /// When multiple codecs are used, the level applies to all that
    /// support it.
    pub level: u32,

    /// Chunk size for xtool -c parameter.
    /// Range: 4mb to 2gb.
    /// Larger = better stream detection, more memory.
    /// Format: "16mb", "64mb", "128mb", "256mb"
    pub chunk_size: String,

    /// Number of threads. "0" = auto (total CPU - 1).
    /// Can be exact number or percentage with "p" suffix.
    /// Examples: "0", "4", "75p", "100p-1"
    pub threads: String,

    /// Memory limit for xtool --mem parameter.
    /// "0" = auto (80% of RAM).
    /// Can be exact or percentage.
    /// Examples: "0", "4096mb", "75p", "75p-600mb"
    pub memory: String,

    /// Precompression depth (xtool -d parameter).
    /// Number of depths to search for streams within streams.
    /// 0 = no depth search (default, fastest)
    /// 1 = search one level deep (catches zip-in-zip)
    /// 2+ = deeper search (very slow, rarely needed)
    pub depth: u32,

    /// Enable stream database (xtool --dbase).
    /// Speeds up processing of repeated streams.
    /// true = enable, false = disable
    pub dbase: bool,

    /// Enable stream deduplication (xtool --dedup parameter).
    /// Deduplicates identical compressed streams found during precompression.
    /// Produces an additional .dedup file that must be present during decode.
    /// Empty string = disabled.
    /// Filename = enabled, e.g. "dedup.bin"
    pub dedup: String,

    /// Delta encoding threshold (xtool --diff parameter).
    /// Controls when streams that can't be perfectly restored are discarded.
    /// "5p" = 5% of stream size (default)
    /// "0" = never discard (keep all, even if imperfect)
    /// "100p" = always discard imperfect streams
    pub diff: String,

    /// Low memory mode (xtool -lm flag).
    /// Reduces memory usage at cost of speed.
    /// true = only one chunk scanned at a time
    /// false = each thread gets its own chunk (default, faster)
    pub low_memory: bool,
}

impl ProfileFile {
    /// Load a profile from a TOML file.
    pub fn load(path: &Path) -> GResult<Self> {
        let contents = std::fs::read_to_string(path)?;
        toml::from_str(&contents).map_err(|e| {
            GError::Other(format!("failed to parse profile {}: {}", path.display(), e))
        })
    }

    /// Load a profile by name from the profiles directory.
    /// Tries `<name>.gimprofile`, then `<name>` as-is.
    pub fn load_by_name(profiles_dir: &Path, name: &str) -> GResult<Self> {
        // Try with extension first.
        let with_ext = profiles_dir.join(format!("{}{}", name, PROFILE_EXT));
        if with_ext.exists() {
            return Self::load(&with_ext);
        }
        // Try as-is (user might have passed full filename).
        let as_is = profiles_dir.join(name);
        if as_is.exists() {
            return Self::load(&as_is);
        }
        Err(GError::Other(format!(
            "profile \"{}\" not found in {}. Available profiles can be listed with: gim repack --list-profiles",
            name,
            profiles_dir.display()
        )))
    }

    /// List all profiles in the profiles directory.
    /// Returns (filename, profile) pairs sorted by name.
    pub fn list_all(profiles_dir: &Path) -> GResult<Vec<(String, Self)>> {
        let mut out = Vec::new();
        if !profiles_dir.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(profiles_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() { continue; }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            // Accept .gimprofile extension or any file.
            if !name.ends_with(PROFILE_EXT) && !name.ends_with(".toml") && !name.ends_with(".cfg") {
                continue;
            }
            match Self::load(&path) {
                Ok(profile) => out.push((name, profile)),
                Err(e) => log::warn!("skipping invalid profile {}: {}", path.display(), e),
            }
        }
        out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        Ok(out)
    }

    /// Ensure the profiles directory exists. Does NOT create any
    /// profile files — profiles are 100% external. The 3 default
    /// profiles (zstd, lz4, oodle) ship as files in the repo at
    /// xtool/profiles/ and should be present alongside the binary.
    pub fn ensure_dir(profiles_dir: &Path) -> GResult<()> {
        std::fs::create_dir_all(profiles_dir)?;
        Ok(())
    }

    /// Build xtool encode (precomp) argument list from this profile.
    /// Override level if `level_override` is Some.
    pub fn xtool_encode_args(&self, level_override: Option<u32>, threads: Option<usize>, memory: Option<u64>) -> Vec<String> {
        let level = level_override.unwrap_or(self.level);
        let threads_str = match threads {
            Some(t) => t.to_string(),
            None => self.threads.clone(),
        };
        let memory_str = match memory {
            Some(m) => format!("{}mb", m),
            None => self.memory.clone(),
        };

        let mut args = vec![
            "precomp".to_string(),
            format!("-m{}:l{}", self.codecs, level),
            format!("-c{}", self.chunk_size),
            format!("-t{}", threads_str),
            format!("--mem={}", memory_str),
        ];

        if self.depth > 0 {
            args.push(format!("-d{}", self.depth));
        }
        if self.dbase {
            args.push("--dbase".to_string());
        }
        if !self.dedup.is_empty() {
            args.push(format!("--dedup={}", self.dedup));
        }
        if !self.diff.is_empty() && self.diff != "5p" {
            args.push(format!("--diff={}", self.diff));
        }
        if self.low_memory {
            args.push("-lm".to_string());
        }

        args
    }

    /// Build xtool decode argument list.
    pub fn xtool_decode_args(&self, threads: Option<usize>) -> Vec<String> {
        let threads_str = match threads {
            Some(t) => t.to_string(),
            None => self.threads.clone(),
        };
        let mut args = vec![
            "decode".to_string(),
            format!("-t{}", threads_str),
        ];
        if !self.dedup.is_empty() {
            args.push(format!("--dedup={}", self.dedup));
        }
        args
    }

    /// Short summary for `--list-profiles` display.
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("codecs: {}", self.codecs),
            format!("level: {}", self.level),
            format!("chunk: {}", self.chunk_size),
        ];
        if self.depth > 0 { parts.push(format!("depth: {}", self.depth)); }
        if self.dbase { parts.push("dbase".to_string()); }
        if !self.dedup.is_empty() { parts.push(format!("dedup: {}", self.dedup)); }
        parts.join(", ")
    }
}
