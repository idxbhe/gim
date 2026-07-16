//! Layer 2 compression — Rust-native, standalone.
//!
//! Compresses/decompresses the output of xtool precompression.
//! No external binary required.
//!
//! Algorithms:
//! - `zstd`  — ZStandard (best ratio+speed balance, recommended)
//! - `lzma`  — LZMA2/XZ (best ratio, slower)
//! - `lz4`   — LZ4 (fastest, lower ratio)
//!
//! Algorithm + level stored in manifest so unpack doesn't need profile.

use crate::error::{GError, GResult};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressAlgorithm {
    Zstd,
    Lzma,
    Lz4,
}

impl CompressAlgorithm {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressAlgorithm::Zstd => "zstd",
            CompressAlgorithm::Lzma => "lzma",
            CompressAlgorithm::Lz4 => "lz4",
        }
    }

    pub fn parse(s: &str) -> GResult<Self> {
        match s.to_lowercase().as_str() {
            "zstd" => Ok(CompressAlgorithm::Zstd),
            "lzma" | "xz" | "lzma2" => Ok(CompressAlgorithm::Lzma),
            "lz4" => Ok(CompressAlgorithm::Lz4),
            other => Err(GError::Other(format!(
                "unknown compression algorithm \"{other}\" (supported: zstd, lzma, lz4)"
            ))),
        }
    }

    pub fn max_level(&self) -> u32 {
        match self {
            CompressAlgorithm::Zstd => 22,
            CompressAlgorithm::Lzma => 9,
            CompressAlgorithm::Lz4 => 12,
        }
    }

    pub fn default_level(&self) -> u32 {
        match self {
            CompressAlgorithm::Zstd => 19,
            CompressAlgorithm::Lzma => 6,
            CompressAlgorithm::Lz4 => 6,
        }
    }

    /// Validate level or clamp to valid range.
    pub fn validate_level_or_default(&self, level: u32) -> u32 {
        let max = self.max_level();
        if level == 0 || level > max {
            self.default_level()
        } else {
            level
        }
    }
}

impl std::fmt::Display for CompressAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compress file. Returns compressed size.
pub fn compress_file(
    input: &Path,
    output: &Path,
    algorithm: CompressAlgorithm,
    level: u32,
) -> GResult<u64> {
    let data = std::fs::read(input)?;
    let compressed = match algorithm {
        CompressAlgorithm::Zstd => {
            zstd::encode_all(&data[..], level as i32)
                .map_err(|e| GError::Other(format!("zstd compress: {e}")))?
        }
        CompressAlgorithm::Lzma => {
            let mut encoder = xz2::write::XzEncoder::new(Vec::new(), level as u32);
            encoder.write_all(&data)
                .map_err(|e| GError::Other(format!("lzma write: {e}")))?;
            encoder.finish()
                .map_err(|e| GError::Other(format!("lzma finish: {e}")))?
        }
        CompressAlgorithm::Lz4 => {
            // Store original size as 8-byte prefix for decompression.
            let orig_size = data.len() as u64;
            let mut out = Vec::with_capacity(8 + data.len());
            out.extend_from_slice(&orig_size.to_le_bytes());
            let compressed = lz4_flex::compress_prepend_size(&data);
            out.extend_from_slice(&compressed);
            out
        }
    };
    std::fs::write(output, &compressed)?;
    Ok(compressed.len() as u64)
}

/// Decompress file. Returns decompressed size.
pub fn decompress_file(
    input: &Path,
    output: &Path,
    algorithm: CompressAlgorithm,
) -> GResult<u64> {
    let data = std::fs::read(input)?;
    let decompressed = match algorithm {
        CompressAlgorithm::Zstd => {
            zstd::decode_all(&data[..])
                .map_err(|e| GError::Other(format!("zstd decompress: {e}")))?
        }
        CompressAlgorithm::Lzma => {
            let mut decoder = xz2::read::XzDecoder::new(&data[..]);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out)
                .map_err(|e| GError::Other(format!("lzma decompress: {e}")))?;
            out
        }
        CompressAlgorithm::Lz4 => {
            if data.len() < 8 {
                return Err(GError::Other("lz4 decompress: data too short".into()));
            }
            let compressed = &data[8..];
            // lz4_flex compress_prepend_size stores size as 4-byte LE prefix.
            lz4_flex::decompress_size_prepended(compressed)
                .map_err(|e| GError::Other(format!("lz4 decompress: {e}")))?
        }
    };
    std::fs::write(output, &decompressed)?;
    Ok(decompressed.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zstd_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("in.bin");
        let comp = tmp.path().join("comp.bin");
        let decomp = tmp.path().join("decomp.bin");
        let data = b"Hello World! zstd roundtrip test data.".repeat(100);
        std::fs::write(&input, &data).unwrap();
        compress_file(&input, &comp, CompressAlgorithm::Zstd, 19).unwrap();
        decompress_file(&comp, &decomp, CompressAlgorithm::Zstd).unwrap();
        assert_eq!(std::fs::read(&decomp).unwrap(), data);
    }

    #[test]
    fn lzma_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("in.bin");
        let comp = tmp.path().join("comp.bin");
        let decomp = tmp.path().join("decomp.bin");
        let data = b"Hello World! lzma roundtrip test data.".repeat(100);
        std::fs::write(&input, &data).unwrap();
        compress_file(&input, &comp, CompressAlgorithm::Lzma, 6).unwrap();
        decompress_file(&comp, &decomp, CompressAlgorithm::Lzma).unwrap();
        assert_eq!(std::fs::read(&decomp).unwrap(), data);
    }

    #[test]
    fn lz4_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("in.bin");
        let comp = tmp.path().join("comp.bin");
        let decomp = tmp.path().join("decomp.bin");
        let data = b"Hello World! lz4 roundtrip test data.".repeat(100);
        std::fs::write(&input, &data).unwrap();
        compress_file(&input, &comp, CompressAlgorithm::Lz4, 6).unwrap();
        decompress_file(&comp, &decomp, CompressAlgorithm::Lz4).unwrap();
        assert_eq!(std::fs::read(&decomp).unwrap(), data);
    }

    #[test]
    fn parse_algorithms() {
        assert_eq!(CompressAlgorithm::parse("zstd").unwrap(), CompressAlgorithm::Zstd);
        assert_eq!(CompressAlgorithm::parse("lzma").unwrap(), CompressAlgorithm::Lzma);
        assert_eq!(CompressAlgorithm::parse("lz4").unwrap(), CompressAlgorithm::Lz4);
        assert!(CompressAlgorithm::parse("invalid").is_err());
    }
}
