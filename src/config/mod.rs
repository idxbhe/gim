use crate::error::{GError, GResult};
use std::path::{Path, PathBuf};

pub mod gim_config;
pub use gim_config::GimConfig;

#[derive(Debug, Clone)]
pub struct Paths {
    pub binary_dir: PathBuf, pub data_dir: PathBuf, pub games_db: PathBuf, pub global_gignore: PathBuf,
}

impl Paths {
    pub fn from_env() -> GResult<Self> {
        let exe = std::env::current_exe().map_err(|e| GError::Config(format!("cannot locate exe: {e}")))?;
        let bd = exe.parent().ok_or_else(|| GError::Config("no parent dir".into()))?.to_path_buf();
        Self::from_binary_dir(bd)
    }
    pub fn from_binary_dir(bd: PathBuf) -> GResult<Self> {
        let dd = bd.join("data");
        Ok(Self { games_db: dd.join("games.db"), global_gignore: dd.join("gignore"), binary_dir: bd, data_dir: dd })
    }
    pub fn with_data_dir(mut self, dd: PathBuf) -> Self {
        self.games_db = dd.join("games.db"); self.global_gignore = dd.join("gignore"); self.data_dir = dd; self
    }
    pub fn game_data_dir(&self, a: &str) -> PathBuf { self.data_dir.join(a) }
    pub fn snaps_db(&self, a: &str) -> PathBuf { self.game_data_dir(a).join("snaps.db") }
    pub fn objects_dir(&self, a: &str) -> PathBuf { self.game_data_dir(a).join("objects") }
    pub fn per_game_gignore(&self, a: &str) -> PathBuf { self.game_data_dir(a).join(".gignore") }
    pub fn in_game_gignore(&self, gd: &Path) -> PathBuf { gd.join(".gignore") }
    pub fn ensure_data_dir(&self) -> GResult<()> { if !self.data_dir.exists() { std::fs::create_dir_all(&self.data_dir)?; } Ok(()) }
}

pub fn env_data_dir_override() -> Option<PathBuf> { std::env::var_os("GIM_DATA_DIR").map(PathBuf::from) }
