//! Ignore-pattern matching — gitignore-compatible.

use crate::config::Paths;
use crate::error::{GError, GResult};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::Path;

pub const DEFAULT_PATTERNS: &[&str] = &["*.tmp", "*.temp", "*.bak", "*.swp", "Thumbs.db", ".DS_Store", "desktop.ini"];

pub struct IgnoreSet {
    matcher: Gitignore,
    pub sources: Vec<IgnoreSource>,
}

#[derive(Debug, Clone)]
pub struct IgnoreSource {
    pub label: String,
    pub patterns: Vec<String>,
}

impl IgnoreSet {
    pub fn empty() -> GResult<Self> {
        Ok(Self { matcher: GitignoreBuilder::new("").build()?, sources: vec![] })
    }

    pub fn add_lines(&mut self, root: &Path, label: &str, lines: &[String]) -> GResult<()> {
        let mut builder = GitignoreBuilder::new(root);
        let mut kept: Vec<String> = Vec::with_capacity(lines.len());
        for raw in lines {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') { continue; }
            if let Err(e) = builder.add_line(None, raw) {
                return Err(GError::IgnorePattern { file: label.into(), message: format!("line \"{raw}\": {e}") });
            }
            kept.push(raw.clone());
        }
        let merged = if self.sources.is_empty() {
            builder.build()?
        } else {
            let mut b = GitignoreBuilder::new(root);
            for src in &self.sources { for line in &src.patterns { let _ = b.add_line(None, line); } }
            for line in &kept { let _ = b.add_line(None, line); }
            b.build()?
        };
        self.matcher = merged;
        self.sources.push(IgnoreSource { label: label.to_string(), patterns: kept });
        Ok(())
    }

    pub fn add_file(&mut self, root: &Path, label: &str, path: &Path) -> GResult<()> {
        if !path.exists() { return Ok(()); }
        let contents = std::fs::read_to_string(path)?;
        let lines: Vec<String> = contents.lines().map(|s| s.to_string()).collect();
        self.add_lines(root, label, &lines)
    }

    pub fn is_ignored(&self, relative_path: &str, is_dir: bool) -> bool {
        let p = Path::new(relative_path);
        matches!(self.matcher.matched(p, is_dir), ignore::Match::Ignore(_))
    }
}

pub fn build_for_game(paths: &Paths, alias: &str, game_dir: &Path) -> GResult<IgnoreSet> {
    let mut set = IgnoreSet::empty()?;
    let defaults: Vec<String> = DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect();
    set.add_lines(game_dir, "# Global defaults (built-in)", &defaults)?;
    set.add_file(game_dir, "# Global gignore (data/gignore)", &paths.global_gignore)?;
    set.add_file(game_dir, &format!("# Per-game (data/{alias}/.gignore)"), &paths.per_game_gignore(alias))?;
    set.add_file(game_dir, "# In-game (gameDir/.gignore)", &paths.in_game_gignore(game_dir))?;
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(lines: &[&str]) -> IgnoreSet {
        let mut s = IgnoreSet::empty().unwrap();
        let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        s.add_lines(Path::new("/games/mario"), "test", &owned).unwrap();
        s
    }

    #[test]
    fn matches_glob() {
        let s = build(&["*.tmp"]);
        assert!(s.is_ignored("foo.tmp", false));
        assert!(!s.is_ignored("foo.txt", false));
    }
    #[test]
    fn negation() {
        let s = build(&["*.log", "!important.log"]);
        assert!(s.is_ignored("foo.log", false));
        assert!(!s.is_ignored("important.log", false));
    }
}
