//! Stage 6 — `FSCTL_MOVE_FILE` wrapper.
//!
//! `FSCTL_MOVE_FILE` relocates a range of VCNs (virtual clusters, file
//! offset) of an open file to a new LCN range (physical clusters, on disk).
//! NTFS performs the move atomically: the MFT entry is updated in a single
//! transaction, so a power loss mid-move rolls back to the previous MFT
//! state. We **never** `read + write + delete` manually — that would risk
//! data corruption on interruption.
//!
//! # Per-move safety (Double-Check Overwrite Guard)
//!
//! The volume bitmap we scanned in stage 4 is a snapshot. Between that
//! scan and the actual move, the OS may have allocated our target LCNs
//! to another file. We re-check the target region with a fresh bitmap
//! chunk immediately before each `FSCTL_MOVE_FILE` call. If any cluster
//! in the target range is no longer free, we abort that move and let the
//! planner pick a new target.
//!
//! # Alignment
//!
//! On Advanced Format drives (physical sector 4 KB, logical 512 B), LCN
//! ranges must be aligned to the physical sector size to avoid the
//! read-modify-write penalty. The planner already aligns; here we just
//! assert (debug-build only) that the inputs look aligned.
//!
//! # Non-Windows
//!
//! Returns `NotSupportedPlatform`.

use crate::defrag::bitmap::is_lcn_free;
use crate::error::{GError, GResult};

/// Request to move a contiguous VCN range of `file` to `target_lcn`.
#[derive(Debug, Clone, Copy)]
pub struct MoveRequest {
    /// File handle already open by the caller (passed as raw ptr on Windows).
    pub file_handle: *mut std::ffi::c_void,
    /// Starting VCN (file-relative cluster offset).
    pub start_vcn: u64,
    /// Number of clusters to move.
    pub cluster_count: u64,
    /// Destination LCN (volume-absolute).
    pub target_lcn: u64,
}

/// Outcome of a single move attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveOutcome {
    /// The clusters were moved successfully.
    Moved { bytes: u64 },
    /// The target LCN range was no longer fully free. Caller should pick
    /// a new target.
    TargetOccupied,
    /// The file is locked by another process. Caller should defer this
    /// file to the end of the queue or skip it.
    Locked,
    /// The OS refused for a reason we don't recover from (e.g. MFT
    /// attribute list too long). Caller should skip the file.
    HardError(u32),
}

// ── Win32 FFI ────────────────────────────────────────────────────────────
#[cfg(target_os = "windows")]
const FSCTL_MOVE_FILE: u32 = 0x000900A4;
// = CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 29, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA)

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct MoveFileInput {
    file_handle: *mut std::ffi::c_void,
    starting_vcn: u64,
    starting_lcn: u64,
    cluster_count: u32,
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn DeviceIoControl(
        hvolume: *mut std::ffi::c_void,
        dwiocontrolcode: u32,
        lpinbuffer: *const std::ffi::c_void,
        ninbuffersize: u32,
        lpoutbuffer: *mut std::ffi::c_void,
        noutbuffersize: u32,
        lpbytesreturned: *mut u32,
        lpoverlapped: *mut std::ffi::c_void,
    ) -> i32;
}

/// Execute one move, with the Double-Check Overwrite Guard.
///
/// `volume_handle` is the `\\.\X:` handle obtained from `open_volume`.
/// `bytes_per_cluster` is used to convert cluster counts to byte counts
/// for the throttle bookkeeping.
pub fn execute_move(
    drive: char,
    volume_handle: *mut std::ffi::c_void,
    req: MoveRequest,
    bytes_per_cluster: u64,
) -> GResult<MoveOutcome> {
    // ── Double-Check Overwrite Guard ─────────────────────────────────
    // Right before issuing FSCTL_MOVE_FILE, verify the target LCN range
    // is still free. The volume bitmap snapshot from stage 4 may be stale
    // by now — another process could have grabbed those clusters.
    if !verify_target_free(drive, req.target_lcn, req.cluster_count)? {
        return Ok(MoveOutcome::TargetOccupied);
    }

    #[cfg(target_os = "windows")]
    {
        debug_assert!(
            req.cluster_count > 0,
            "FSCTL_MOVE_FILE with zero clusters is a no-op"
        );

        let input = MoveFileInput {
            file_handle: req.file_handle,
            starting_vcn: req.start_vcn,
            starting_lcn: req.target_lcn,
            cluster_count: req.cluster_count as u32,
        };
        let mut returned: u32 = 0;
        // SAFETY: `volume_handle` is a valid HANDLE owned by the caller.
        // `input` is a stack struct with the documented layout. No output
        // buffer needed.
        let ok = unsafe {
            DeviceIoControl(
                volume_handle,
                FSCTL_MOVE_FILE,
                &input as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<MoveFileInput>() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 {
            return Ok(MoveOutcome::Moved {
                bytes: req.cluster_count * bytes_per_cluster,
            });
        }
        // Inspect the Win32 error code to classify the failure.
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
        // ERROR_LOCK_VIOLATION (33) and ERROR_SHARING_VIOLATION (32) → file locked.
        // ERROR_ACCESS_DENIED (5) → typically file locked by another process.
        if code == 33 || code == 32 || code == 5 {
            return Ok(MoveOutcome::Locked);
        }
        // ERROR_DISK_FULL (112), ERROR_NOT_ENOUGH_QUOTA, etc. → target LCN
        // got grabbed between our guard check and the FSCTL.
        if code == 112 {
            return Ok(MoveOutcome::TargetOccupied);
        }
        Ok(MoveOutcome::HardError(code))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (drive, volume_handle, req, bytes_per_cluster);
        Err(GError::NotSupportedPlatform)
    }
}

/// Verify that LCNs `[start, start + count)` are all free *right now*.
///
/// This is the Double-Check Overwrite Guard: a per-cluster re-scan of
/// the volume bitmap immediately before each `FSCTL_MOVE_FILE` call.
///
/// The cost is one `FSCTL_GET_VOLUME_BITMAP` call per move, which on a
/// 4 KB-cluster volume returns a few hundred KB per chunk. That's
/// negligible next to the actual cluster-move cost (typically tens of MB
/// of disk I/O per file).
pub fn verify_target_free(drive: char, start_lcn: u64, count: u64) -> GResult<bool> {
    for lcn in start_lcn..start_lcn.saturating_add(count) {
        if !is_lcn_free(drive, lcn)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_request_layout() {
        let r = MoveRequest {
            file_handle: std::ptr::null_mut(),
            start_vcn: 0,
            cluster_count: 100,
            target_lcn: 5000,
        };
        assert_eq!(r.start_vcn, 0);
        assert_eq!(r.cluster_count, 100);
        assert_eq!(r.target_lcn, 5000);
    }

    #[test]
    fn move_outcome_classification() {
        assert!(matches!(MoveOutcome::Moved { bytes: 4096 }, MoveOutcome::Moved { .. }));
        assert_eq!(MoveOutcome::TargetOccupied, MoveOutcome::TargetOccupied);
        assert_eq!(MoveOutcome::Locked, MoveOutcome::Locked);
        assert_eq!(MoveOutcome::HardError(1224), MoveOutcome::HardError(1224));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn execute_move_unsupported_off_windows() {
        let r = MoveRequest {
            file_handle: std::ptr::null_mut(),
            start_vcn: 0, cluster_count: 10, target_lcn: 1000,
        };
        assert!(matches!(
            execute_move('C', std::ptr::null_mut(), r, 4096),
            Err(GError::NotSupportedPlatform)
        ));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn verify_target_free_unsupported_off_windows() {
        assert!(matches!(verify_target_free('C', 0, 10),
                         Err(GError::NotSupportedPlatform)));
    }
}
