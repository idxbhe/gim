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

    /// Ensure built-in profiles exist in the profiles directory.
    /// Creates them if missing. Does NOT overwrite existing files.
    pub fn ensure_builtins(profiles_dir: &Path) -> GResult<()> {
        std::fs::create_dir_all(profiles_dir)?;

        let zstd_path = profiles_dir.join(format!("zstd{}", PROFILE_EXT));
        if !zstd_path.exists() {
            std::fs::write(&zstd_path, BUILTIN_ZSTD)?;
        }

        let lz4_path = profiles_dir.join(format!("lz4{}", PROFILE_EXT));
        if !lz4_path.exists() {
            std::fs::write(&lz4_path, BUILTIN_LZ4)?;
        }

        let oodle_path = profiles_dir.join(format!("oodle{}", PROFILE_EXT));
        if !oodle_path.exists() {
            std::fs::write(&oodle_path, BUILTIN_OODLE)?;
        }

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

// ── Built-in profile templates ─────────────────────────────────────

/// Built-in zstd profile.
/// zstd: best general-purpose codec. Good ratio, fast decode.
/// Level 1-22 (higher = better ratio, slower encode).
/// Default level 10 = mid-range, good balance.
const BUILTIN_ZSTD: &str = r#"# ──────────────────────────────────────────────────────────────────────
# gim compression profile: zstd
# ──────────────────────────────────────────────────────────────────────
# zstd is the best general-purpose precompression codec. It offers
# excellent compression ratio with very fast decompression speed.
# Recommended for most games.
#
# This file is also a TEMPLATE for creating custom profiles.
# Copy it, modify values, and save as <name>.gimprofile in this folder.
# Then use: gim repack <game> --profile <name>

name = "zstd"
description = "ZStandard — best general-purpose, good ratio + fast decode"

# ── Codecs ────────────────────────────────────────────────────────────
# Codecs to use for precompression. Multiple codecs separated by "+".
# xtool scans for compressed streams matching these codecs and
# recompresses them so the outer compression works better.
#
# Available codecs:
#   zlib       — Deflate/ZIP streams (.zip, .pak, .upk, .bik, .dds)
#                Level range: 1-9
#   zstd       — ZStandard streams (modern games, Epic/Unreal)
#                Level range: 1-22
#   lz4        — LZ4 streams (some game engines)
#                Level range: 1-12
#   lzo        — LZO streams (rare, legacy)
#                Level range: N/A
#   oodle      — Oodle streams (Unreal Engine, RAD Game Tools)
#                Sub-codecs: kraken, mermaid, selkie, hydra, leviathan, lzna
#                Level range: 1-8 (varies by sub-codec)
#                Usage: "oodle" (all sub-codecs) or "kraken" (specific)
#   preflate   — Advanced deflate scanner (catches what zlib misses)
#                Level range: N/A (no level parameter)
#   reflate    — Re-compresses deflate streams at max level
#                Level range: N/A (no level parameter)
#                WARNING: Slow decode, not recommended for fast unpack
#
# Examples:
#   codecs = "zstd"
#   codecs = "zstd+preflate"
#   codecs = "zstd+preflate+kraken"
#   codecs = "lz4+oodle"
codecs = "zstd+preflate"

# ── Compression Level ─────────────────────────────────────────────────
# Default compression level. Passed to xtool as :l<N> on each codec.
# Higher = better ratio, slower encode. Does NOT affect decode speed.
# Override with: gim repack <game> --level <N>
#
# Level ranges per codec:
#   zstd:  1-22 (default: 10)
#   zlib:  1-9  (default: 5)
#   lz4:   1-12 (default: 6)
#   oodle: 1-8  (default: 4)
#   preflate/reflate: no level (ignored)
#
# When multiple codecs are used, level applies to all that support it.
level = 10

# ── Chunk Size ────────────────────────────────────────────────────────
# Size of data chunks xtool processes at a time.
# Range: 4mb to 2gb
# Larger chunks = better stream detection, but more memory per thread.
# Recommended:
#   16mb  — fast, low memory (good for testing)
#   64mb  — balanced (good for most games)
#   128mb — thorough (large game files)
#   256mb — maximum detection (very high memory)
chunk_size = "64mb"

# ── Threads ───────────────────────────────────────────────────────────
# Number of CPU threads for xtool.
# "0" = auto (total CPU threads - 1, leaves 1 for system)
# Can be exact number or percentage:
#   "4"      — exactly 4 threads
#   "75p"    — 75% of available threads
#   "100p-1" — all threads minus 1
#   "100p-2" — all threads minus 2 (more system headroom)
threads = "0"

# ── Memory ────────────────────────────────────────────────────────────
# Memory limit for xtool deduplication and processing.
# "0" = auto (80% of system RAM)
# Can be exact or percentage:
#   "4096mb"   — 4 GB
#   "8192mb"   — 8 GB
#   "75p"      — 75% of system RAM
#   "75p-600mb" — 75% of RAM minus 600 MB
memory = "0"

# ── Depth ─────────────────────────────────────────────────────────────
# Precompression depth. How deep to search for streams within streams.
# 0 = no depth search (fastest, default)
# 1 = search one level deep (catches zip-in-zip, archive-in-archive)
# 2+ = deeper search (very slow, rarely finds additional streams)
#
# When to use depth > 0:
#   - Game packages that contain nested archives (.pak within .pak)
#   - Compressed files that were re-compressed
#   - Tradeoff: depth 1 roughly doubles repack time
depth = 0

# ── Stream Database ───────────────────────────────────────────────────
# Enable xtool stream database for faster processing of repeated streams.
# xtool caches processed streams and reuses them when the same stream
# is found again. Especially useful for game data with many identical
# textures, sounds, or models across files.
#
# true = enable (recommended for large games)
# false = disable (default, simpler)
#
# Note: there is a small risk of hash collision (xtool docs mention this).
# If you encounter restore issues, try disabling dbase.
dbase = true

# ── Deduplication ─────────────────────────────────────────────────────
# Enable stream deduplication. xtool finds identical compressed streams
# and stores only one copy, replacing others with references.
# Produces an additional .dedup file that must be present during decode.
#
# Empty string = disabled
# Filename = enabled, the dedup database will be written to this file
#
# How it works:
#   1. xtool finds stream A in file1.pak and file2.pak
#   2. Only processes stream A once
#   3. In file2.pak, inserts a reference to the stored stream A
#   4. Result: smaller output, faster decode
#
# The .dedup file is shared across all data processed with the same
# --dedup flag. For gim repack, all objects use the same dedup file.
dedup = "dedup.bin"

# ── Delta Threshold ───────────────────────────────────────────────────
# Controls when streams that cannot be perfectly restored are discarded.
# Some streams can't be reconstructed exactly — xtool uses xdelta to
# store the difference. If the diff is too large, the stream is discarded
# (stored as-is, uncompressed) to avoid negative ratio.
#
# "5p"  = 5% of stream size (xtool default, recommended)
# "0"   = never discard (keep all, even imperfect — may increase size)
# "100p" = always discard imperfect streams
# "10p" = 10% threshold (more strict)
diff = "5p"

# ── Low Memory Mode ───────────────────────────────────────────────────
# Reduces memory usage at the cost of speed.
# false = each thread gets its own chunk (default, faster)
# true = only one chunk scanned at a time (slower, less memory)
#
# When to enable:
#   - System with limited RAM (< 8GB)
#   - Large chunk_size (256mb+) with many threads
#   - Other applications running that need memory
low_memory = false
"#;

/// Built-in lz4 profile.
/// lz4: fastest codec. Lower ratio than zstd, but extremely fast
/// encode and decode. Good for quick repacks or when speed matters
/// more than ratio.
const BUILTIN_LZ4: &str = r#"# ──────────────────────────────────────────────────────────────────────
# gim compression profile: lz4
# ──────────────────────────────────────────────────────────────────────
# lz4 is the fastest precompression codec. Extremely fast encode and
# decode, but lower compression ratio than zstd.
# Good for: quick repacks, testing, or when unpack speed is critical.
#
# See zstd.gimprofile for full documentation of all options.

name = "lz4"
description = "LZ4 — fastest codec, lower ratio, extremely fast unpack"

codecs = "lz4+preflate"

# lz4 level range: 1-12
# 6 = mid-range, good balance of speed and ratio
level = 6

chunk_size = "64mb"
threads = "0"
memory = "0"
depth = 0
dbase = true
dedup = "dedup.bin"
diff = "5p"
low_memory = false
"#;

/// Built-in oodle profile.
/// oodle: codec used by Unreal Engine and many AAA games.
/// Supports sub-codecs (kraken, mermaid, etc.) for specific stream types.
/// Good ratio, fast decode. Best for Unreal Engine games.
const BUILTIN_OODLE: &str = r#"# ──────────────────────────────────────────────────────────────────────
# gim compression profile: oodle
# ──────────────────────────────────────────────────────────────────────
# oodle is the codec used by Unreal Engine and many AAA games.
# Supports sub-codecs for specific stream types:
#   kraken    — most common Oodle format
#   mermaid   — faster than kraken, slightly lower ratio
#   selkie    — fastest oodle, lowest ratio
#   hydra     — combination of kraken + mermaid
#   leviathan — highest ratio oodle, slowest
#   lzna      — legacy oodle format
#
# Best for: Unreal Engine games, Epic Games Store titles.
# See zstd.gimprofile for full documentation of all options.

name = "oodle"
description = "Oodle (kraken+mermaid) — best for Unreal Engine games"

# Use kraken + mermaid to cover most oodle stream variants.
# Add "+zstd" to also catch zstd streams if the game uses mixed codecs.
codecs = "kraken+mermaid+preflate"

# oodle level range: 1-8
# 4 = mid-range (xtool docs recommend l4 for kraken)
level = 4

chunk_size = "64mb"
threads = "0"
memory = "0"
depth = 0
dbase = true
dedup = "dedup.bin"
diff = "5p"
low_memory = false
"#;
