//! Stage 3 — analyze fragmentation of individual game files.
//!
//! We don't scan the whole volume. Instead we walk the game folder, open
//! each candidate file, and call `FSCTL_GET_RETRIEVAL_POINTERS`. That
//! returns a list of `(VCN, LCN)` extents describing where the file's
//! virtual clusters live physically on disk.
//!
//! # Fragmentation metric
//!
//! A perfectly contiguous file has 1 extent (covers VCNs 0..N at LCNs
//! L..L+N). The more extents, the more fragmented. We compute:
//!
//! - `extent_count` — how many `(VCN, LCN)` runs the file occupies.
//! - `fragmentation_ratio` — `extent_count / (file_size_in_clusters)`.
//!   A 100-cluster file in 5 fragments has ratio 0.05 (5%).
//! - `is_fragmented(threshold_pct)` — true when `ratio * 100 >= threshold`.
//!
//! The 5% threshold from instruction.md means a 1 GB file with >50
//! extents is worth defragmenting; fewer than that and we skip it (the
//! MFT and seek-time cost of moving outweighs the gain).
//!
//! # Non-Windows
//!
//! `analyze_fragmentation` returns an empty `FileMap` for every file.
//! The command dispatcher gates the whole flow on `target_os = "windows"`
//! anyway via `NotSupportedPlatform` errors.

use crate::error::{GError, GResult};
use std::path::{Path, PathBuf};

/// One `(VCN, LCN)` run inside a file.
///
/// - `vcn` — Virtual Cluster Number (file-relative offset, in clusters).
/// - `lcn` — Logical Cluster Number (volume-absolute physical location).
/// - `len` — Run length in clusters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileExtent {
    pub vcn: u64,
    pub lcn: u64,
    pub len: u64,
}

impl FileExtent {
    /// `true` if this extent is a "sparse hole" (no physical cluster
    /// allocated). NTFS reports these with `LCN == -1` (`u64::MAX`).
    pub fn is_sparse(&self) -> bool {
        self.lcn == u64::MAX
    }

    /// `true` if this extent is contiguous with `next` — i.e. this run's
    /// end LCN is immediately followed by `next`'s start LCN.
    pub fn contiguous_with(&self, next: &FileExtent) -> bool {
        if self.is_sparse() || next.is_sparse() { return false; }
        self.lcn + self.len == next.lcn
    }
}

/// The full VCN→LCN map for one file.
#[derive(Debug, Clone)]
pub struct FileMap {
    pub path: PathBuf,
    pub size: u64,
    pub bytes_per_cluster: u64,
    pub extents: Vec<FileExtent>,
}

impl FileMap {
    /// Total clusters the file occupies (size / cluster, rounded up).
    pub fn total_clusters(&self) -> u64 {
        if self.bytes_per_cluster == 0 { 0 }
        else { self.size.div_ceil(self.bytes_per_cluster) }
    }

    /// Number of non-sparse extents (i.e. allocated physical runs).
    pub fn allocated_extent_count(&self) -> usize {
        self.extents.iter().filter(|e| !e.is_sparse()).count()
    }

    /// Fragmentation ratio = allocated_extents / total_clusters.
    /// 1 extent for a 100-cluster file → 0.01 (perfectly contiguous).
    /// 50 extents for a 100-cluster file → 0.50 (heavily fragmented).
    pub fn fragmentation_ratio(&self) -> f64 {
        let total = self.total_clusters();
        if total == 0 { return 0.0; }
        self.allocated_extent_count() as f64 / total as f64
    }

    /// `true` when the fragmentation ratio exceeds `threshold_pct`.
    pub fn is_fragmented(&self, threshold_pct: u8) -> bool {
        self.fragmentation_ratio() * 100.0 >= threshold_pct as f64
    }

    /// Lowest LCN occupied by any non-sparse extent of this file.
    /// Useful for "where does this file physically live?" reporting.
    pub fn lowest_lcn(&self) -> Option<u64> {
        self.extents.iter()
            .filter(|e| !e.is_sparse())
            .map(|e| e.lcn)
            .min()
    }

    /// Highest LCN + 1 occupied by any non-sparse extent.
    pub fn highest_lcn_exclusive(&self) -> Option<u64> {
        self.extents.iter()
            .filter(|e| !e.is_sparse())
            .map(|e| e.lcn + e.len)
            .max()
    }

    /// Would this file benefit from defragmentation?
    ///
    /// We say "yes" when:
    /// - allocated extents > 1 (more than one physical run), AND
    /// - fragmentation ratio meets the threshold.
    pub fn needs_defrag(&self, threshold_pct: u8) -> bool {
        self.allocated_extent_count() > 1 && self.is_fragmented(threshold_pct)
    }
}

/// Aggregated stats for a folder of game files. Reported to the user
/// before the yes/no prompt.
#[derive(Debug, Clone, Default)]
pub struct FragmentationStats {
    pub total_files: u64,
    pub analyzed_files: u64,
    pub fragmented_files: u64,
    pub skipped_locked: u64,
    pub skipped_attrs: u64,
    pub skipped_too_small: u64,
    pub total_size: u64,
    pub fragmented_size: u64,
    pub total_extents: u64,
    pub max_extents_seen: u64,
}

impl FragmentationStats {
    pub fn merge(&mut self, other: &FragmentationStats) {
        self.total_files += other.total_files;
        self.analyzed_files += other.analyzed_files;
        self.fragmented_files += other.fragmented_files;
        self.skipped_locked += other.skipped_locked;
        self.skipped_attrs += other.skipped_attrs;
        self.skipped_too_small += other.skipped_too_small;
        self.total_size += other.total_size;
        self.fragmented_size += other.fragmented_size;
        self.total_extents += other.total_extents;
        self.max_extents_seen = self.max_extents_seen.max(other.max_extents_seen);
    }
}

// ── Win32 FFI ────────────────────────────────────────────────────────────
#[cfg(target_os = "windows")]
const GENERIC_READ: u32 = 0x80000000;
#[cfg(target_os = "windows")]
const FILE_SHARE_READ: u32 = 0x00000001;
#[cfg(target_os = "windows")]
const FILE_SHARE_WRITE: u32 = 0x00000002;
#[cfg(target_os = "windows")]
const OPEN_EXISTING: u32 = 3;
#[cfg(target_os = "windows")]
const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = -1isize as *mut std::ffi::c_void;

// FSCTL_GET_RETRIEVAL_POINTERS = CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 28,
//                                         METHOD_NEITHER, FILE_ANY_ACCESS)
//                                          = 0x000900B4
#[cfg(target_os = "windows")]
const FSCTL_GET_RETRIEVAL_POINTERS: u32 = 0x000900B4;

/// `STARTING_VCN_INPUT_BUFFER` — a single u64 VCN where the enumeration
/// should start. We always pass 0 to enumerate the whole file.
#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct StartingVcnInputBuffer {
    starting_vcn: u64,
}

/// `RETRIEVAL_POINTERS_BUFFER` — variable-size struct returned by the
/// FSCTL. We read the fixed header then walk the `Extents[]` array with
/// pointer arithmetic.
#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct RetrievalPointersBufferHeader {
    extent_count: u32,
    starting_vcn: u64,
    // `Extents[]` follows here. Each element is two u64s: NextVcn, NextLcn.
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        lpfilename: *const u16,
        dwdesiredaccess: u32,
        dwsharemode: u32,
        lpsecurityattributes: *mut std::ffi::c_void,
        dwcreationdisposition: u32,
        dwflagsandattributes: u32,
        htemplatefile: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;

    fn DeviceIoControl(
        hdevice: *mut std::ffi::c_void,
        dwiocontrolcode: u32,
        lpinbuffer: *const std::ffi::c_void,
        ninbuffersize: u32,
        lpoutbuffer: *mut std::ffi::c_void,
        noutbuffersize: u32,
        lpbytesreturned: *mut u32,
        lpoverlapped: *mut std::ffi::c_void,
    ) -> i32;

    fn CloseHandle(hobject: *mut std::ffi::c_void) -> i32;
}

#[cfg(target_os = "windows")]
struct HandleGuard(*mut std::ffi::c_void);

#[cfg(target_os = "windows")]
impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe { let _ = CloseHandle(self.0); }
        }
    }
}

/// Analyze a single file: open it, query its VCN/LCN map, return a
/// `FileMap`.
pub fn analyze_fragmentation(path: &Path, bytes_per_cluster: u64) -> GResult<FileMap> {
    let size = std::fs::symlink_metadata(path).map(|m| m.len()).unwrap_or(0);
    let extents = query_retrieval_pointers(path)?;

    Ok(FileMap {
        path: path.to_path_buf(),
        size,
        bytes_per_cluster,
        extents,
    })
}

#[cfg(target_os = "windows")]
fn query_retrieval_pointers(path: &Path) -> GResult<Vec<FileExtent>> {
    use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

    let mut wide: Vec<u16> = OsStr::new(path).encode_wide().collect();
    wide.push(0);

    // Open with read-only + broad sharing. Files locked by a running game
    // will fail here — that's exactly the safety we want.
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | 0x0004 /* DELETE */,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        return Err(GError::Defrag(format!(
            "open {} for retrieval pointers failed (Win32 error {})",
            path.display(),
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        )));
    }
    let _guard = HandleGuard(h);

    // First call with a small buffer; grow on ERROR_MORE_DATA.
    let mut out_buf: Vec<u8> = vec![0u8; 4096];
    loop {
        let input = StartingVcnInputBuffer { starting_vcn: 0 };
        let mut returned: u32 = 0;

        let ok = unsafe {
            DeviceIoControl(
                h,
                FSCTL_GET_RETRIEVAL_POINTERS,
                &input as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<StartingVcnInputBuffer>() as u32,
                out_buf.as_mut_ptr() as *mut std::ffi::c_void,
                out_buf.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };

        if ok != 0 {
            break;
        }
        // ERROR_MORE_DATA (234) means the buffer was too small — grow and retry.
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if code == 234 {
            // Grow aggressively to avoid many round-trips on huge files.
            out_buf.resize(out_buf.len() * 2, 0);
            continue;
        }
        return Err(GError::Defrag(format!(
            "FSCTL_GET_RETRIEVAL_POINTERS on {} failed (Win32 error {code})",
            path.display()
        )));
    }

    // Parse the variable-size RETRIEVAL_POINTERS_BUFFER. The layout is:
    //   u32 ExtentCount
    //   u64 StartingVcn
    //   repeated { u64 NextVcn, u64 NextLcn } × ExtentCount
    //
    // Each extent describes: VCN range [prev_vcn, NextVcn) lives at LCN
    // NextLcn (or -1 = sparse). The first extent's prev_vcn is
    // StartingVcn from the header.
    if out_buf.len() < 12 {
        return Ok(Vec::new());
    }
    let extent_count = u32::from_le_bytes([out_buf[0], out_buf[1], out_buf[2], out_buf[3]]) as usize;
    let mut starting_vcn = u64::from_le_bytes([
        out_buf[4], out_buf[5], out_buf[6], out_buf[7],
        out_buf[8], out_buf[9], out_buf[10], out_buf[11],
    ]);

    // Sanity: ensure our buffer is big enough to hold all extents. Each
    // extent is 16 bytes; the header is 12. If the FSCTL reported more
    // extents than fit, that's a bug in our retry logic above — bail.
    let needed = 12 + extent_count * 16;
    if out_buf.len() < needed {
        return Err(GError::Defrag(format!(
            "retrieval pointer buffer truncated: have {} bytes, need {needed}",
            out_buf.len()
        )));
    }

    let mut extents: Vec<FileExtent> = Vec::with_capacity(extent_count);
    let mut prev_vcn = starting_vcn;
    for i in 0..extent_count {
        let off = 12 + i * 16;
        let next_vcn = u64::from_le_bytes([
            out_buf[off], out_buf[off + 1], out_buf[off + 2], out_buf[off + 3],
            out_buf[off + 4], out_buf[off + 5], out_buf[off + 6], out_buf[off + 7],
        ]);
        let next_lcn_raw = u64::from_le_bytes([
            out_buf[off + 8], out_buf[off + 9], out_buf[off + 10], out_buf[off + 11],
            out_buf[off + 12], out_buf[off + 13], out_buf[off + 14], out_buf[off + 15],
        ]);
        let len = next_vcn.saturating_sub(prev_vcn);
        extents.push(FileExtent {
            vcn: prev_vcn,
            lcn: next_lcn_raw,
            len,
        });
        prev_vcn = next_vcn;
    }

    Ok(extents)
}

#[cfg(not(target_os = "windows"))]
#[allow(non_snake_case)]
fn query_retrieval_pointers(path: &Path) -> GResult<Vec<FileExtent>> {
    let _ = path;
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_extent_detection() {
        let sparse = FileExtent { vcn: 0, lcn: u64::MAX, len: 10 };
        let real = FileExtent { vcn: 10, lcn: 1000, len: 50 };
        assert!(sparse.is_sparse());
        assert!(!real.is_sparse());
    }

    #[test]
    fn contiguous_check() {
        let a = FileExtent { vcn: 0, lcn: 100, len: 50 };
        let b = FileExtent { vcn: 50, lcn: 150, len: 30 };
        let c = FileExtent { vcn: 80, lcn: 200, len: 10 };
        assert!(a.contiguous_with(&b));
        assert!(!b.contiguous_with(&c)); // gap between 180 and 200
    }

    #[test]
    fn fragmentation_ratio_contiguous_file() {
        let map = FileMap {
            path: PathBuf::from("foo.pak"),
            size: 100 * 4096,
            bytes_per_cluster: 4096,
            extents: vec![FileExtent { vcn: 0, lcn: 1000, len: 100 }],
        };
        // 1 extent / 100 clusters = 0.01 → 1% → below 5% threshold.
        assert_eq!(map.allocated_extent_count(), 1);
        assert!((map.fragmentation_ratio() - 0.01).abs() < 1e-9);
        assert!(!map.is_fragmented(5));
        assert!(!map.needs_defrag(5));
    }

    #[test]
    fn fragmentation_ratio_fragmented_file() {
        let map = FileMap {
            path: PathBuf::from("bar.pak"),
            size: 100 * 4096,
            bytes_per_cluster: 4096,
            extents: (0..10).map(|i| FileExtent {
                vcn: i * 10, lcn: 1000 + i * 100, len: 10,
            }).collect(),
        };
        // 10 extents / 100 clusters = 0.10 → 10% → above 5% threshold.
        assert_eq!(map.allocated_extent_count(), 10);
        assert!((map.fragmentation_ratio() - 0.10).abs() < 1e-9);
        assert!(map.is_fragmented(5));
        assert!(map.needs_defrag(5));
    }

    #[test]
    fn fragmentation_ratio_with_sparse_extents() {
        let map = FileMap {
            path: PathBuf::from("sparse.bin"),
            size: 100 * 4096,
            bytes_per_cluster: 4096,
            extents: vec![
                FileExtent { vcn: 0, lcn: 100, len: 30 },
                FileExtent { vcn: 30, lcn: u64::MAX, len: 40 }, // sparse hole
                FileExtent { vcn: 70, lcn: 200, len: 30 },
            ],
        };
        // Only 2 allocated extents out of 100 clusters → 2% → not fragmented.
        assert_eq!(map.allocated_extent_count(), 2);
        assert!((map.fragmentation_ratio() - 0.02).abs() < 1e-9);
        assert!(!map.is_fragmented(5));
    }

    #[test]
    fn lowest_highest_lcn() {
        let map = FileMap {
            path: PathBuf::from("x.bin"),
            size: 300 * 4096,
            bytes_per_cluster: 4096,
            extents: vec![
                FileExtent { vcn: 0, lcn: 5000, len: 100 },
                FileExtent { vcn: 100, lcn: 2000, len: 100 },
                FileExtent { vcn: 200, lcn: 8000, len: 100 },
            ],
        };
        assert_eq!(map.lowest_lcn(), Some(2000));
        assert_eq!(map.highest_lcn_exclusive(), Some(8100));
    }

    #[test]
    fn needs_defrag_requires_multiple_extents() {
        let map = FileMap {
            path: PathBuf::from("single.pak"),
            size: 100 * 4096,
            bytes_per_cluster: 4096,
            extents: vec![FileExtent { vcn: 0, lcn: 1000, len: 100 }],
        };
        // Single extent → ratio is 1% but not fragmented (only 1 extent).
        assert!(!map.needs_defrag(5));
    }

    #[test]
    fn stats_merge_adds_counts() {
        let mut a = FragmentationStats {
            total_files: 10, analyzed_files: 8, fragmented_files: 4,
            skipped_locked: 1, skipped_attrs: 1, skipped_too_small: 0,
            total_size: 1_000_000, fragmented_size: 400_000,
            total_extents: 40, max_extents_seen: 12,
        };
        let b = FragmentationStats {
            total_files: 5, analyzed_files: 4, fragmented_files: 2,
            skipped_locked: 0, skipped_attrs: 1, skipped_too_small: 0,
            total_size: 500_000, fragmented_size: 200_000,
            total_extents: 18, max_extents_seen: 15,
        };
        a.merge(&b);
        assert_eq!(a.total_files, 15);
        assert_eq!(a.analyzed_files, 12);
        assert_eq!(a.fragmented_files, 6);
        assert_eq!(a.skipped_attrs, 2);
        assert_eq!(a.total_size, 1_500_000);
        assert_eq!(a.max_extents_seen, 15);
    }
}
