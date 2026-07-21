//! Stage 4 — scan the NTFS volume bitmap for contiguous free regions.
//!
//! `FSCTL_GET_VOLUME_BITMAP` returns a bitmap where each bit represents one
//! cluster on the volume: `1` = allocated, `0` = free. We use it to find
//! contiguous free runs at low LCNs (the outer tracks of an HDD, where
//! sequential reads are fastest).
//!
//! # RAM-safe chunking
//!
//! On a 4 TB HDD with 4 KB clusters the bitmap is 128 MB. Loading that
//! into RAM in one shot would balloon our memory footprint. Instead we
//! call `FSCTL_GET_VOLUME_BITMAP` repeatedly with increasing
//! `StartingLcn` values, processing one chunk at a time. The chunk size
//! is set so each call's bitmap fits in a few hundred KB.
//!
//! # Concurrency caveat (Double-Check Overwrite Guard)
//!
//! The bitmap is a *snapshot* — by the time we issue `FSCTL_MOVE_FILE`
//! the OS may have allocated our target clusters to another file. The
//! move engine performs a per-target re-check before each move; this
//! module is only the "planning" pass.
//!
//! # Non-Windows
//!
//! Returns empty results. The command dispatcher gates the whole flow.

use crate::error::{GError, GResult};

/// How many clusters one bitmap chunk covers.
/// buffer to ~256 KB which on a 4 KB-cluster volume covers ~2 million
/// clusters per call (≈8 GB of disk per chunk).
pub const BITMAP_CHUNK_CLUSTERS: u64 = 8 * 1024 * 1024; // 1 MB bitmap = 8 M clusters

/// One contiguous free run found in the volume bitmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeRegion {
    /// Starting LCN of the free run.
    pub start_lcn: u64,
    /// Length in clusters.
    pub len_clusters: u64,
}

impl FreeRegion {
    pub fn end_lcn_exclusive(&self) -> u64 { self.start_lcn + self.len_clusters }

    /// `true` if this region is large enough to hold `needed_clusters`
    /// contiguous.
    pub fn fits(&self, needed_clusters: u64) -> bool {
        self.len_clusters >= needed_clusters
    }

    /// Split off a sub-region of exactly `n` clusters from the start of
    /// this region. Returns `(allocated, remainder)`.
    pub fn split_at(&self, n: u64) -> (FreeRegion, Option<FreeRegion>) {
        if n >= self.len_clusters {
            (FreeRegion { start_lcn: self.start_lcn, len_clusters: self.len_clusters }, None)
        } else {
            (
                FreeRegion { start_lcn: self.start_lcn, len_clusters: n },
                Some(FreeRegion { start_lcn: self.start_lcn + n,
                                  len_clusters: self.len_clusters - n }),
            )
        }
    }
}

/// Iterator over the `1`-bit runs inside a bitmap chunk. Used by the
/// allocation check too, not just free-run scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContiguousRun {
    pub start_bit: u64,
    pub len_bits: u64,
    pub set: bool, // true = allocated, false = free
}

/// Parsed bitmap chunk returned by `FSCTL_GET_VOLUME_BITMAP`. We store
/// only the header fields plus the raw bitmap bytes; the bytes are
/// consumed by the iterator and discarded when the next chunk is fetched.
#[derive(Debug, Clone)]
pub struct VolumeBitmap {
    /// LCN of the first cluster covered by this bitmap chunk.
    pub starting_lcn: u64,
    /// Total clusters covered by this chunk (may be less than
    /// `BITMAP_CHUNK_CLUSTERS` for the final chunk).
    pub cluster_count: u64,
    /// Raw bitmap bytes. Bit `i` corresponds to cluster
    /// `starting_lcn + i`.
    pub bitmap: Vec<u8>,
}

impl VolumeBitmap {
    /// `true` if cluster `lcn` (absolute) is allocated in this chunk.
    /// Returns `None` if `lcn` falls outside this chunk.
    pub fn is_allocated(&self, lcn: u64) -> Option<bool> {
        if lcn < self.starting_lcn { return None; }
        let rel = lcn - self.starting_lcn;
        if rel >= self.cluster_count { return None; }
        let byte_idx = (rel / 8) as usize;
        let bit_idx = rel % 8;
        if byte_idx >= self.bitmap.len() { return None; }
        Some((self.bitmap[byte_idx] >> bit_idx) & 1 == 1)
    }

    /// Iterate over all contiguous runs (free or allocated) in this chunk.
    /// The iterator owns a snapshot reference to the bitmap bytes.
    pub fn runs(&self) -> VolumeBitmapRuns<'_> {
        VolumeBitmapRuns {
            bitmap: &self.bitmap,
            starting_lcn: self.starting_lcn,
            pos: 0,
            total: self.cluster_count,
        }
    }

    /// All free runs (`set == false`) in this chunk, sorted by ascending
    /// LCN. Use this to find low-LCN free regions for consolidation.
    pub fn free_runs(&self) -> Vec<FreeRegion> {
        self.runs()
            .filter(|r| !r.set)
            .map(|r| FreeRegion {
                start_lcn: r.start_bit,
                len_clusters: r.len_bits,
            })
            .collect()
    }
}

/// Owning iterator over runs inside a `VolumeBitmap`. Borrows the bitmap
/// bytes for the duration of iteration.
pub struct VolumeBitmapRuns<'a> {
    bitmap: &'a [u8],
    starting_lcn: u64,
    pos: u64,
    total: u64,
}

impl<'a> Iterator for VolumeBitmapRuns<'a> {
    type Item = ContiguousRun;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.total { return None; }
        let start = self.pos;
        let start_set = self.bit_set(start);

        // Walk forward while the bit value stays the same.
        let mut end = start;
        while end < self.total && self.bit_set(end) == start_set {
            end += 1;
        }
        self.pos = end;
        Some(ContiguousRun {
            start_bit: self.starting_lcn + start,
            len_bits: end - start,
            set: start_set,
        })
    }
}

impl<'a> VolumeBitmapRuns<'a> {
    fn bit_set(&self, rel: u64) -> bool {
        let byte_idx = (rel / 8) as usize;
        let bit_idx = rel % 8;
        if byte_idx >= self.bitmap.len() { return false; }
        (self.bitmap[byte_idx] >> bit_idx) & 1 == 1
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

// FSCTL_GET_VOLUME_BITMAP = CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 27,
//                                    METHOD_NEITHER, FILE_ANY_ACCESS)
//                                    = 0x0009006F
#[cfg(target_os = "windows")]
const FSCTL_GET_VOLUME_BITMAP: u32 = 0x0009006F;

/// `STARTING_LCN_INPUT_BUFFER` — u64 LCN where the bitmap enumeration
/// should start.
#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct StartingLcnInputBuffer {
    starting_lcn: u64,
}

/// `VOLUME_BITMAP_BUFFER` header — variable-size struct returned by the
/// FSCTL. The bitmap bytes follow immediately after these two fields.
#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct VolumeBitmapBufferHeader {
    starting_lcn: u64,
    bitmap_size: u64, // number of clusters (bits) in the returned bitmap
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

/// Fetch one chunk of the volume bitmap starting at `starting_lcn`.
///
/// The caller drives the loop: bump `starting_lcn` by `chunk.cluster_count`
/// and call again until the returned chunk's `cluster_count` is smaller
/// than `BITMAP_CHUNK_CLUSTERS` (final chunk).
pub fn bitmap_chunk(drive: char, starting_lcn: u64) -> GResult<VolumeBitmap> {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

        let volume_path = format!("\\\\.\\{}:", drive.to_ascii_uppercase());
        let mut wide: Vec<u16> = OsStr::new(&volume_path).encode_wide().collect();
        wide.push(0);

        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Err(GError::Defrag(format!(
                "open volume {drive}: for bitmap failed (Win32 error {})",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            )));
        }
        let _guard = HandleGuard(h);

        // Output buffer: 16 bytes header + enough bytes for one chunk.
        let chunk_bits = BITMAP_CHUNK_CLUSTERS;
        let chunk_bytes = (chunk_bits / 8) as usize;
        let mut out_buf: Vec<u8> = vec![0u8; 16 + chunk_bytes];

        let input = StartingLcnInputBuffer { starting_lcn };
        let mut returned: u32 = 0;

        let ok = unsafe {
            DeviceIoControl(
                h,
                FSCTL_GET_VOLUME_BITMAP,
                &input as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<StartingLcnInputBuffer>() as u32,
                out_buf.as_mut_ptr() as *mut std::ffi::c_void,
                out_buf.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(GError::Defrag(format!(
                "FSCTL_GET_VOLUME_BITMAP on {drive}: @ LCN {starting_lcn} failed (Win32 error {})",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            )));
        }

        // Parse the header.
        let returned_starting_lcn = u64::from_le_bytes([
            out_buf[0], out_buf[1], out_buf[2], out_buf[3],
            out_buf[4], out_buf[5], out_buf[6], out_buf[7],
        ]);
        let bitmap_size = u64::from_le_bytes([
            out_buf[8], out_buf[9], out_buf[10], out_buf[11],
            out_buf[12], out_buf[13], out_buf[14], out_buf[15],
        ]);
        // The actual bitmap bytes follow the 16-byte header.
        let bitmap_byte_count = ((bitmap_size + 7) / 8) as usize;
        let bitmap_end = 16 + bitmap_byte_count;
        if bitmap_end > out_buf.len() {
            // Buffer was too small — clamp.
            return Err(GError::Defrag(format!(
                "volume bitmap buffer too small: got {} bytes, need {bitmap_end}",
                out_buf.len()
            )));
        }
        let bitmap = out_buf[16..bitmap_end].to_vec();

        Ok(VolumeBitmap {
            starting_lcn: returned_starting_lcn,
            cluster_count: bitmap_size,
            bitmap,
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (drive, starting_lcn);
        Err(GError::NotSupportedPlatform)
    }
}

/// Scan the entire volume bitmap (chunked) and return all free runs,
/// sorted by ascending LCN. **RAM-friendly**: only one chunk is in
/// memory at a time.
///
/// This is the entry point the planner uses to find target LCNs for
/// consolidation.
pub fn scan_all_free_regions(drive: char, total_clusters: u64) -> GResult<Vec<FreeRegion>> {
    let mut free: Vec<FreeRegion> = Vec::new();
    let mut lcn: u64 = 0;
    while lcn < total_clusters {
        let chunk = bitmap_chunk(drive, lcn)?;
        if chunk.cluster_count == 0 { break; }
        for r in chunk.free_runs() {
            // Coalesce with the previous run if they're contiguous — this
            // happens when a free region spans a chunk boundary.
            if let Some(last) = free.last_mut() {
                if last.end_lcn_exclusive() == r.start_lcn {
                    last.len_clusters += r.len_clusters;
                    lcn += chunk.cluster_count;
                    continue;
                }
            }
            free.push(r);
        }
        lcn += chunk.cluster_count;
        if chunk.cluster_count < BITMAP_CHUNK_CLUSTERS { break; }
    }
    Ok(free)
}

/// Quick check: is `lcn` (absolute) free right now? Used by the move
/// engine's Double-Check Overwrite Guard immediately before issuing
/// `FSCTL_MOVE_FILE`.
pub fn is_lcn_free(drive: char, lcn: u64) -> GResult<bool> {
    let chunk = bitmap_chunk(drive, lcn)?;
    Ok(chunk.is_allocated(lcn) == Some(false))
}

/// Test-only helper: build a `VolumeBitmap` from a byte slice.
#[cfg(test)]
pub fn make_test_bitmap(starting_lcn: u64, cluster_count: u64, bitmap: Vec<u8>) -> VolumeBitmap {
    VolumeBitmap { starting_lcn, cluster_count, bitmap }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_region_split_exact() {
        let r = FreeRegion { start_lcn: 100, len_clusters: 50 };
        let (a, b) = r.split_at(50);
        assert_eq!(a.len_clusters, 50);
        assert_eq!(b, None);
    }

    #[test]
    fn free_region_split_partial() {
        let r = FreeRegion { start_lcn: 100, len_clusters: 50 };
        let (a, b) = r.split_at(20);
        assert_eq!(a.len_clusters, 20);
        assert_eq!(a.start_lcn, 100);
        assert_eq!(b.unwrap().len_clusters, 30);
        assert_eq!(b.unwrap().start_lcn, 120);
    }

    #[test]
    fn free_region_fits_check() {
        let r = FreeRegion { start_lcn: 0, len_clusters: 100 };
        assert!(r.fits(100));
        assert!(r.fits(50));
        assert!(!r.fits(101));
    }

    #[test]
    fn bitmap_is_allocated() {
        // bitmap = 0b10110001 = 0xB1. LSB is bit 0.
        // bit 0 = 1 → allocated
        // bit 1 = 0 → free
        // bit 2 = 0 → free
        // bit 3 = 0 → free
        // bit 4 = 1 → allocated
        // bit 5 = 1 → allocated
        // bit 6 = 0 → free
        // bit 7 = 1 → allocated
        let b = VolumeBitmap {
            starting_lcn: 1000,
            cluster_count: 8,
            bitmap: vec![0b10110001],
        };
        assert_eq!(b.is_allocated(1000), Some(true));  // bit 0
        assert_eq!(b.is_allocated(1001), Some(false)); // bit 1
        assert_eq!(b.is_allocated(1002), Some(false)); // bit 2
        assert_eq!(b.is_allocated(1003), Some(false)); // bit 3
        assert_eq!(b.is_allocated(1004), Some(true));  // bit 4
        assert_eq!(b.is_allocated(1005), Some(true));  // bit 5
        assert_eq!(b.is_allocated(1006), Some(false)); // bit 6
        assert_eq!(b.is_allocated(1007), Some(true));  // bit 7
        assert_eq!(b.is_allocated(2000), None);        // out of range
    }

    #[test]
    fn bitmap_free_runs() {
        // 16 clusters, bitmap = [0b00001111, 0b11110000]
        // byte 0 = 0x0F → bits 0..4 = 1 (allocated), bits 4..8 = 0 (free)
        // byte 1 = 0xF0 → bits 0..4 of byte 1 = 0 (free), bits 4..8 = 1 (allocated)
        // So overall: clusters 0..4 alloc, 4..12 free (8 contiguous), 12..16 alloc.
        let b = VolumeBitmap {
            starting_lcn: 0,
            cluster_count: 16,
            bitmap: vec![0b00001111, 0b11110000],
        };
        let runs = b.free_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0], FreeRegion { start_lcn: 4, len_clusters: 8 });
    }

    #[test]
    fn bitmap_runs_iter_yields_all_runs() {
        let b = VolumeBitmap {
            starting_lcn: 0,
            cluster_count: 16,
            bitmap: vec![0b00001111, 0b11110000],
        };
        let runs: Vec<_> = b.runs().collect();
        // Expect: free(0..4), alloc(4..8), alloc(8..12), free(12..16).
        // Actually since bytes 0 and 1 are 0x0F and 0xF0, the runs are:
        //   bit 0..4 = 1 (alloc), bit 4..8 = 0 (free),
        //   bit 8..12 = 1 (alloc), bit 12..16 = 0 (free).
        // Wait — bitmap[0]=0x0F means bits 0..4 = 1, bits 4..8 = 0.
        // bitmap[1]=0xF0 means bits 0..4 of byte 1 = 0, bits 4..8 = 1.
        // So overall:
        //   clusters 0..4 = 1 (alloc)
        //   clusters 4..8 = 0 (free)
        //   clusters 8..12 = 0 (free)  ← contiguous with previous free!
        //   clusters 12..16 = 1 (alloc)
        // The iterator should merge 4..8 and 8..12 into one free run.
        assert_eq!(runs.len(), 3);
        assert!(runs[0].set);
        assert_eq!(runs[0].len_bits, 4);
        assert!(!runs[1].set);
        assert_eq!(runs[1].len_bits, 8); // merged
        assert!(runs[2].set);
        assert_eq!(runs[2].len_bits, 4);
    }

    #[test]
    fn chunk_size_is_reasonable() {
        // 8M clusters × 4 KB clusters = 32 GB per chunk — plenty.
        assert!(BITMAP_CHUNK_CLUSTERS >= 1_000_000);
        assert!(BITMAP_CHUNK_CLUSTERS <= 100_000_000);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn bitmap_chunk_unsupported_off_windows() {
        assert!(matches!(bitmap_chunk('C', 0), Err(GError::NotSupportedPlatform)));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn is_lcn_free_unsupported_off_windows() {
        assert!(matches!(is_lcn_free('C', 0), Err(GError::NotSupportedPlatform)));
    }
}
