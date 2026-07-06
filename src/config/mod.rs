//! Configuration & path resolution.
//!
//! The `gim` binary lives somewhere on disk; per spec, its data lives in
//! a `data/` directory next to the binary. We resolve this once at
//! startup and never re-compute it.

use crate::error::{GError, GResult};
use std::path::{Path, PathBuf};

/// Resolved global configuration: where the `gim` binary lives, where
/// its `data/` directory is, and where the global `games.db` is.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Directory containing the `gim` executable (or, if overridden, the
    /// user-specified data root).
    pub binary_dir: PathBuf,
    /// `data/` directory next to the binary (or the override).
    pub data_dir: PathBuf,
    /// `data/games.db` — global registry of tracked games.
    pub games_db: PathBuf,
    /// `data/gignore` — global ignore patterns.
    pub global_gignore: PathBuf,
}

impl Paths {
    /// Resolve paths from the current executable location.
    pub fn from_env() -> GResult<Self> {
        let exe = std::env::current_exe()
            .map_err(|e| GError::Config(format!("cannot locate current exe: {e}")))?;
        let binary_dir = exe
            .parent()
            .ok_or_else(|| GError::Config("exe has no parent directory".into()))?
            .to_path_buf();
        Self::from_binary_dir(binary_dir)
    }

    /// Construct from an explicit binary directory (used in tests).
    pub fn from_binary_dir(binary_dir: PathBuf) -> GResult<Self> {
        let data_dir = binary_dir.join("data");
        let games_db = data_dir.join("games.db");
        let global_gignore = data_dir.join("gignore");
        Ok(Self {
            binary_dir,
            data_dir,
            games_db,
            global_gignore,
        })
    }

    /// Override the data root (for `GIM_DATA_DIR` env var, tests, etc.).
    pub fn with_data_dir(mut self, data_dir: PathBuf) -> Self {
        self.games_db = data_dir.join("games.db");
        self.global_gignore = data_dir.join("gignore");
        self.data_dir = data_dir;
        self
    }

    /// Directory for a specific game's per-game data.
    pub fn game_data_dir(&self, alias: &str) -> PathBuf {
        self.data_dir.join(alias)
    }

    /// Path to a game's `snaps.db`.
    pub fn snaps_db(&self, alias: &str) -> PathBuf {
        self.game_data_dir(alias).join("snaps.db")
    }

    /// Path to a game's `objects/` directory.
    pub fn objects_dir(&self, alias: &str) -> PathBuf {
        self.game_data_dir(alias).join("objects")
    }

    /// Path to a game's per-game `.gignore` file (in the data dir).
    pub fn per_game_gignore(&self, alias: &str) -> PathBuf {
        self.game_data_dir(alias).join(".gignore")
    }

    /// Path to a game's in-game `.gignore` file (inside the game directory).
    pub fn in_game_gignore(&self, game_dir: &Path) -> PathBuf {
        game_dir.join(".gignore")
    }

    /// Ensure the global `data/` directory exists.
    pub fn ensure_data_dir(&self) -> GResult<()> {
        if !self.data_dir.exists() {
            std::fs::create_dir_all(&self.data_dir)?;
        }
        Ok(())
    }
}

/// Read the `GIM_DATA_DIR` environment variable if set, and use it to
/// override the default data directory.
pub fn env_data_dir_override() -> Option<PathBuf> {
    std::env::var_os("GIM_DATA_DIR").map(PathBuf::from)
}
