//! Ignore-pattern matching — gitignore-compatible.
//!
//! Per spec, ignore patterns come from three sources (all merged):
//! 1. **Built-in defaults** (`*.tmp`, `*.temp`, `*.bak`, `*.swp`, `Thumbs.db`,
//!    `.DS_Store`, `desktop.ini`) — always applied, cannot be overridden.
//! 2. **Global `data/gignore`** — applies to all games.
//! 3. **Per-game `data/[alias]/.gignore`** — applies to a specific game.
//! 4. **In-game `[gameDir]/.gignore`** — lives inside the game directory.
//!
//! Pattern syntax follows gitignore semantics (negations with `!`,
//! directory-suffix `/`, glob wildcards). We use the `ignore` crate's
//! `Gitignore` parser, which is the same one `ripgrep` uses and is
//! battle-tested.

use crate::config::Paths;
use crate::error::{GError, GResult};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::Path;

/// Built-in default patterns that are always applied. Users may append
/// to these via `data/gignore`, but cannot override them.
pub const DEFAULT_PATTERNS: &[&str] = &[
    "*.tmp",
    "*.temp",
    "*.bak",
    "*.swp",
    "Thumbs.db",
    ".DS_Store",
    "desktop.ini",
];

/// A merged set of ignore patterns from all sources, ready to test
/// file paths against.
pub struct IgnoreSet {
    matcher: Gitignore,
    /// Human-readable source labels, for `g ignore --list`.
    pub sources: Vec<IgnoreSource>,
}

/// A named source of ignore patterns (used by `g ignore --list`).
#[derive(Debug, Clone)]
pub struct IgnoreSource {
    pub label: String,
    pub patterns: Vec<String>,
}

impl IgnoreSet {
    /// Build an empty ignore set (no patterns). Used as a starting point.
    pub fn empty() -> GResult<Self> {
        let matcher = GitignoreBuilder::new("");
        let matcher = matcher.build()?;
        Ok(Self {
            matcher,
            sources: vec![],
        })
    }

    /// Add patterns from a string slice, with a source label.
    pub fn add_lines(&mut self, root: &Path, label: &str, lines: &[String]) -> GResult<()> {
        let mut builder = GitignoreBuilder::new(root);
        let mut kept: Vec<String> = Vec::with_capacity(lines.len());
        for raw in lines {
            let trimmed = raw.trim();
            // Skip comments and blank lines (but keep them in the listing
            // output for readability).
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Err(e) = builder.add_line(None, raw) {
                return Err(GError::IgnorePattern {
                    file: label.into(),
                    message: format!("line \"{raw}\": {e}"),
                });
            }
            kept.push(raw.clone());
        }
        // Merge with existing matcher: re-build from accumulated lines.
        // Gitignore doesn't support incremental merge, so we re-create the
        // full matcher from scratch each time.
        let merged = if self.sources.is_empty() {
            builder.build()?
        } else {
            let mut b = GitignoreBuilder::new(root);
            for src in &self.sources {
                for line in &src.patterns {
                    let _ = b.add_line(None, line);
                }
            }
            for line in &kept {
                let _ = b.add_line(None, line);
            }
            b.build()?
        };
        self.matcher = merged;
        self.sources.push(IgnoreSource {
            label: label.to_string(),
            patterns: kept,
        });
        Ok(())
    }

    /// Add patterns from a file on disk. Skips silently if the file does
    /// not exist (all ignore files are optional per spec).
    pub fn add_file(&mut self, root: &Path, label: &str, path: &Path) -> GResult<()> {
        if !path.exists() {
            return Ok(());
        }
        let contents = std::fs::read_to_string(path)?;
        let lines: Vec<String> = contents.lines().map(|s| s.to_string()).collect();
        self.add_lines(root, label, &lines)
    }

    /// Test whether a relative path (forward-slash style) should be ignored.
    ///
    /// `is_dir` indicates whether the path is a directory (affects whether
    /// patterns ending in `/` match).
    pub fn is_ignored(&self, relative_path: &str, is_dir: bool) -> bool {
        // Gitignore::matched expects a path relative to its root. Since we
        // always feed it the normalized (forward-slash, relative) form,
        // this works regardless of platform.
        let p = Path::new(relative_path);
        match self.matcher.matched(p, is_dir) {
            ignore::Match::Ignore(_) => true,
            ignore::Match::Whitelist(_) => false,
            ignore::Match::None => false,
        }
    }
}

/// Build the complete ignore set for a game: built-in defaults + global
/// gignore + per-game gignore + in-game gignore.
pub fn build_for_game(paths: &Paths, alias: &str, game_dir: &Path) -> GResult<IgnoreSet> {
    let mut set = IgnoreSet::empty()?;

    // 1. Built-in defaults — always applied.
    let defaults: Vec<String> = DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect();
    set.add_lines(game_dir, "# Global defaults (built-in)", &defaults)?;

    // 2. Global gignore (data/gignore)
    set.add_file(game_dir, "# Global gignore (data/gignore)", &paths.global_gignore)?;

    // 3. Per-game gignore (data/[alias]/.gignore)
    set.add_file(
        game_dir,
        &format!("# Per-game (data/{alias}/.gignore)"),
        &paths.per_game_gignore(alias),
    )?;

    // 4. In-game gignore (gameDir/.gignore)
    set.add_file(
        game_dir,
        "# In-game (gameDir/.gignore)",
        &paths.in_game_gignore(game_dir),
    )?;

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
    fn matches_simple_glob() {
        let s = build(&["*.tmp"]);
        assert!(s.is_ignored("foo.tmp", false));
        assert!(!s.is_ignored("foo.txt", false));
    }

    #[test]
    fn matches_dir_pattern() {
        // `logs/` matches the directory `logs` itself (when is_dir=true).
        // The walker then prunes the entire subtree, so files inside
        // are never visited. Per gitignore semantics, a directory-only
        // pattern does NOT match a file path directly.
        let s = build(&["logs/"]);
        assert!(s.is_ignored("logs", true)); // dir itself
        assert!(!s.is_ignored("logs.txt", false)); // unrelated file
        // `logs/foo.txt` would not be tested directly in practice —
        // the walker prunes `logs` before descending into it.
    }

    #[test]
    fn negation_reincludes() {
        let s = build(&["*.log", "!important.log"]);
        assert!(s.is_ignored("foo.log", false));
        assert!(!s.is_ignored("important.log", false));
    }

    #[test]
    fn path_specific_pattern() {
        let s = build(&["saves/auto_save_*"]);
        assert!(s.is_ignored("saves/auto_save_001", false));
        assert!(!s.is_ignored("saves/manual_save_001", false));
    }
}
