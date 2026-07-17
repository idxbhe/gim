//! Compression algorithms for `gim compact`.
//!
//! Windows exposes two distinct on-disk compression mechanisms:
//!
//! - **NTFS compression** (`FSCTL_SET_COMPRESSION` — same as `compact.exe /C`).
//!   Supports only **LZNT1**. Transparent, lives in the file's data stream.
//!
//! - **WOF — Windows Overlay Filter** (`FSCTL_SET_EXTERNAL_BACKING` — same as
//!   `compact.exe /EXE`). Supports **XPRESS4K**, **XPRESS8K**, **XPRESS16K**,
//!   and **LZX** (the best-ratio algorithm and the project default). The file
//!   stays readable transparently; compressed chunks live in a backing
//!   data stream managed by the WOF driver.
//!
//! See <https://learn.microsoft.com/en-us/windows/win32/api/wofapi/ns-wofapi-wof_file_compression_info_v1>
//! for the algorithm constants.
//!
//! On non-Windows platforms, every operation returns an error — these APIs
//! are Windows-only filesystem features.

use crate::error::{GError, GResult};
use std::str::FromStr;

/// Which Windows compression mechanism backs an algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactMode {
    /// NTFS live compression — `FSCTL_SET_COMPRESSION`. Only `lznt1`.
    Ntfs,
    /// Windows Overlay Filter — `FSCTL_SET_EXTERNAL_BACKING`. LZX/XPRESS.
    Wof,
}

/// A concrete compression algorithm.
///
/// The order also defines the canonical short name used in config (`as_str`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactAlgorithm {
    /// NTFS LZNT1 (`compact.exe /C`).
    Ntfs,
    /// WOF XPRESS4K — fastest, lowest ratio (`compact.exe /EXE:XPRESS4K`).
    Xpress4k,
    /// WOF LZX — best ratio, the project default (`compact.exe /EXE:LZX`).
    Lzx,
    /// WOF XPRESS8K — balanced.
    Xpress8k,
    /// WOF XPRESS16K — higher ratio than XPRESS8K.
    Xpress16k,
    /// Decompress / remove any existing compression.
    None,
}

impl Default for CompactAlgorithm {
    fn default() -> Self {
        Self::Lzx
    }
}

impl FromStr for CompactAlgorithm {
    type Err = GError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl CompactAlgorithm {
    /// The mechanism this algorithm runs through.
    pub fn mode(&self) -> CompactMode {
        match self {
            Self::Ntfs => CompactMode::Ntfs,
            Self::Xpress4k | Self::Lzx | Self::Xpress8k | Self::Xpress16k | Self::None => {
                CompactMode::Wof
            }
        }
    }

    /// Canonical short name used in config & CLI (`lzx`, `ntfs`, ...).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ntfs => "ntfs",
            Self::Xpress4k => "xpress4k",
            Self::Lzx => "lzx",
            Self::Xpress8k => "xpress8k",
            Self::Xpress16k => "xpress16k",
            Self::None => "none",
        }
    }

    /// Human-readable label for summaries / help text.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ntfs => "NTFS LZNT1",
            Self::Xpress4k => "WOF XPRESS4K",
            Self::Lzx => "WOF LZX",
            Self::Xpress8k => "WOF XPRESS8K",
            Self::Xpress16k => "WOF XPRESS16K",
            Self::None => "none (decompress)",
        }
    }

    /// All accepted short names, in help-display order. Used by `--help` and
    /// config validation error messages.
    pub fn all_strs() -> &'static [&'static str] {
        &["lzx", "xpress4k", "xpress8k", "xpress16k", "ntfs", "none"]
    }

    /// Parse a short name into an algorithm (case-insensitive).
    pub fn parse(s: &str) -> GResult<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "lzx" => Ok(Self::Lzx),
            "xpress4k" | "xpress4K" => Ok(Self::Xpress4k),
            "xpress8k" | "xpress8K" => Ok(Self::Xpress8k),
            "xpress16k" | "xpress16K" => Ok(Self::Xpress16k),
            "ntfs" | "lznt1" => Ok(Self::Ntfs),
            "none" | "off" | "decompress" => Ok(Self::None),
            other => Err(GError::Other(format!(
                "unknown compact algorithm \"{other}\" (supported: {})",
                Self::all_strs().join(", ")
            ))),
        }
    }

    /// `true` for variants that decompress rather than compress.
    pub fn is_decompress(&self) -> bool {
        matches!(self, Self::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip() {
        for &name in CompactAlgorithm::all_strs() {
            let algo = CompactAlgorithm::parse(name).unwrap();
            assert_eq!(algo.as_str(), name);
        }
    }

    #[test]
    fn parse_case_insensitive() {
        assert_eq!(CompactAlgorithm::parse("LZX").unwrap(), CompactAlgorithm::Lzx);
        assert_eq!(CompactAlgorithm::parse("  Lzx ").unwrap(), CompactAlgorithm::Lzx);
        assert_eq!(CompactAlgorithm::parse("NTFS").unwrap(), CompactAlgorithm::Ntfs);
    }

    #[test]
    fn aliases() {
        assert_eq!(CompactAlgorithm::parse("lznt1").unwrap(), CompactAlgorithm::Ntfs);
        assert_eq!(CompactAlgorithm::parse("off").unwrap(), CompactAlgorithm::None);
        assert_eq!(CompactAlgorithm::parse("decompress").unwrap(), CompactAlgorithm::None);
    }

    #[test]
    fn parse_invalid() {
        assert!(CompactAlgorithm::parse("gzip").is_err());
        assert!(CompactAlgorithm::parse("").is_err());
    }

    #[test]
    fn default_is_lzx() {
        assert_eq!(CompactAlgorithm::default(), CompactAlgorithm::Lzx);
    }

    #[test]
    fn mode_mapping() {
        assert_eq!(CompactAlgorithm::Ntfs.mode(), CompactMode::Ntfs);
        assert_eq!(CompactAlgorithm::Lzx.mode(), CompactMode::Wof);
        assert_eq!(CompactAlgorithm::Xpress4k.mode(), CompactMode::Wof);
        assert_eq!(CompactAlgorithm::None.mode(), CompactMode::Wof);
    }
}
