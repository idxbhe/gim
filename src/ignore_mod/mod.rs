use crate::config::Paths;
use crate::error::{GError, GResult};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::Path;

pub const DEFAULT_PATTERNS: &[&str] = &["*.tmp", "*.temp", "*.bak", "*.swp", "Thumbs.db", ".DS_Store", "desktop.ini"];

pub struct IgnoreSet { matcher: Gitignore, pub sources: Vec<IgnoreSource> }
#[derive(Debug, Clone)]
pub struct IgnoreSource { pub label: String, pub patterns: Vec<String> }

impl Clone for IgnoreSet {
    fn clone(&self) -> Self {
        let mut new = IgnoreSet::empty().unwrap();
        for src in &self.sources { let _ = new.add_lines(Path::new("/"), &src.label, &src.patterns); }
        new
    }
}

impl IgnoreSet {
    pub fn empty() -> GResult<Self> { Ok(Self { matcher: GitignoreBuilder::new("").build()?, sources: vec![] }) }
    pub fn add_lines(&mut self, root: &Path, label: &str, lines: &[String]) -> GResult<()> {
        let mut builder = GitignoreBuilder::new(root);
        let mut kept = Vec::new();
        for raw in lines {
            let t = raw.trim();
            if t.is_empty() || t.starts_with('#') { continue; }
            if let Err(e) = builder.add_line(None, raw) { return Err(GError::IgnorePattern { file: label.into(), message: format!("line \"{raw}\": {e}") }); }
            kept.push(raw.clone());
        }
        let merged = if self.sources.is_empty() { builder.build()? } else {
            let mut b = GitignoreBuilder::new(root);
            for src in &self.sources { for l in &src.patterns { let _ = b.add_line(None, l); } }
            for l in &kept { let _ = b.add_line(None, l); }
            b.build()?
        };
        self.matcher = merged;
        self.sources.push(IgnoreSource { label: label.to_string(), patterns: kept });
        Ok(())
    }
    pub fn add_file(&mut self, root: &Path, label: &str, path: &Path) -> GResult<()> {
        if !path.exists() { return Ok(()); }
        let lines: Vec<String> = std::fs::read_to_string(path)?.lines().map(|s| s.to_string()).collect();
        self.add_lines(root, label, &lines)
    }
    pub fn is_ignored(&self, rp: &str, is_dir: bool) -> bool {
        matches!(self.matcher.matched(Path::new(rp), is_dir), ignore::Match::Ignore(_))
    }
}

pub fn build_for_game(paths: &Paths, alias: &str, game_dir: &Path) -> GResult<IgnoreSet> {
    let mut s = IgnoreSet::empty()?;
    let d: Vec<String> = DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect();
    s.add_lines(game_dir, "# Global defaults", &d)?;
    s.add_file(game_dir, "# Global gignore", &paths.global_gignore)?;
    s.add_file(game_dir, &format!("# Per-game (data/{alias}/.gignore)"), &paths.per_game_gignore(alias))?;
    s.add_file(game_dir, "# In-game", &paths.in_game_gignore(game_dir))?;
    Ok(s)
}
