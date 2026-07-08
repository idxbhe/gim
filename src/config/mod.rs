//! Configuration & path resolution.

use crate::error::{GError, GResult};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub binary_dir: PathBuf,
    pub data_dir: PathBuf,
    pub games_db: PathBuf,
    pub global_gignore: PathBuf,
}

impl Paths {
    pub fn from_env() -> GResult<Self> {
        let exe = std::env::current_exe().map_err(|e| GError::Config(format!("cannot locate current exe: {e}")))?;
        let binary_dir = exe.parent().ok_or_else(|| GError::Config("exe has no parent directory".into()))?.to_path_buf();
        Self::from_binary_dir(binary_dir)
    }

    pub fn from_binary_dir(binary_dir: PathBuf) -> GResult<Self> {
        let data_dir = binary_dir.join("data");
        Ok(Self {
            games_db: data_dir.join("games.db"),
            global_gignore: data_dir.join("gignore"),
            binary_dir,
            data_dir,
        })
    }

    pub fn with_data_dir(mut self, data_dir: PathBuf) -> Self {
        self.games_db = data_dir.join("games.db");
        self.global_gignore = data_dir.join("gignore");
        self.data_dir = data_dir;
        self
    }

    pub fn game_data_dir(&self, alias: &str) -> PathBuf { self.data_dir.join(alias) }
    pub fn snaps_db(&self, alias: &str) -> PathBuf { self.game_data_dir(alias).join("snaps.db") }
    pub fn objects_dir(&self, alias: &str) -> PathBuf { self.game_data_dir(alias).join("objects") }
    pub fn per_game_gignore(&self, alias: &str) -> PathBuf { self.game_data_dir(alias).join(".gignore") }
    pub fn in_game_gignore(&self, game_dir: &Path) -> PathBuf { game_dir.join(".gignore") }

    pub fn ensure_data_dir(&self) -> GResult<()> {
        if !self.data_dir.exists() { std::fs::create_dir_all(&self.data_dir)?; }
        Ok(())
    }
}

pub fn env_data_dir_override() -> Option<PathBuf> {
    std::env::var_os("GIM_DATA_DIR").map(PathBuf::from)
}
