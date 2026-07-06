//! XXH3-128 hashing for file content.
//!
//! Per spec: 128-bit, non-cryptographic, output as 32-char lowercase hex.
//!
//! XXH3 is implemented by the `xxhash-rust` crate and runs at multi-GB/s
//! on modern CPUs. We use the streaming hasher so that arbitrarily large
//! game files (textures, archives) can be hashed without loading them
//! fully into memory.

use crate::error::{GError, GResult};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use xxhash_rust::xxh3::Xxh3;

/// Output buffer size for streaming reads. 1 MiB strikes a good balance
/// between syscall overhead and cache pressure on most platforms.
const READ_BUF: usize = 1024 * 1024;

/// A finalized 128-bit XXH3 hash, stored as a 32-character lowercase
/// hex string. We use a `String` rather than `[u8; 16]` because every
/// consumer (SQLite, JSON output, object-store path) wants the hex form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hash(pub String);

impl Hash {
    /// Returns the 32-character lowercase hex digest.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// First 2 characters — used for object-store directory sharding.
    pub fn prefix(&self) -> &str {
        &self.0[..2]
    }
}

impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Hash {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Hash an in-memory byte slice. Used for small inputs (e.g. test
/// fixtures, ignore-pattern blobs).
pub fn hash_bytes(data: &[u8]) -> Hash {
    use xxhash_rust::xxh3::xxh3_128;
    let h = xxh3_128(data);
    Hash(format!("{:032x}", h))
}

/// Hash a file on disk by streaming it through a 1 MiB buffer.
///
/// Returns the hash and the file size in bytes. The caller usually wants
/// both — storing `(hash, size)` together avoids a separate `stat()`
/// call and avoids TOCTOU if the file changes mid-hash.
///
/// # Errors
///
/// Returns [`GError::Io`] for filesystem errors, or [`GError::Hashing`]
/// for unexpected internal errors.
pub fn hash_file(path: &Path) -> GResult<(Hash, u64)> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(READ_BUF, file);
    let mut hasher = Xxh3::new();
    let mut total: u64 = 0;
    let mut buf = vec![0u8; READ_BUF];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }

    let digest = hasher.digest128();
    Ok((Hash(format!("{:032x}", digest)), total))
}

/// Try to open and hash a file, retrying up to `max_retries` times with
/// `retry_delay` between attempts if the file cannot be opened (e.g. it
/// is locked by a running game).
///
/// Returns `Ok(Some(hash))` if the file was hashed successfully,
/// `Ok(None)` if all retries failed (file is locked), or `Err` for
/// non-retryable errors.
pub fn hash_file_with_retry(
    path: &Path,
    max_retries: u32,
    retry_delay: std::time::Duration,
) -> GResult<Option<(Hash, u64)>> {
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..=max_retries {
        match hash_file(path) {
            Ok(v) => return Ok(Some(v)),
            Err(GError::Io(e)) => {
                let kind = e.kind();
                // Only retry on permission/sharing errors. Other errors
                // (not found, invalid path) are non-retryable.
                let retryable = matches!(
                    kind,
                    std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::ResourceBusy
                ) || e.raw_os_error().is_some_and(|c| {
                    // Windows: ERROR_SHARING_VIOLATION (32) and ERROR_LOCK_VIOLATION (33)
                    c == 32 || c == 33
                });
                if !retryable || attempt == max_retries {
                    last_err = Some(e);
                    break;
                }
                last_err = Some(e);
                std::thread::sleep(retry_delay);
            }
            Err(other) => return Err(other),
        }
    }
    // Locked — return None so caller can collect warnings
    log::debug!(
        "file {:?} could not be opened after retries: {:?}",
        path,
        last_err
    );
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn hash_empty() {
        let h = hash_bytes(b"");
        assert_eq!(h.0.len(), 32);
        // XXH3-128 of empty input — known constant
        assert_eq!(h.as_str(), "99aa06d3014798d86001c324468d497f");
    }

    #[test]
    fn hash_deterministic() {
        let a = hash_bytes(b"hello world");
        let b = hash_bytes(b"hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_differs_for_different_input() {
        let a = hash_bytes(b"hello world");
        let b = hash_bytes(b"hello worlD");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_file_streams() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        let payload = b"some test data, larger than zero bytes";
        tmp.write_all(payload).unwrap();
        tmp.flush().unwrap();
        let (h, size) = hash_file(tmp.path()).unwrap();
        assert_eq!(size as usize, payload.len());
        assert_eq!(h, hash_bytes(payload));
    }
}
