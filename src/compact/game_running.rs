//! Detect whether any tracked game is currently running.
//!
//! Used by the background compaction worker to auto-pause while a game is
//! in play, so compaction never competes with the user's game for disk/CPU.
//!
//! Detection strategy: enumerate processes via the `sysinfo` crate (already
//! a project dependency), then check each process's `exe()` path. If that
//! path lies **inside** a tracked game's `game_dir`, that game is running.
//!
//! Only paths that canonically resolve under a tracked `game_dir` count —
//! a coincidentally named `game.exe` somewhere else on disk does not.

use crate::db::games::Game;
use crate::db::GamesDb;
use crate::error::GResult;
use std::path::{Path, PathBuf};

/// One running tracked game + the exe that proved it.
#[derive(Debug, Clone)]
pub struct RunningGame {
    pub alias: String,
    pub title: String,
    pub exe: PathBuf,
}

/// Result of a running-game scan.
#[derive(Debug, Default, Clone)]
pub struct GameRunState {
    pub running: Vec<RunningGame>,
}

impl GameRunState {
    pub fn any(&self) -> bool { !self.running.is_empty() }

    /// Human-readable summary, e.g. `"cyberpunk (cyberpunk2077.exe)"`.
    pub fn summary(&self) -> String {
        self.running.iter()
            .map(|g| format!("{} ({})", g.alias,
                             g.exe.file_name().map(|f| f.to_string_lossy().into_owned())
                               .unwrap_or_else(|| "?".into())))
            .collect::<Vec<_>>().join(", ")
    }
}

/// Scan all tracked games and return the ones whose `game_dir` currently
/// contains at least one running process executable.
pub fn check_running_tracked(gdb: &GamesDb) -> GResult<GameRunState> {
    let games = gdb.list()?;
    Ok(scan_against(&games))
}

/// Lower-level entry point used by the background worker for repeated
/// polling (avoids re-opening the games DB each tick).
pub fn scan_against(games: &[Game]) -> GameRunState {
    if games.is_empty() { return GameRunState::default(); }

    // Canonicalize game dirs once (ignore ones that don't resolve).
    let dirs: Vec<(Game, PathBuf)> = games.iter()
        .filter_map(|g| {
            std::fs::canonicalize(&g.game_dir).ok().map(|c| (g.clone(), c))
        })
        .collect();
    if dirs.is_empty() { return GameRunState::default(); }

    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut running: Vec<RunningGame> = Vec::new();
    let mut seen_alias: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (_pid, proc_) in sys.processes() {
        let exe = match proc_.exe() { Some(p) => p, None => continue };
        let exe_canon = match std::fs::canonicalize(exe) { Ok(c) => c, Err(_) => continue };
        for (game, dir) in &dirs {
            if exe_canon.starts_with(dir) && seen_alias.insert(game.alias.clone()) {
                running.push(RunningGame {
                    alias: game.alias.clone(),
                    title: game.title.clone(),
                    exe: exe_canon.clone(),
                });
            }
        }
    }

    GameRunState { running }
}

/// `true` if `exe_path` canonically lives inside `game_dir`.
///
/// Exposed for unit testing without a live process table.
pub fn exe_inside_game_dir(exe_path: &Path, game_dir: &Path) -> bool {
    let exe = std::fs::canonicalize(exe_path).unwrap_or_else(|_| exe_path.to_path_buf());
    let dir = std::fs::canonicalize(game_dir).unwrap_or_else(|_| game_dir.to_path_buf());
    exe.starts_with(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn empty_state_is_not_any() {
        let s = GameRunState::default();
        assert!(!s.any());
        assert_eq!(s.summary(), "");
    }

    #[test]
    fn summary_lists_aliases() {
        let s = GameRunState {
            running: vec![
                RunningGame {
                    alias: "cp2077".into(), title: "Cyberpunk".into(),
                    exe: PathBuf::from("C:/Games/CP2077/cp.exe"),
                },
            ],
        };
        assert!(s.any());
        assert_eq!(s.summary(), "cp2077 (cp.exe)");
    }

    #[test]
    fn exe_inside_checks_prefix() {
        // Synthetic paths (no canonicalization needed since neither exists).
        assert!(exe_inside_game_dir(
            Path::new("C:/Games/Foo/game.exe"),
            Path::new("C:/Games/Foo"),
        ));
        assert!(!exe_inside_game_dir(
            Path::new("C:/Games/Bar/game.exe"),
            Path::new("C:/Games/Foo"),
        ));
    }
}
