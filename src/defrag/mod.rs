//! `gim defrag` — NTFS defragmentation for game folders (HDD only).
//!
//! Windows exposes the building blocks of `defrag.exe` / third-party
//! optimizers as `FSCTL_*` control codes via `DeviceIoControl`. This module
//! drives them directly (raw FFI, no shell-out) so the operation is fast,
//! observable, and fails safe.
//!
//! # Workflow (7 stages — see [`crate::commands::defrag`])
//!
//! 1. **Admin check** — every FSCTL used here requires elevation.
//! 2. **Media detection** — SSD → optional TRIM, exit. HDD → continue.
//! 3. **Targeted fragmentation analysis** — only game files (`.pak`, `.bin`,
//!    `.vpk`, …), via `FSCTL_GET_RETRIEVAL_POINTERS` (VCN/LCN extents).
//! 4. **Volume bitmap scan** — `FSCTL_GET_VOLUME_BITMAP` (chunked, low RAM),
//!    find contiguous free regions at the lowest LCNs (outer HDD tracks).
//! 5. **Safety validation** — ≥15% free space, no locks, target region big
//!    enough, file attribute filter (skip COMPRESSED/ENCRYPTED/SPARSE).
//! 6. **Defrag engine** — `FSCTL_MOVE_FILE` per file, kernel-atomic.
//! 7. **Consolidation** — move defragmented files into the fast zone.
//!
//! # Why we never `copy + delete`
//!
//! `FSCTL_MOVE_FILE` is kernel-atomic: NTFS updates the MFT in one
//! transaction. If power is lost mid-move, Windows rolls back to the
//! previous MFT state — no corruption. Manual `Read → Write → Delete` would
//! lose data on interruption, so we never do it.
//!
//! # Platform support
//!
//! Windows-only. Every public entry point returns
//! [`crate::error::GError::NotSupportedPlatform`] elsewhere.

pub mod admin;
pub mod bitmap;
pub mod file_map;
pub mod media;
pub mod move_file;
pub mod plan;
pub mod safety;
pub mod throttle;
pub mod volume;

pub use admin::{is_elevated, request_elevation, ElevationToken};
pub use bitmap::{ContiguousRun, FreeRegion, VolumeBitmap, VolumeBitmapRuns, bitmap_chunk, scan_all_free_regions, is_lcn_free};
pub use file_map::{FileExtent, FileMap, FragmentationStats, analyze_fragmentation};
pub use media::{MediaKind, detect_media_kind};
pub use move_file::{MoveOutcome, MoveRequest, execute_move, verify_target_free};
pub use plan::{DefragPlan, PlanError, PlannedFile, PlannedMove, SkipReason, build_plan};
pub use safety::{FileAttrs, FileSafety, check_file_safety, check_locked, read_file_attrs};
pub use throttle::IoThrottle;
pub use volume::{VolumeHandle, VolumeInfo, open_volume, query_volume_info, detect_vss_active};

use crate::error::{GError, GResult};
use std::path::{Path, PathBuf};

/// Which folder(s) to defragment for a game. Mirrors `compact::TargetFolder`
/// so config / CLI flags stay consistent across both commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFolder {
    GameDir,
    DataDir,
    Both,
}

impl TargetFolder {
    pub fn parse(s: &str) -> GResult<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "game" | "game_dir" | "gamedir" => Ok(Self::GameDir),
            "data" | "data_dir" | "datadir" | "objects" => Ok(Self::DataDir),
            "both" | "all" => Ok(Self::Both),
            other => Err(GError::Other(format!(
                "unknown defrag target \"{other}\" (supported: game, data, both)"
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

/// Full options resolved for a defrag run (CLI merged with config).
///
/// All thresholds have defaults that match the project's robustness rules
/// (see instruction.md):
/// - `min_free_pct = 15` — refuse to run below 15% free space
/// - `fragment_threshold_pct = 5` — skip files whose extent ratio < 5%
/// - `max_extents_per_file = 20` — NTFS attribute-list hard limit safety
/// - `throttle_mb = 500` — sleep after every 500 MB of cluster moves
/// - `throttle_sleep_ms = 200` — sleep duration after the throttle budget
#[derive(Debug, Clone)]
pub struct DefragOptions {
    pub target: TargetFolder,
    pub threads: usize,
    pub exclude: Vec<String>,
    pub force: bool,
    pub confirm: bool,
    pub dry_run: bool,
    /// Skip SSDs and refuse to run on them unless `--force` is set.
    /// Even with `--force`, SSDs only get TRIM (no cluster moves).
    pub allow_ssd: bool,
    /// Hard floor on free space as percentage of total volume size.
    pub min_free_pct: u8,
    /// Skip files whose extent/size ratio is below this percentage.
    pub fragment_threshold_pct: u8,
    /// Maximum extents allowed per file after defrag — NTFS attribute list
    /// limit. Files we can't squeeze under this are skipped, not failed.
    pub max_extents_per_file: u32,
    /// Move at most this many MB before yielding to other I/O.
    pub throttle_mb: u64,
    /// How long to sleep after the throttle budget is hit.
    pub throttle_sleep_ms: u64,
    /// Run consolidation phase (move defragmented files to outer tracks).
    pub consolidate: bool,
}

impl Default for DefragOptions {
    fn default() -> Self {
        Self {
            target: TargetFolder::GameDir,
            threads: 0,
            exclude: Vec::new(),
            force: false,
            confirm: false,
            dry_run: false,
            allow_ssd: false,
            min_free_pct: 15,
            fragment_threshold_pct: 5,
            max_extents_per_file: 20,
            throttle_mb: 500,
            throttle_sleep_ms: 200,
            consolidate: true,
        }
    }
}

// ── Background defrag state file ─────────────────────────────────────────
//
// Mirrors the compact state file pattern: JSON written to
// `data/[alias]/defrag.state` so a future `gim defrag --status` can report
// progress from a separate process. (For now we run foreground-only, but
// the struct is in place for the follow-up background worker.)

/// Lifecycle phase recorded in the state file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefragPhase {
    /// Stage 1 — admin check.
    Authorizing,
    /// Stage 2 — media detection.
    DetectingMedia,
    /// Stage 3 — analyzing game file fragmentation.
    Analyzing,
    /// Stage 4 — scanning NTFS volume bitmap for free regions.
    ScanningBitmap,
    /// Stage 5 — validating safety rules.
    Validating,
    /// Stage 6 — moving fragmented clusters (defrag engine).
    Defragmenting,
    /// Stage 7 — consolidating files into the fast zone.
    Consolidating,
    /// Finished (success).
    Done,
    /// Aborted with an error.
    Failed,
    /// User cancelled (Ctrl-C or "n" at prompt).
    Cancelled,
}

impl DefragPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Authorizing => "authorizing",
            Self::DetectingMedia => "detecting-media",
            Self::Analyzing => "analyzing",
            Self::ScanningBitmap => "scanning-bitmap",
            Self::Validating => "validating",
            Self::Defragmenting => "defragmenting",
            Self::Consolidating => "consolidating",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "authorizing" => Self::Authorizing,
            "detecting-media" => Self::DetectingMedia,
            "analyzing" => Self::Analyzing,
            "scanning-bitmap" => Self::ScanningBitmap,
            "validating" => Self::Validating,
            "defragmenting" => Self::Defragmenting,
            "consolidating" => Self::Consolidating,
            "done" => Self::Done,
            "cancelled" => Self::Cancelled,
            _ => Self::Failed,
        }
    }
}

/// Serializable snapshot of foreground/background defrag progress.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DefragState {
    pub phase: String,
    pub target: String,
    pub total_files: u64,
    pub processed_files: u64,
    pub defragged_files: u64,
    pub skipped_files: u64,
    pub failed_files: u64,
    pub total_extents_before: u64,
    pub total_extents_after: u64,
    pub bytes_moved: u64,
    pub started_at: i64,
    pub updated_at: i64,
    pub message: String,
}

impl DefragState {
    pub fn new(target: TargetFolder) -> Self {
        let now = unix_now();
        Self {
            phase: DefragPhase::Authorizing.as_str().to_string(),
            target: target.as_str().to_string(),
            total_files: 0,
            processed_files: 0,
            defragged_files: 0,
            skipped_files: 0,
            failed_files: 0,
            total_extents_before: 0,
            total_extents_after: 0,
            bytes_moved: 0,
            started_at: now,
            updated_at: now,
            message: String::new(),
        }
    }

    pub fn phase(&self) -> DefragPhase {
        DefragPhase::parse(&self.phase)
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

/// Path for a game's defrag state file.
pub fn state_file_path(data_dir: &Path, alias: &str) -> PathBuf {
    data_dir.join(alias).join("defrag.state")
}

/// Path for a game's defrag lock file.
pub fn lock_file_path(data_dir: &Path, alias: &str) -> PathBuf {
    data_dir.join(alias).join("defrag.lock")
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
        for p in [DefragPhase::Authorizing, DefragPhase::DetectingMedia,
                  DefragPhase::Analyzing, DefragPhase::ScanningBitmap,
                  DefragPhase::Validating, DefragPhase::Defragmenting,
                  DefragPhase::Consolidating, DefragPhase::Done,
                  DefragPhase::Failed, DefragPhase::Cancelled] {
            assert_eq!(DefragPhase::parse(p.as_str()), p);
        }
    }

    #[test]
    fn state_save_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("defrag.state");
        let mut s = DefragState::new(TargetFolder::GameDir);
        s.total_files = 200;
        s.processed_files = 75;
        s.defragged_files = 60;
        s.phase = DefragPhase::Defragmenting.as_str().to_string();
        s.save(&path).unwrap();
        let loaded = DefragState::load(&path).unwrap().unwrap();
        assert_eq!(loaded.total_files, 200);
        assert_eq!(loaded.processed_files, 75);
        assert_eq!(loaded.defragged_files, 60);
        assert_eq!(loaded.phase(), DefragPhase::Defragmenting);
        assert_eq!(loaded.target, "game");
    }

    #[test]
    fn load_missing_returns_none() {
        let p = PathBuf::from("/this/does/not/exist/defrag.state");
        assert!(matches!(DefragState::load(&p), Ok(None)));
    }

    #[test]
    fn state_paths_under_data_dir() {
        let dd = PathBuf::from("data");
        assert_eq!(state_file_path(&dd, "cp2077"), PathBuf::from("data/cp2077/defrag.state"));
        assert_eq!(lock_file_path(&dd, "cp2077"), PathBuf::from("data/cp2077/defrag.lock"));
    }

    #[test]
    fn defaults_match_instruction_rules() {
        let o = DefragOptions::default();
        // Hard thresholds from instruction.md.
        assert_eq!(o.min_free_pct, 15, "15% free space rule");
        assert_eq!(o.fragment_threshold_pct, 5, "skip files <5% fragmented");
        assert!(o.max_extents_per_file <= 20, "MFT attribute list safety");
        assert!(o.max_extents_per_file >= 10, "don't be too aggressive");
        assert_eq!(o.throttle_mb, 500, "throttle after 500 MB");
    }
}
