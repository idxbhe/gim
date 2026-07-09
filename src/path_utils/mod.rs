use crate::error::{GError, GResult};
use std::path::{Component, Path, PathBuf};

pub fn normalize(game_root: &Path, file_path: &Path) -> GResult<String> {
    let rel = if file_path.is_absolute() {
        file_path.strip_prefix(game_root).map_err(|_| GError::Path(format!("file path \"{}\" not inside \"{}\"", file_path.display(), game_root.display())))?
    } else { file_path };
    let mut parts: Vec<&str> = Vec::with_capacity(8);
    for comp in rel.components() {
        match comp {
            Component::Normal(os) => { parts.push(os.to_str().ok_or_else(|| GError::Path(format!("non-UTF-8 path: {}", rel.display())))?); }
            Component::CurDir => {}
            Component::ParentDir => { if !parts.is_empty() { parts.pop(); } }
            _ => return Err(GError::Path(format!("unexpected component: {}", rel.display()))),
        }
    }
    if parts.is_empty() { return Err(GError::Path(format!("empty path: {}", file_path.display()))); }
    Ok(parts.join("/"))
}

pub fn denormalize(game_root: &Path, normalized: &str) -> PathBuf {
    let mut p = game_root.to_path_buf();
    for part in normalized.split('/') { p.push(part); }
    p
}

pub fn object_path(objects_dir: &Path, hash: &str) -> PathBuf {
    objects_dir.join(&hash[..2]).join(hash)
}

pub fn to_forward_slash(rel: &str) -> String {
    #[cfg(windows)] { rel.replace('\\', "/") }
    #[cfg(not(windows))] { rel.to_string() }
}
