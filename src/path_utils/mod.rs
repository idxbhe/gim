//! Path normalization utilities.

use crate::error::{GError, GResult};
use std::path::{Component, Path, PathBuf};

pub fn normalize(game_root: &Path, file_path: &Path) -> GResult<String> {
    let rel = if file_path.is_absolute() {
        file_path
            .strip_prefix(game_root)
            .map_err(|_| GError::Path(format!("file path \"{}\" is not inside game root \"{}\"", file_path.display(), game_root.display())))?
    } else {
        file_path
    };

    let mut parts: Vec<&str> = Vec::with_capacity(8);
    for comp in rel.components() {
        match comp {
            Component::Normal(os) => {
                let s = os.to_str().ok_or_else(|| GError::Path(format!("path \"{}\" contains non-UTF-8 components", rel.display())))?;
                parts.push(s);
            }
            Component::CurDir => {}
            Component::ParentDir => { if !parts.is_empty() { parts.pop(); } }
            Component::RootDir | Component::Prefix(_) => {
                return Err(GError::Path(format!("path \"{}\" contains unexpected absolute component", rel.display())));
            }
        }
    }
    if parts.is_empty() {
        return Err(GError::Path(format!("normalized path is empty for \"{}\"", file_path.display())));
    }
    Ok(parts.join("/"))
}

pub fn denormalize(game_root: &Path, normalized: &str) -> PathBuf {
    let mut p = game_root.to_path_buf();
    for part in normalized.split('/') { p.push(part); }
    p
}

pub fn hash_prefix(hash: &str) -> &str { &hash[..2] }

pub fn object_path(objects_dir: &Path, hash: &str) -> PathBuf {
    debug_assert!(hash.len() >= 2);
    objects_dir.join(&hash[..2]).join(hash)
}

/// Convert a relative OS-native path to forward-slash form without
/// allocating a new String on platforms where backslash is never a
/// separator (Unix). On Windows, does the replace.
///
/// This is used in hot paths (walker filter callbacks) where we want
/// to avoid unnecessary heap allocation.
pub fn to_forward_slash(rel: &str) -> String {
    #[cfg(windows)]
    { rel.replace('\\', "/") }
    #[cfg(not(windows))]
    { rel.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_simple() {
        let root = Path::new("/games/mario");
        assert_eq!(normalize(root, Path::new("/games/mario/mods/sky.dds")).unwrap(), "mods/sky.dds");
    }
    #[test]
    fn normalize_already_relative() {
        let root = Path::new("/games/mario");
        assert_eq!(normalize(root, Path::new("mods/sky.dds")).unwrap(), "mods/sky.dds");
    }
    #[test]
    fn normalize_rejects_outside() {
        assert!(normalize(Path::new("/games/mario"), Path::new("/etc/passwd")).is_err());
    }
    #[test]
    fn object_path_uses_2char_prefix() {
        let p = object_path(Path::new("/data/objects"), "abcdef0123456789abcdef0123456789");
        assert_eq!(p, Path::new("/data/objects/ab/abcdef0123456789abcdef0123456789"));
    }
}
