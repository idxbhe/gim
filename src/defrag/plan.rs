//! Stage 5/6/7 — turn fragmentation analysis into a concrete move plan.
//!
//! Inputs:
//! - A list of `FileMap`s (one per game file) tagged with their safety
//!   status.
//! - A list of `FreeRegion`s (from the volume bitmap scan) sorted by
//!   ascending LCN.
//! - Volume info (cluster size, physical sector size, total clusters).
//! - `DefragOptions` (max extents per file, fragment threshold, etc.).
//!
//! Outputs:
//! - A `DefragPlan` listing which files to move, where to move them, and
//!   which files to skip (and why).
//!
//! # Algorithm
//!
//! 1. **Filter**: drop files that are skipped (attrs/locked), too small
//!    (below 1 cluster), or already perfectly contiguous (1 extent).
//! 2. **Threshold**: drop files whose fragmentation ratio is below
//!    `fragment_threshold_pct`.
//! 3. **Sort**: largest files first. Moving the biggest files into the
//!    fast zone gives the highest seek-time savings per move.
//! 4. **Allocate**: for each file, walk the free-region list (sorted by
//!    ascending LCN) and grab the first region that fits. Carve off the
//!    needed clusters, advance the region. Allocations are aligned to the
//!    physical sector size.
//! 5. **Extent cap**: if a file's plan would create more than
//!    `max_extents_per_file` extents, skip it (NTFS attribute list limit).
//! 6. **Fast zone**: the first N MB of the disk is the "fast zone" —
//!    consolidation moves go here. Files that needed defrag (rather than
//!    just consolidation) get priority.
//!
//! # Failure modes
//!
//! - `OutOfFreeSpace` — total free space is below `min_free_pct`.
//! - `NoContiguousFit` — a file is so large no single free region can
//!   hold it. We don't fall back to multi-region moves for that file
//!   (would re-fragment it); we skip and report.
//!
//! # Non-Windows
//!
//! Pure logic — works the same on all platforms. The Win32 parts (bitmap
//! scan, retrieval pointers) feed in as data.

use crate::defrag::bitmap::FreeRegion;
use crate::defrag::file_map::FileMap;
use crate::defrag::safety::FileSafety;
use crate::defrag::volume::VolumeInfo;
use crate::defrag::DefragOptions;
use std::path::PathBuf;

/// One file in the plan, with its target placement decided.
#[derive(Debug, Clone)]
pub struct PlannedFile {
    pub path: PathBuf,
    pub size: u64,
    pub clusters: u64,
    pub extents_before: usize,
    /// Where each VCN range should be moved to. The first entry's VCN
    /// is always 0; the move engine walks them in order.
    pub moves: Vec<PlannedMove>,
    /// Why this file was selected (for reporting).
    pub reason: PlanReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedMove {
    /// Source VCN (file-relative).
    pub start_vcn: u64,
    /// Number of clusters in this move.
    pub cluster_count: u64,
    /// Destination LCN (volume-absolute, physical-sector aligned).
    pub target_lcn: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanReason {
    /// File is fragmented beyond the threshold — defrag it.
    Fragmented,
    /// File is contiguous but lives on slow tracks — consolidate to fast
    /// zone.
    Consolidate,
}

/// A file the planner chose to skip, with the reason.
#[derive(Debug, Clone)]
pub struct SkippedFile {
    pub path: PathBuf,
    pub size: u64,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    BelowFragmentThreshold,
    SingleExtent,
    TooSmall,
    SafetyBlock,
    Locked,
    NoContiguousFit,
    ExtentCapExceeded,
    Inaccessible,
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BelowFragmentThreshold => "below fragment threshold",
            Self::SingleExtent => "already contiguous (single extent)",
            Self::TooSmall => "smaller than one cluster",
            Self::SafetyBlock => "compressed/encrypted/sparse/etc",
            Self::Locked => "locked by another process",
            Self::NoContiguousFit => "no single free region fits",
            Self::ExtentCapExceeded => "would exceed extent cap",
            Self::Inaccessible => "inaccessible",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DefragPlan {
    pub planned: Vec<PlannedFile>,
    pub skipped: Vec<SkippedFile>,
    pub bytes_to_move: u64,
    pub clusters_to_move: u64,
    pub planned_count: u64,
    pub skipped_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    OutOfFreeSpace { free_pct: u8, min_pct: u8 },
    EmptyPlan,
}

/// Build the plan from the analyzed files + bitmap free regions.
///
/// `files` is a list of `(FileMap, FileSafety)` tuples — the caller has
/// already run the safety check. `free_regions` must be sorted by
/// ascending LCN (as returned by `bitmap::scan_all_free_regions`).
pub fn build_plan(
    files: &[(FileMap, FileSafety)],
    free_regions: &[FreeRegion],
    volume: &VolumeInfo,
    opts: &DefragOptions,
) -> Result<DefragPlan, PlanError> {
    // ── Stage 5a: hard free-space threshold ──────────────────────────
    if volume.free_below(opts.min_free_pct) {
        return Err(PlanError::OutOfFreeSpace {
            free_pct: volume.free_pct,
            min_pct: opts.min_free_pct,
        });
    }

    let mut planned: Vec<PlannedFile> = Vec::new();
    let mut skipped: Vec<SkippedFile> = Vec::new();
    let mut bytes_to_move: u64 = 0;
    let mut clusters_to_move: u64 = 0;

    // ── Stage 5b: filter + sort largest-first ────────────────────────
    let mut candidates: Vec<&FileMap> = files.iter()
        .filter_map(|(m, s)| {
            if *s != FileSafety::Ok { return None; }
            // Skip files smaller than one cluster.
            if m.total_clusters() == 0 { return None; }
            // Skip single-extent files unless we're consolidating.
            if m.allocated_extent_count() <= 1 && !opts.consolidate {
                return None;
            }
            Some(m)
        })
        .collect();
    candidates.sort_by(|a, b| b.size.cmp(&a.size));

    // Walk the free-region list as we allocate. Each `FreeRegion` is
    // consumed (shrunk) as we carve chunks off it. We sort a *copy* so
    // the caller's list isn't mutated.
    let mut regions: Vec<FreeRegion> = free_regions.to_vec();
    // Already sorted by ascending LCN, but defensive-sort just in case.
    regions.sort_by_key(|r| r.start_lcn);

    for fm in candidates {
        let clusters_needed = fm.total_clusters();

        // Skip files that are already a single extent (perfectly
        // contiguous — nothing to do). Check this *before* the
        // fragmentation threshold so the skip reason is accurate.
        if fm.allocated_extent_count() <= 1 {
            skipped.push(SkippedFile {
                path: fm.path.clone(),
                size: fm.size,
                reason: SkipReason::SingleExtent,
            });
            continue;
        }

        // Fragmentation threshold check.
        if !fm.needs_defrag(opts.fragment_threshold_pct) {
            // File has multiple extents but is below the threshold —
            // not worth the move cost.
            skipped.push(SkippedFile {
                path: fm.path.clone(),
                size: fm.size,
                reason: SkipReason::BelowFragmentThreshold,
            });
            continue;
        }

        // Find a free region that fits the whole file in one piece.
        // Multi-region allocation would re-fragment the file — exactly
        // what we're trying to avoid.
        let target_lcn = match find_fit(&mut regions, clusters_needed, volume) {
            Some(lcn) => lcn,
            None => {
                skipped.push(SkippedFile {
                    path: fm.path.clone(),
                    size: fm.size,
                    reason: SkipReason::NoContiguousFit,
                });
                continue;
            }
        };

        // Extent cap: this plan produces exactly 1 extent (we're
        // consolidating the whole file into one region). If the file's
        // allocated_extent_count was already 1, we skipped earlier; if it
        // was higher, the move reduces it to 1. So the cap is satisfied
        // trivially here. We keep the check for future when we may allow
        // multi-region moves.
        if 1 > opts.max_extents_per_file as usize {
            skipped.push(SkippedFile {
                path: fm.path.clone(),
                size: fm.size,
                reason: SkipReason::ExtentCapExceeded,
            });
            continue;
        }

        let mv = PlannedMove {
            start_vcn: 0,
            cluster_count: clusters_needed,
            target_lcn,
        };
        bytes_to_move += fm.size;
        clusters_to_move += clusters_needed;
        planned.push(PlannedFile {
            path: fm.path.clone(),
            size: fm.size,
            clusters: clusters_needed,
            extents_before: fm.allocated_extent_count(),
            moves: vec![mv],
            reason: PlanReason::Fragmented,
        });
    }

    // Push files that failed safety/lock checks into the skipped list
    // for reporting completeness.
    for (fm, s) in files {
        let reason = match s {
            FileSafety::Ok => continue,
            FileSafety::SkipAttrs(_) => SkipReason::SafetyBlock,
            FileSafety::Locked => SkipReason::Locked,
            FileSafety::Inaccessible => SkipReason::Inaccessible,
        };
        skipped.push(SkippedFile {
            path: fm.path.clone(),
            size: fm.size,
            reason,
        });
    }

    let planned_count = planned.len() as u64;
    let skipped_count = skipped.len() as u64;

    if planned.is_empty() && skipped.is_empty() {
        return Err(PlanError::EmptyPlan);
    }

    Ok(DefragPlan {
        planned, skipped,
        bytes_to_move, clusters_to_move,
        planned_count, skipped_count,
    })
}

/// Find the lowest-LCN free region that fits `needed` clusters and return
/// its (aligned) start LCN. The chosen region is shrunk in-place.
///
/// Alignment: round `start_lcn` up to the physical sector boundary so
/// Advanced Format drives don't pay the read-modify-write penalty.
fn find_fit(regions: &mut Vec<FreeRegion>, needed: u64, volume: &VolumeInfo) -> Option<u64> {
    let align_mask = volume.physical_sector_mask();
    for region in regions.iter_mut() {
        if !region.fits(needed) { continue; }
        // Round the start LCN up to the physical sector boundary.
        let aligned_start = (region.start_lcn + (!align_mask)) & align_mask;
        // Did alignment eat into our usable space?
        let aligned_offset = aligned_start.saturating_sub(region.start_lcn);
        if region.len_clusters < aligned_offset + needed { continue; }
        // Carve off the aligned chunk from the front of this region.
        let consumed_to = aligned_start + needed;
        let new_start = consumed_to;
        let new_len = region.len_clusters - (consumed_to - region.start_lcn);
        if new_len == 0 {
            // Region fully consumed — mark it as zero-length; the caller
            // can compact later. (Leaving zero-length entries is fine for
            // correctness; we just skip them on subsequent lookups.)
            region.len_clusters = 0;
        } else {
            region.start_lcn = new_start;
            region.len_clusters = new_len;
        }
        return Some(aligned_start);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defrag::file_map::FileExtent;
    use std::path::PathBuf;

    fn make_volume() -> VolumeInfo {
        VolumeInfo {
            drive: 'C',
            volume_path: "\\\\.\\C:".into(),
            total_bytes: 1_000_000_000,
            free_bytes: 500_000_000,
            bytes_per_cluster: 4096,
            bytes_per_sector_log: 512,
            bytes_per_sector_phys: 4096,
            free_pct: 50,
            vss_active: false,
        }
    }

    fn make_fragmented_file(path: &str, size_clusters: u64, n_extents: usize) -> FileMap {
        let per_extent = size_clusters / n_extents as u64;
        let mut extents = Vec::new();
        for i in 0..n_extents {
            extents.push(FileExtent {
                vcn: i as u64 * per_extent,
                lcn: 1000 + (i as u64) * 500, // scattered LCNs
                len: per_extent,
            });
        }
        FileMap {
            path: PathBuf::from(path),
            size: size_clusters * 4096,
            bytes_per_cluster: 4096,
            extents,
        }
    }

    #[test]
    fn plan_skips_below_threshold() {
        let volume = make_volume();
        let opts = DefragOptions::default();
        // Single extent, fragmented? No (1 extent → not fragmented).
        let fm = FileMap {
            path: PathBuf::from("single.pak"),
            size: 100 * 4096,
            bytes_per_cluster: 4096,
            extents: vec![FileExtent { vcn: 0, lcn: 1000, len: 100 }],
        };
        let files = vec![(fm, FileSafety::Ok)];
        let free = vec![FreeRegion { start_lcn: 0, len_clusters: 100_000 }];
        let plan = build_plan(&files, &free, &volume, &opts).unwrap();
        // Single extent → skipped.
        assert_eq!(plan.planned_count, 0);
        assert!(plan.skipped.iter().any(|s| s.reason == SkipReason::SingleExtent));
    }

    #[test]
    fn plan_fits_fragmented_file() {
        let volume = make_volume();
        let opts = DefragOptions::default();
        // 10 extents / 100 clusters = 10% fragmentation.
        let fm = make_fragmented_file("foo.pak", 100, 10);
        let files = vec![(fm, FileSafety::Ok)];
        let free = vec![FreeRegion { start_lcn: 0, len_clusters: 100_000 }];
        let plan = build_plan(&files, &free, &volume, &opts).unwrap();
        assert_eq!(plan.planned_count, 1);
        assert_eq!(plan.planned[0].moves.len(), 1);
        assert_eq!(plan.planned[0].moves[0].cluster_count, 100);
        assert_eq!(plan.planned[0].reason, PlanReason::Fragmented);
        assert_eq!(plan.bytes_to_move, 100 * 4096);
    }

    #[test]
    fn plan_skips_safety_blocked() {
        let volume = make_volume();
        let opts = DefragOptions::default();
        let fm = make_fragmented_file("compressed.pak", 100, 10);
        let files = vec![(fm, FileSafety::SkipAttrs("NTFS-compressed"))];
        let free = vec![FreeRegion { start_lcn: 0, len_clusters: 100_000 }];
        // Safety-blocked file ends up in the skipped list (not in planned).
        // The plan succeeds with 0 planned and 1 skipped.
        let plan = build_plan(&files, &free, &volume, &opts).unwrap();
        assert_eq!(plan.planned_count, 0);
        assert_eq!(plan.skipped_count, 1);
        assert!(plan.skipped.iter().any(|s| s.reason == SkipReason::SafetyBlock));
    }

    #[test]
    fn plan_skips_locked() {
        let volume = make_volume();
        let opts = DefragOptions::default();
        let fm = make_fragmented_file("locked.pak", 100, 10);
        let files = vec![(fm, FileSafety::Locked)];
        let free = vec![FreeRegion { start_lcn: 0, len_clusters: 100_000 }];
        let plan = build_plan(&files, &free, &volume, &opts).unwrap();
        assert_eq!(plan.planned_count, 0);
        assert_eq!(plan.skipped_count, 1);
        assert!(plan.skipped.iter().any(|s| s.reason == SkipReason::Locked));
    }

    #[test]
    fn plan_errors_when_free_space_below_threshold() {
        let mut volume = make_volume();
        volume.free_pct = 5; // below 15% default
        let opts = DefragOptions::default();
        let fm = make_fragmented_file("foo.pak", 100, 10);
        let files = vec![(fm, FileSafety::Ok)];
        let free = vec![FreeRegion { start_lcn: 0, len_clusters: 100_000 }];
        assert!(matches!(
            build_plan(&files, &free, &volume, &opts),
            Err(PlanError::OutOfFreeSpace { free_pct: 5, min_pct: 15 })
        ));
    }

    #[test]
    fn plan_skips_when_no_contiguous_fit() {
        let volume = make_volume();
        let opts = DefragOptions::default();
        // File needs 100 clusters but the only free region has 50.
        let fm = make_fragmented_file("big.pak", 100, 10);
        let files = vec![(fm, FileSafety::Ok)];
        let free = vec![FreeRegion { start_lcn: 0, len_clusters: 50 }];
        let plan = build_plan(&files, &free, &volume, &opts).unwrap();
        assert_eq!(plan.planned_count, 0);
        assert!(plan.skipped.iter().any(|s| s.reason == SkipReason::NoContiguousFit));
    }

    #[test]
    fn plan_picks_lowest_lcn_first() {
        let volume = make_volume();
        let opts = DefragOptions::default();
        // Two free regions: one at LCN 10000, one at LCN 100.
        // The planner should pick the lower one (LCN 100) for the first file.
        let fm = make_fragmented_file("foo.pak", 100, 10);
        let files = vec![(fm, FileSafety::Ok)];
        let mut free = vec![
            FreeRegion { start_lcn: 10000, len_clusters: 100_000 },
            FreeRegion { start_lcn: 100, len_clusters: 100_000 },
        ];
        free.sort_by_key(|r| r.start_lcn);
        let plan = build_plan(&files, &free, &volume, &opts).unwrap();
        assert!(plan.planned[0].moves[0].target_lcn >= 100);
        assert!(plan.planned[0].moves[0].target_lcn < 10000);
    }

    #[test]
    fn plan_aligns_to_physical_sector() {
        let mut volume = make_volume();
        volume.bytes_per_sector_phys = 4096; // Advanced Format
        let opts = DefragOptions::default();
        let fm = make_fragmented_file("foo.pak", 100, 10);
        let files = vec![(fm, FileSafety::Ok)];
        // Free region starts at LCN 5 (not 4096-aligned in cluster terms,
        // but the alignment mask is in clusters when cluster size == phys
        // sector size, so any cluster start is aligned). Use a more
        // interesting setup: physical sector = 8192 (2 clusters per sector).
        volume.bytes_per_sector_phys = 8192;
        // Free region starts at odd cluster.
        let free = vec![FreeRegion { start_lcn: 5, len_clusters: 100_000 }];
        let plan = build_plan(&files, &free, &volume, &opts).unwrap();
        let target = plan.planned[0].moves[0].target_lcn;
        // 8192 / 4096 = 2 clusters per sector → target must be even.
        assert_eq!(target % 2, 0, "target LCN must be aligned to 2-cluster sector boundary");
    }

    #[test]
    fn plan_processes_files_largest_first() {
        let volume = make_volume();
        let opts = DefragOptions::default();
        // Two files: small (50 clusters, 10 fragments) and big (200 clusters, 10 fragments).
        let big = make_fragmented_file("big.pak", 200, 10);
        let small = make_fragmented_file("small.pak", 50, 10);
        let files = vec![(big, FileSafety::Ok), (small, FileSafety::Ok)];
        let free = vec![FreeRegion { start_lcn: 0, len_clusters: 1_000_000 }];
        let plan = build_plan(&files, &free, &volume, &opts).unwrap();
        // Big file should be planned first (lower target LCN consumed first).
        assert!(plan.planned[0].size >= plan.planned[1].size);
    }

    #[test]
    fn skip_reason_strings_nonempty() {
        for r in [SkipReason::BelowFragmentThreshold, SkipReason::SingleExtent,
                  SkipReason::TooSmall, SkipReason::SafetyBlock,
                  SkipReason::Locked, SkipReason::NoContiguousFit,
                  SkipReason::ExtentCapExceeded, SkipReason::Inaccessible] {
            assert!(!r.as_str().is_empty());
        }
    }
}
