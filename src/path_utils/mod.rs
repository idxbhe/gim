//! Path normalization utilities.
//!
//! Per spec, every file path stored in the database **must** be:
//! 1. Relative to the game directory root.
//! 2. Use forward slash `/` on all platforms.
//! 3. No leading slash.
//! 4. No trailing slash.
//! 5. UTF-8 encoded.
//!
//! Normalization is applied at both `snap` and `restore` time, and must be
//! consistent across all commands. All other modules consume normalized
//! paths exclusively — they never touch raw `PathBuf`s for stored data.

use crate::error::{GError, GResult};
use std::path::{Component, Path, PathBuf};

/// Normalize an absolute or relative file path against a game root.
///
/// Returns a `String` (not `PathBuf`) because the result is a POSIX-style
/// relative path that we want to store verbatim in SQLite and compare by
/// exact string equality.
pub fn normalize(game_root: &Path, file_path: &Path) -> GResult<String> {
    let rel = if file_path.is_absolute() {
        file_path
            .strip_prefix(game_root)
            .map_err(|_| {
                GError::Path(format!(
                    "file path \"{}\" is not inside game root \"{}\"",
                    file_path.display(),
                    game_root.display()
                ))
            })?
    } else {
        file_path
    };

    let mut parts: Vec<&str> = Vec::with_capacity(8);
    for comp in rel.components() {
        match comp {
            Component::Normal(os) => {
                let s = os.to_str().ok_or_else(|| {
                    GError::Path(format!(
                        "path \"{}\" contains non-UTF-8 components",
                        rel.display()
                    ))
                })?;
                parts.push(s);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !parts.is_empty() {
                    parts.pop();
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(GError::Path(format!(
                    "path \"{}\" contains unexpected absolute component",
                    rel.display()
                )));
            }
        }
    }

    if parts.is_empty() {
        return Err(GError::Path(format!(
            "normalized path is empty for \"{}\"",
            file_path.display()
        )));
    }

    Ok(parts.join("/"))
}

/// Convert a normalized (forward-slash, relative) path back into a
/// platform-specific `PathBuf` rooted at `game_root`.
pub fn denormalize(game_root: &Path, normalized: &str) -> PathBuf {
    let mut p = game_root.to_path_buf();
    for part in normalized.split('/') {
        p.push(part);
    }
    p
}

/// Return the 2-character prefix for a hash (used for prefix-based directory
/// sharding in the object store).
pub fn hash_prefix(hash: &str) -> &str {
    &hash[..2]
}

/// Determine the object-store path for a given hash.
///
/// Per spec, objects are stored as `objects/[hash_prefix]/[hash]`.
pub fn object_path(objects_dir: &Path, hash: &str) -> PathBuf {
    debug_assert!(hash.len() >= 2, "hash must be at least 2 chars, got {hash}");
    let prefix = &hash[..2];
    objects_dir.join(prefix).join(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_simple() {
        let root = Path::new("/games/mario");
        let f = Path::new("/games/mario/mods/sky.dds");
        assert_eq!(normalize(root, f).unwrap(), "mods/sky.dds");
    }

    #[test]
    fn normalize_already_relative() {
        let root = Path::new("/games/mario");
        let f = Path::new("mods/sky.dds");
        assert_eq!(normalize(root, f).unwrap(), "mods/sky.dds");
    }

    #[test]
    fn normalize_strips_curdir() {
        let root = Path::new("/games/mario");
        let f = Path::new("./mods/./sky.dds");
        assert_eq!(normalize(root, f).unwrap(), "mods/sky.dds");
    }

    #[test]
    fn normalize_rejects_outside() {
        let root = Path::new("/games/mario");
        let f = Path::new("/etc/passwd");
        assert!(normalize(root, f).is_err());
    }

    #[test]
    fn object_path_uses_2char_prefix() {
        let objects = Path::new("/data/mario/objects");
        let p = object_path(objects, "abcdef0123456789abcdef0123456789");
        assert_eq!(
            p,
            Path::new("/data/mario/objects/ab/abcdef0123456789abcdef0123456789")
        );
    }
}
