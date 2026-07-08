//! XXH3-128 hashing for file content.

use crate::error::{GError, GResult};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use xxhash_rust::xxh3::Xxh3;

const READ_BUF: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hash(pub String);

impl Hash {
    pub fn as_str(&self) -> &str { &self.0 }
    pub fn prefix(&self) -> &str { &self.0[..2] }
}

impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}

impl From<String> for Hash {
    fn from(s: String) -> Self { Self(s) }
}

pub fn hash_bytes(data: &[u8]) -> Hash {
    use xxhash_rust::xxh3::xxh3_128;
    Hash(format!("{:032x}", xxh3_128(data)))
}

pub fn hash_file(path: &Path) -> GResult<(Hash, u64)> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(READ_BUF, file);
    let mut hasher = Xxh3::new();
    let mut total: u64 = 0;
    let mut buf = vec![0u8; READ_BUF];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((Hash(format!("{:032x}", hasher.digest128())), total))
}

pub fn hash_file_with_retry(path: &Path, max_retries: u32, retry_delay: std::time::Duration) -> GResult<Option<(Hash, u64)>> {
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..=max_retries {
        match hash_file(path) {
            Ok(v) => return Ok(Some(v)),
            Err(GError::Io(e)) => {
                let kind = e.kind();
                let retryable = matches!(kind,
                    std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::ResourceBusy
                ) || e.raw_os_error().is_some_and(|c| c == 32 || c == 33);
                if !retryable || attempt == max_retries { last_err = Some(e); break; }
                last_err = Some(e);
                std::thread::sleep(retry_delay);
            }
            Err(other) => return Err(other),
        }
    }
    log::debug!("file {:?} could not be opened after retries: {:?}", path, last_err);
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn hash_deterministic() { assert_eq!(hash_bytes(b"hello"), hash_bytes(b"hello")); }
    #[test]
    fn hash_differs() { assert_ne!(hash_bytes(b"a"), hash_bytes(b"b")); }
    #[test]
    fn hash_file_streams() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"test data").unwrap();
        tmp.flush().unwrap();
        let (h, n) = hash_file(tmp.path()).unwrap();
        assert_eq!(n, 9);
        assert_eq!(h, hash_bytes(b"test data"));
    }
}
