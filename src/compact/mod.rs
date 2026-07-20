//! `gim compact` — on-disk compression of game folders & snapshot data.
//!
//! Two Windows mechanisms are supported (see [`algorithm::CompactMode`]):
//!
//! - **NTFS** compression (`FSCTL_SET_COMPRESSION`, LZNT1 only).
//! - **WOF** — Windows Overlay Filter (`FSCTL_SET_EXTERNAL_BACKING`,
//!   supports LZX / XPRESS4K / XPRESS8K / XPRESS16K). **LZX is the default.**
//!
//! The high-level flow lives in [`crate::commands::compact`]: scan →
//! estimate → confirm → compress (foreground or background). This module
//! hosts the building blocks: algorithm selection, per-file compress/
//! decompress, estimation, and background-state serialization.

pub mod algorithm;
pub mod estimate;
pub mod game_running;
pub mod ntfs;
pub mod wof;

pub use algorithm::{CompactAlgorithm, CompactMode};
pub use estimate::{Estimate, FileClass, FileKind, ScannedFile, SkipReason, scan, summarize};
pub use game_running::{GameRunState, RunningGame, check_running_tracked, scan_against};
pub use wof::{
    WofDriverStatus, WofRuntimeProbe,
    check_wof_available, enable_wof_driver, probe_wof_driver, probe_wof_runtime,
    reset_wof_availability,
};

use crate::error::{GError, GResult};
use std::path::{Path, PathBuf};

/// Which folder(s) to compact for a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFolder {
    /// The user's actual game directory (`Game.game_dir`).
    GameDir,
    /// The per-game snapshot data directory (`data/[alias]/objects`).
    DataDir,
    /// Both of the above.
    Both,
}

impl TargetFolder {
    pub fn parse(s: &str) -> GResult<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "game" | "game_dir" | "gamedir" => Ok(Self::GameDir),
            "data" | "data_dir" | "datadir" | "objects" => Ok(Self::DataDir),
            "both" | "all" => Ok(Self::Both),
            other => Err(GError::Other(format!(
                "unknown compact target \"{other}\" (supported: game, data, both)"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GameDir => "game",
            Self::DataDir => "data",
            Self::Both => "both",
        }
    }
}

impl Default for TargetFolder {
    fn default() -> Self { Self::GameDir }
}

/// Full options resolved for a compaction run (CLI merged with config).
#[derive(Debug, Clone)]
pub struct CompactOptions {
    pub algorithm: CompactAlgorithm,
    pub target: TargetFolder,
    pub threads: usize,
    pub exclude: Vec<String>,
    pub force: bool,
    pub confirm: bool,
    pub background: bool,
    pub dry_run: bool,
}

impl Default for CompactOptions {
    fn default() -> Self {
        Self {
            algorithm: CompactAlgorithm::Lzx,
            target: TargetFolder::GameDir,
            threads: 0,
            exclude: Vec::new(),
            force: false,
            confirm: false,
            background: false,
            dry_run: false,
        }
    }
}

/// Apply the configured algorithm to a single file.
///
/// Files below [`estimate::MIN_COMPRESS_SIZE`] are silently skipped (NTFS
/// round-trips those at a loss). Errors from the Win32 call bubble up; the
/// caller decides whether to treat them as fatal or collect them.
///
/// If the WOF driver is not available and a WOF algorithm is requested,
/// the [`GError::WofNotAvailable`] error is returned. Callers should use
/// [`probe_wof_driver`] before batch processing to check availability
/// and provide actionable guidance to the user.
pub fn compress_file(path: &Path, opts: &CompactOptions) -> GResult<()> {
    // Skip tiny files — overhead negates gains.
    if let Ok(m) = std::fs::symlink_metadata(path) {
        if m.len() < estimate::MIN_COMPRESS_SIZE { return Ok(()); }
    }

    match opts.algorithm {
        CompactAlgorithm::Ntfs => ntfs::set_ntfs_compression(path, ntfs::COMPRESSION_FORMAT_LZNT1),
        CompactAlgorithm::Xpress4k => wof::set_wof_compression(path, wof::FILE_PROVIDER_COMPRESSION_XPRESS4K),
        CompactAlgorithm::Lzx => wof::set_wof_compression(path, wof::FILE_PROVIDER_COMPRESSION_LZX),
        CompactAlgorithm::Xpress8k => wof::set_wof_compression(path, wof::FILE_PROVIDER_COMPRESSION_XPRESS8K),
        CompactAlgorithm::Xpress16k => wof::set_wof_compression(path, wof::FILE_PROVIDER_COMPRESSION_XPRESS16K),
        CompactAlgorithm::None => decompress_file(path),
    }
}

/// Remove any compression (both NTFS and WOF) from a file.
///
/// We try both mechanisms — WOF delete is a no-op if the file wasn't
/// WOF-backed, and NTFS `COMPRESSION_FORMAT_NONE` is a no-op if it wasn't
/// NTFS-compressed, so the order doesn't matter. The NTFS step uses a
/// verified decompress (with a copy-replace fallback) so it never reports
/// success while the file is still compressed (see BUG 1).
pub fn decompress_file(path: &Path) -> GResult<()> {
    // Try WOF first; if there's no WOF backing this returns Ok anyway
    // (ERROR_NOT_FOUND is treated as success inside the FFI layer).
    wof::remove_wof_compression(path)?;
    // Then clear NTFS compression state, verifying it actually cleared.
    ntfs::decompress_ntfs_verified(path)?;
    Ok(())
}

// ── Background compaction state file ───────────────────────────────────
//
// The background worker writes a small JSON state file at
// `data/[alias]/compact.state` so `gim compact --status` can report
// progress from a separate process invocation. The lock file at
// `data/[alias]/compact.lock` (managed via `fs2` in the command) prevents
// two workers from compacting the same game at once.

/// Lifecycle phase recorded in the state file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactPhase {
    /// Worker has booted and is about to start compressing.
    Starting,
    /// Actively compressing files.
    Running,
    /// Auto-paused because a tracked game is running.
    Paused,
    /// Finished (success).
    Done,
    /// Aborted with an error.
    Failed,
}

impl CompactPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "starting" => Self::Starting,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "done" => Self::Done,
            _ => Self::Failed,
        }
   }
}

/// Serializable snapshot of background progress, written as JSON.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompactState {
    pub phase: String,
    pub algorithm: String,
    pub target: String,
    pub total_files: u64,
    pub processed_files: u64,
    pub compressed_files: u64,
    pub skipped_files: u64,
    pub failed_files: u64,
    pub total_size: u64,
    pub started_at: i64,
    pub updated_at: i64,
    pub message: String,
}

impl CompactState {
    pub fn new(algorithm: CompactAlgorithm, target: TargetFolder) -> Self {
        let now = unix_now();
        Self {
            phase: CompactPhase::Starting.as_str().to_string(),
            algorithm: algorithm.as_str().to_string(),
            target: target.as_str().to_string(),
            total_files: 0,
            processed_files: 0,
            compressed_files: 0,
            skipped_files: 0,
            failed_files: 0,
            total_size: 0,
            started_at: now,
            updated_at: now,
            message: String::new(),
        }
    }

    pub fn phase(&self) -> CompactPhase {
        CompactPhase::parse(&self.phase)
    }

    pub fn save(&self, path: &Path) -> GResult<()> {
        if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load(path: &Path) -> GResult<Option<Self>> {
        if !path.exists() { return Ok(None); }
        let json = std::fs::read_to_string(path)?;
        let state: Self = serde_json::from_str(&json)?;
        Ok(Some(state))
    }
}

/// Path for a game's background compaction state file.
pub fn state_file_path(data_dir: &Path, alias: &str) -> PathBuf {
    data_dir.join(alias).join("compact.state")
}

/// Path for a game's background compaction lock file.
pub fn lock_file_path(data_dir: &Path, alias: &str) -> PathBuf {
    data_dir.join(alias).join("compact.lock")
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn target_parse_roundtrip() {
        for s in ["game", "data", "both"] {
            let t = TargetFolder::parse(s).unwrap();
            assert_eq!(t.as_str(), s);
        }
    }

    #[test]
    fn target_aliases() {
        assert_eq!(TargetFolder::parse("GameDir").unwrap(), TargetFolder::GameDir);
        assert_eq!(TargetFolder::parse("objects").unwrap(), TargetFolder::DataDir);
        assert_eq!(TargetFolder::parse("ALL").unwrap(), TargetFolder::Both);
    }

    #[test]
    fn target_invalid() {
        assert!(TargetFolder::parse("network").is_err());
    }

    #[test]
    fn phase_roundtrip() {
        for p in [CompactPhase::Starting, CompactPhase::Running,
                  CompactPhase::Paused, CompactPhase::Done, CompactPhase::Failed] {
            assert_eq!(CompactPhase::parse(p.as_str()), p);
        }
    }

    #[test]
    fn state_save_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("compact.state");
        let mut s = CompactState::new(CompactAlgorithm::Lzx, TargetFolder::GameDir);
        s.total_files = 100;
        s.processed_files = 30;
        s.phase = CompactPhase::Running.as_str().to_string();
        s.save(&path).unwrap();
        let loaded = CompactState::load(&path).unwrap().unwrap();
        assert_eq!(loaded.total_files, 100);
        assert_eq!(loaded.processed_files, 30);
        assert_eq!(loaded.phase(), CompactPhase::Running);
        assert_eq!(loaded.algorithm, "lzx");
        assert_eq!(loaded.target, "game");
    }

    #[test]
    fn load_missing_returns_none() {
        let p = PathBuf::from("/this/does/not/exist/compact.state");
        assert!(matches!(CompactState::load(&p), Ok(None)));
    }

    #[test]
    fn state_paths_under_data_dir() {
        let dd = PathBuf::from("data");
        assert_eq!(state_file_path(&dd, "cp2077"), PathBuf::from("data/cp2077/compact.state"));
        assert_eq!(lock_file_path(&dd, "cp2077"), PathBuf::from("data/cp2077/compact.lock"));
    }
}
