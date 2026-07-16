//! File-based compression profiles — full xtool parameter access.
//!
//! Profiles are TOML files in `[bin_dir]/xtool/profiles/`.
//! Each file defines a complete xtool precomp configuration with
//! access to ALL xtool parameters.
//!
//! Profile structure has two layers:
//! - Layer 1: Precompression (scan + inflate streams)
//! - Layer 2: Output compression (built-in LZMA2 or external compressor)

use crate::error::{GError, GResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const PROFILE_EXT: &str = ".gimprofile";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileFile {
    pub name: String,
    pub description: String,

    // ── Layer 1: Precompression ───────────────────────────────────
    /// Codecs for stream scanning. Separated by "+".
    /// Available: zlib, zstd, lz4, lz4hc, lzo, kraken, mermaid, selkie,
    ///            hydra, preflate, reflate, grittibanzli
    /// Example: "zstd+preflate" or "kraken+mermaid+preflate"
    pub codecs: String,

    /// Codec level for stream scanning. Appended as :l<N> to codecs.
    /// Ranges: zlib 1-9, zstd 1-22, lz4 1-12, oodle 1-8
    pub codec_level: u32,

    /// Chunk size for scanning. Range: 4mb-2gb.
    pub chunk_size: String,

    /// Scan depth. 0=none, 1-10=stream-in-stream search depth.
    pub depth: u32,

    /// Stream database for faster processing of repeated streams.
    pub dbase: bool,

    /// Stream deduplication filename. Empty = disabled.
    pub dedup: String,

    /// Dedup memory limit. "0"=auto, or "4096mb", "75p", "75p-600mb".
    pub dedup_memory: String,

    /// Delta threshold for imperfect stream restoration.
    /// "5p"=5% (default), "0"=never discard, "100p"=always discard.
    pub diff: String,

    /// Low memory mode. One chunk at a time (slower, less memory).
    pub low_memory: bool,

    /// Skip stream verification (faster, risky).
    pub skip_verify: bool,

    /// Full scan mode (more thorough).
    pub full_scan: bool,

    /// Optimize output for faster decode.
    pub optimize_decode: bool,

    /// Prefetch cache size. "0mb"=disabled.
    pub prefetch_cache: String,

    /// Recompress streams with another codec. Empty = disabled.
    pub recompress: String,

    /// Reassign detected streams to another codec. Empty = disabled.
    pub reassign: String,

    /// Extract streams to directory (debugging). Empty = disabled.
    pub extract_dir: String,

    // ── Layer 2: Output Compression ───────────────────────────────
    /// Built-in fast LZMA2 compression level.
    /// 0 = disabled (precompression only, output may be larger than input)
    /// 1-10 = LZMA2 compression level
    pub compression_level: u32,

    /// LZMA2 dictionary size. Empty = default per level.
    /// Example: "128mb"
    pub compression_dict: String,

    /// LZMA2 high compression mode (better ratio, slower encode).
    pub compression_high: bool,

    /// External compressor program. Empty = disabled.
    /// If set, overrides built-in LZMA2.
    /// Use {stdin} and {stdout} as pipe placeholders.
    /// Example: "7z a -txz -mx=9 {stdin} {stdout}"
    pub external_compressor: String,

    // ── Resource Management ───────────────────────────────────────
    /// Thread count. "0"=auto (CPU-1), or "4", "75p", "100p-1".
    pub threads: String,

    /// Thread priority: "idle", "normal", "high", "timecritical".
    pub thread_priority: String,

    // ── Custom Libraries ──────────────────────────────────────────
    /// Custom LZ4 library filename. Empty = default.
    pub lz4_library: String,
    /// Custom ZSTD library filename. Empty = default.
    pub zstd_library: String,
    /// Custom Oodle library filename. Empty = auto-detect.
    pub oodle_library: String,

    // ── Debug ────────────────────────────────────────────────────
    /// Verbose xtool output.
    pub verbose: bool,
}

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
            if !name.ends_with(PROFILE_EXT) && !name.ends_with(".toml") && !name.ends_with(".cfg") { continue; }
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

    /// Build xtool precomp argument list from this profile.
    /// `level_override` overrides `codec_level` (from --level CLI flag).
    /// `threads_override` overrides `threads` (from --threads CLI flag).
    pub fn xtool_encode_args(
        &self,
        level_override: Option<u32>,
        threads_override: Option<usize>,
    ) -> Vec<String> {
        let codec_level = level_override.unwrap_or(self.codec_level);
        let threads_str = match threads_override {
            Some(t) => t.to_string(),
            None => self.threads.clone(),
        };

        let mut args: Vec<String> = Vec::new();

        // ── Layer 1: Precompression ───────────────────────────────
        args.push("precomp".to_string());

        // Codecs with level (-m)
        if codec_level > 0 {
            args.push(format!("-m{}:l{}", self.codecs, codec_level));
        } else {
            args.push(format!("-m{}", self.codecs));
        }

        // Chunk size (-c)
        args.push(format!("-c{}", self.chunk_size));

        // Threads (-t)
        args.push(format!("-t{}", threads_str));

        // Depth (-d)
        if self.depth > 0 {
            args.push(format!("-d{}", self.depth));
        }

        // Stream database (--dbase)
        if self.dbase {
            args.push("--dbase".to_string());
        }

        // Deduplication (--dedup)
        if !self.dedup.is_empty() {
            args.push(format!("--dedup={}", self.dedup));
        }

        // Dedup memory (--mem)
        if !self.dedup_memory.is_empty() && self.dedup_memory != "0" {
            args.push(format!("--mem={}", self.dedup_memory));
        }

        // Diff threshold (--diff)
        if !self.diff.is_empty() && self.diff != "5p" {
            args.push(format!("--diff={}", self.diff));
        }

        // Low memory mode (-lm) — note: this is -l with value "m" in xtool
        if self.low_memory {
            args.push("-lm".to_string());
        }

        // Skip verification (-s)
        if self.skip_verify {
            args.push("-s".to_string());
        }

        // Full scan (-f)
        if self.full_scan {
            args.push("-f".to_string());
        }

        // Optimize decode (-o)
        if self.optimize_decode {
            args.push("-o".to_string());
        }

        // Prefetch cache (-p)
        if !self.prefetch_cache.is_empty() && self.prefetch_cache != "0mb" {
            args.push(format!("-p{}", self.prefetch_cache));
        }

        // Recompress (-r)
        if !self.recompress.is_empty() {
            args.push(format!("-r{}", self.recompress));
        }

        // Reassign (-a)
        if !self.reassign.is_empty() {
            args.push(format!("-a{}", self.reassign));
        }

        // Extract to directory (-x)
        if !self.extract_dir.is_empty() {
            args.push(format!("-x{}", self.extract_dir));
        }

        // Custom libraries
        if !self.lz4_library.is_empty() {
            args.push(format!("-lz4{}", self.lz4_library));
        }
        if !self.zstd_library.is_empty() {
            args.push(format!("-zstd{}", self.zstd_library));
        }
        if !self.oodle_library.is_empty() {
            args.push(format!("-oodle{}", self.oodle_library));
        }

        // ── Layer 2: Output Compression ──────────────────────────

        // Built-in LZMA2 (-l) — only if no external compressor
        if self.external_compressor.is_empty() && self.compression_level > 0 {
            let mut l_val = self.compression_level.to_string();
            if self.compression_high {
                l_val.push('x');
            }
            if !self.compression_dict.is_empty() {
                l_val.push_str(&format!(":d{}", self.compression_dict));
            }
            args.push(format!("-l{}", l_val));
        }

        // External compressor (-e)
        if !self.external_compressor.is_empty() {
            args.push(format!("-e{}", self.external_compressor));
        }

        // Thread priority (-T)
        if !self.thread_priority.is_empty() && self.thread_priority != "normal" {
            args.push(format!("-T{}", self.thread_priority));
        }

        // Verbose (-v)
        if self.verbose {
            args.push("-v".to_string());
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
            None => self.threads.clone(),
        };
        let mut args = vec![
            "decode".to_string(),
            format!("-t{}", threads_str),
        ];
        if !self.dedup.is_empty() {
            args.push(format!("--dedup={}", self.dedup));
        }
        if !self.dedup_memory.is_empty() && self.dedup_memory != "0" {
            args.push(format!("--mem={}", self.dedup_memory));
        }
        args
    }

    /// Short summary for --list-profiles.
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("codecs: {}", self.codecs),
            format!("level: {}", self.codec_level),
        ];
        if self.compression_level > 0 {
            parts.push(format!("lzma2: l{}", self.compression_level));
        }
        if !self.external_compressor.is_empty() {
            parts.push("external".to_string());
        }
        if self.depth > 0 { parts.push(format!("depth: {}", self.depth)); }
        if self.dbase { parts.push("dbase".to_string()); }
        if !self.dedup.is_empty() { parts.push("dedup".to_string()); }
        parts.join(", ")
    }
}
