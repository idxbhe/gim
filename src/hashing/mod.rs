//! Hashing module — supports XXH3-128 and BLAKE3.
//!
//! `HashAlgorithm` enum dispatches to the correct hasher. The algorithm
//! is chosen per-game via config (`hash.algorithm`), stored in
//! `data/[alias]/config`. One game always uses one algorithm — if the
//! user changes it, all existing snapshots are rehashed.
//!
//! - **XXH3-128** (`xxhash`): 32-char hex, non-cryptographic, extremely
//!   fast (multi-GB/s). Default.
//! - **BLAKE3** (`blake3`): 64-char hex, cryptographic, fast for a
//!   crypto hash (multi-GB/s with SIMD). Use when integrity verification
//!   against adversarial tampering is needed.

use crate::error::{GError, GResult};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::str::FromStr;
use xxhash_rust::xxh3::Xxh3;

const READ_BUF: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    Xxhash,
    Blake3,
}

impl HashAlgorithm {
    pub fn as_str(&self) -> &'static str {
        match self {
            HashAlgorithm::Xxhash => "xxhash",
            HashAlgorithm::Blake3 => "blake3",
        }
    }

    /// Hex digest length for this algorithm.
    pub fn hex_len(&self) -> usize {
        match self {
            HashAlgorithm::Xxhash => 32,  // 128-bit = 16 bytes = 32 hex chars
            HashAlgorithm::Blake3 => 64,  // 256-bit = 32 bytes = 64 hex chars
        }
    }
}

impl FromStr for HashAlgorithm {
    type Err = GError;
    fn from_str(s: &str) -> GResult<Self> {
        match s.trim().to_lowercase().as_str() {
            "xxhash" | "xxh3" | "xxh3-128" => Ok(HashAlgorithm::Xxhash),
            "blake3" => Ok(HashAlgorithm::Blake3),
            other => Err(GError::Other(format!(
                "unknown hash algorithm \"{other}\" (supported: xxhash, blake3)"
            ))),
        }
    }
}

impl std::fmt::Display for HashAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hash(pub String);

impl Hash {
    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}

impl From<String> for Hash {
    fn from(s: String) -> Self { Self(s) }
}

/// Hash an in-memory byte slice with the given algorithm.
pub fn hash_bytes(data: &[u8], algo: HashAlgorithm) -> Hash {
    match algo {
        HashAlgorithm::Xxhash => {
            use xxhash_rust::xxh3::xxh3_128;
            Hash(format!("{:032x}", xxh3_128(data)))
        }
        HashAlgorithm::Blake3 => {
            Hash(blake3::hash(data).to_hex().to_string())
        }
    }
}

/// Hash a file on disk by streaming it through a buffer.
pub fn hash_file(path: &Path, algo: HashAlgorithm) -> GResult<(Hash, u64)> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(READ_BUF, file);
    let mut total: u64 = 0;
    let mut buf = vec![0u8; READ_BUF];

    let hash = match algo {
        HashAlgorithm::Xxhash => {
            let mut hasher = Xxh3::new();
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 { break; }
                hasher.update(&buf[..n]);
                total += n as u64;
            }
            Hash(format!("{:032x}", hasher.digest128()))
        }
        HashAlgorithm::Blake3 => {
            let mut hasher = blake3::Hasher::new();
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 { break; }
                hasher.update(&buf[..n]);
                total += n as u64;
            }
            Hash(hasher.finalize().to_hex().to_string())
        }
    };

    Ok((hash, total))
}

/// Try to open and hash a file, retrying on lock errors.
pub fn hash_file_with_retry(
    path: &Path,
    algo: HashAlgorithm,
    max_retries: u32,
    delay: std::time::Duration,
) -> GResult<Option<(Hash, u64)>> {
    for attempt in 0..=max_retries {
        match hash_file(path, algo) {
            Ok(v) => return Ok(Some(v)),
            Err(GError::Io(e)) => {
                let retryable = matches!(e.kind(),
                    std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::ResourceBusy
                ) || e.raw_os_error().is_some_and(|c| c == 32 || c == 33);
                if !retryable || attempt == max_retries { break; }
                std::thread::sleep(delay);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xxhash_deterministic() {
        let a = hash_bytes(b"hello", HashAlgorithm::Xxhash);
        let b = hash_bytes(b"hello", HashAlgorithm::Xxhash);
        assert_eq!(a, b);
        assert_eq!(a.0.len(), 32);
    }

    #[test]
    fn blake3_deterministic() {
        let a = hash_bytes(b"hello", HashAlgorithm::Blake3);
        let b = hash_bytes(b"hello", HashAlgorithm::Blake3);
        assert_eq!(a, b);
        assert_eq!(a.0.len(), 64);
    }

    #[test]
    fn different_algos_different_output() {
        let xxh = hash_bytes(b"hello", HashAlgorithm::Xxhash);
        let blk = hash_bytes(b"hello", HashAlgorithm::Blake3);
        assert_ne!(xxh, blk);
        assert_ne!(xxh.0.len(), blk.0.len());
    }

    #[test]
    fn parse_algorithm() {
        assert_eq!("xxhash".parse::<HashAlgorithm>().unwrap(), HashAlgorithm::Xxhash);
        assert_eq!("blake3".parse::<HashAlgorithm>().unwrap(), HashAlgorithm::Blake3);
        assert_eq!("XXHASH".parse::<HashAlgorithm>().unwrap(), HashAlgorithm::Xxhash);
        assert!("unknown".parse::<HashAlgorithm>().is_err());
    }

    #[test]
    fn hex_len_correct() {
        assert_eq!(HashAlgorithm::Xxhash.hex_len(), 32);
        assert_eq!(HashAlgorithm::Blake3.hex_len(), 64);
    }
}
