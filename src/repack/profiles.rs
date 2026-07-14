//! Compression profiles — preset configurations for repack.
//!
//! Profiles map to xtool codec combinations and chunk sizes. The user
//! can also override the level (1-10).

use crate::error::{GError, GResult};
use serde::{Deserialize, Serialize};

/// Preset compression profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionProfile {
    /// Fast compression, fast decompression, moderate ratio.
    /// Codecs: zlib, chunk: 16mb, default level: 2
    Fast,
    /// Balanced — good ratio, good speed.
    /// Codecs: zlib+preflate, chunk: 64mb, default level: 5
    Balanced,
    /// Maximum compression, slower.
    /// Codecs: zlib+preflate+reflate, chunk: 256mb, default level: 9
    Max,
}

impl CompressionProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressionProfile::Fast => "fast",
            CompressionProfile::Balanced => "balanced",
            CompressionProfile::Max => "max",
        }
    }

    /// Default level for this profile (1-10).
    pub fn default_level(&self) -> u32 {
        match self {
            CompressionProfile::Fast => 2,
            CompressionProfile::Balanced => 5,
            CompressionProfile::Max => 9,
        }
    }

    /// xtool codec string (e.g. "zlib+preflate").
    pub fn codecs(&self) -> Vec<String> {
        match self {
            CompressionProfile::Fast => vec!["zlib".to_string()],
            CompressionProfile::Balanced => vec!["zlib".to_string(), "preflate".to_string()],
            CompressionProfile::Max => vec!["zlib".to_string(), "preflate".to_string(), "reflate".to_string()],
        }
    }

    /// xtool codec string for -m parameter (e.g. "zlib+preflate").
    pub fn codec_string(&self) -> String {
        self.codecs().join("+")
    }

    /// Chunk size for xtool -c parameter.
    pub fn chunk_size(&self) -> &'static str {
        match self {
            CompressionProfile::Fast => "16mb",
            CompressionProfile::Balanced => "64mb",
            CompressionProfile::Max => "256mb",
        }
    }

    pub fn parse(s: &str) -> GResult<Self> {
        match s.to_lowercase().as_str() {
            "fast" => Ok(CompressionProfile::Fast),
            "balanced" | "default" => Ok(CompressionProfile::Balanced),
            "max" => Ok(CompressionProfile::Max),
            other => Err(GError::Other(format!(
                "unknown compression profile \"{other}\" (supported: fast, balanced, max)"
            ))),
        }
    }
}

impl std::fmt::Display for CompressionProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Full compression configuration — profile + overrides.
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    pub profile: CompressionProfile,
    pub level: u32,
    pub threads: usize,
    pub memory_mb: u64,
}

impl CompressionConfig {
    /// Create from profile, with defaults for threads/memory.
    /// - threads: total CPU threads - 1
    /// - memory: 80% of RAM + pagefile
    pub fn new(profile: CompressionProfile, level: Option<u32>) -> Self {
        let sys = sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::everything().with_memory(sysinfo::MemoryRefreshKind::everything()),
        );
        let total_threads = sys.cpus().len();
        let threads = total_threads.saturating_sub(1).max(1);

        let total_mem = sys.total_memory();
        let available_mem = sys.available_memory();
        // Use 80% of (total RAM + pagefile). sysinfo reports total_memory
        // as RAM only. We estimate: 80% of total RAM, leave 20% for OS.
        // Pagefile is not directly available via sysinfo, so we use
        // total RAM as base.
        let memory_mb = (total_mem * 80 / 100) / (1024 * 1024);

        Self {
            profile,
            level: level.unwrap_or_else(|| profile.default_level()),
            threads,
            memory_mb,
        }
    }

    /// Build xtool argument string for this config.
    /// Example: "precomp -mzlib+preflate -c64mb -t7 --mem=8192mb"
    pub fn xtool_encode_args(&self) -> Vec<String> {
        let mut args = vec![
            "precomp".to_string(),
            format!("-m{}", self.profile.codec_string()),
            format!("-c{}", self.profile.chunk_size()),
            format!("-t{}", self.threads),
            format!("--mem={}mb", self.memory_mb),
        ];
        // Level is codec-specific. For zlib, higher = more compression.
        // xtool passes level via codec syntax: -mzlib:l9
        // We rebuild the -m arg with level.
        if !args.is_empty() {
            args[1] = format!("-m{}:l{}", self.profile.codec_string(), self.level);
        }
        args
    }

    /// Build xtool decode argument string.
    pub fn xtool_decode_args(&self) -> Vec<String> {
        vec![
            "decode".to_string(),
            format!("-t{}", self.threads),
        ]
    }
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self::new(CompressionProfile::Balanced, None)
    }
}
