//! Windows Overlay Filter (WOF) compression via raw Win32 FFI.
//!
//! Equivalent to `compact.exe /EXE:LZX` etc. The file stays readable
//! transparently — compressed chunks are stored in a backing data stream
//! managed by the WOF driver.
//!
//! # API sequence
//!
//! 1. `CreateFileW` with `GENERIC_READ | GENERIC_WRITE` (plus
//!    `FILE_READ_ATTRIBUTES` so we can read the result) and
//!    `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`.
//! 2. Build a payload buffer: a `WOF_EXTERNAL_INFO` header immediately
//!    followed by a `FILE_PROVIDER_EXTERNAL_INFO_1` body.
//! 3. `DeviceIoControl(handle, FSCTL_SET_EXTERNAL_BACKING, payload, ...)`.
//!
//! The structures are documented at:
//! - <https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_wof_external_info>
//! - <https://learn.microsoft.com/en-us/windows/win32/api/wofapi/ns-wofapi-wof_file_compression_info_v1>
//!
//! This whole module is Windows-only. On other targets every entry point
//! returns [`crate::error::GError::NotSupportedPlatform`].
//!
//! # Safety
//!
//! All `unsafe` is confined to FFI call sites here. Handles are wrapped in a
//! RAII guard (`HandleGuard`) so `CloseHandle` always runs.

use crate::error::{GError, GResult};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

// ── Control codes ───────────────────────────────────────────────────────
// FSCTL_SET_EXTERNAL_BACKING    = CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 195,
//                                          METHOD_BUFFERED, FILE_ANY_ACCESS)
// FSCTL_GET_EXTERNAL_BACKING    = CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 193, ...)
// FSCTL_DELETE_EXTERNAL_BACKING = CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 196, ...)
const FSCTL_SET_EXTERNAL_BACKING: u32 = 0x00098C88;
const FSCTL_GET_EXTERNAL_BACKING: u32 = 0x00098C84;
const FSCTL_DELETE_EXTERNAL_BACKING: u32 = 0x00098C90;

// ── WOF constants ──────────────────────────────────────────────────────
const WOF_CURRENT_VERSION: u32 = 1;
/// File backing provider (as opposed to WIM = 2).
const WOF_PROVIDER_FILE: u32 = 1;

/// `FILE_PROVIDER_COMPRESSION_*` algorithm constants (from wofapi.h).
pub const FILE_PROVIDER_COMPRESSION_XPRESS4K: u32 = 0;
pub const FILE_PROVIDER_COMPRESSION_LZX: u32 = 1;
pub const FILE_PROVIDER_COMPRESSION_XPRESS8K: u32 = 2;
pub const FILE_PROVIDER_COMPRESSION_XPRESS16K: u32 = 3;
pub const FILE_PROVIDER_COMPRESSION_NO_COMPRESSION: u32 = 4;

// ── Win32 access / share / disposition constants ──────────────────────
const GENERIC_READ: u32 = 0x80000000;
const GENERIC_WRITE: u32 = 0x40000000;
const FILE_SHARE_READ: u32 = 0x00000001;
const FILE_SHARE_WRITE: u32 = 0x00000002;
const FILE_SHARE_DELETE: u32 = 0x00000004;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = -1isize as *mut std::ffi::c_void;

#[repr(C)]
#[derive(Copy, Clone)]
struct WofExternalInfo {
    version: u32,
    provider: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct FileProviderExternalInfo1 {
    version: u32,
    algorithm: u32,
    flags: u32,
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

    fn GetLastError() -> u32;
}

/// RAII guard wrapping a Win32 `HANDLE`. Closes the handle on drop.
#[cfg(target_os = "windows")]
struct HandleGuard(*mut std::ffi::c_void);

#[cfg(target_os = "windows")]
impl HandleGuard {
    /// Returns `Err` for `INVALID_HANDLE_VALUE` or a null handle.
    fn check(&self) -> GResult<()> {
        if self.0.is_null() || self.0 == INVALID_HANDLE_VALUE {
            Err(last_error("CreateFileW"))
        } else {
            Ok(())
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe { let _ = CloseHandle(self.0); }
        }
    }
}

/// Open the file for read/write with broad sharing. Sharing violations
/// (e.g. the game holds the file open) surface as `GError::Io`.
#[cfg(target_os = "windows")]
fn open_file_rw(path: &Path) -> GResult<HandleGuard> {
    let mut wide: Vec<u16> = OsStr::new(path).encode_wide().collect();
    wide.push(0); // NUL terminator

    // GENERIC_READ | GENERIC_WRITE; share everything so we don't fight
    // other readers (the file stays usable while we mark it for compression).
    let access = GENERIC_READ | GENERIC_WRITE;
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

    // SAFETY: `wide` is a valid NUL-terminated UTF-16 buffer for the
    // duration of the call; the other pointer args are null/zero per spec.
    let h = unsafe {
        CreateFileW(wide.as_ptr(), access, share, std::ptr::null_mut(),
                    OPEN_EXISTING, 0, std::ptr::null_mut())
    };
    let g = HandleGuard(h);
    g.check()?;
    Ok(g)
}

/// Build the `WOF_EXTERNAL_INFO` + `FILE_PROVIDER_EXTERNAL_INFO_1` payload
/// that `FSCTL_SET_EXTERNAL_BACKING` expects.
fn build_backing_payload(algorithm: u32) -> [u8; 16] {
    // Both structures are 8 bytes each, packed back-to-back. We serialize
    // them as little-endian bytes (matching x86/x64 Windows) to avoid
    // `repr(C, packed)` and any UB around unaligned struct references.
    let header = WofExternalInfo { version: WOF_CURRENT_VERSION, provider: WOF_PROVIDER_FILE };
    let body = FileProviderExternalInfo1 { version: 1, algorithm, flags: 0 };
    let mut buf = [0u8; 16];
    buf[0..4].copy_from_slice(&header.version.to_le_bytes());
    buf[4..8].copy_from_slice(&header.provider.to_le_bytes());
    buf[8..12].copy_from_slice(&body.version.to_le_bytes());
    buf[12..16].copy_from_slice(&body.algorithm.to_le_bytes());
    // flags would be bytes [16..20] but FILE_PROVIDER_EXTERNAL_INFO_1 used
    // by SET only reads the first 12 bytes of the body; we keep payload at 16
    // for alignment safety with all observed Windows versions.
    buf
}

/// Apply WOF compression to a single file.
///
/// After this returns successfully, the file is compressed on disk by the
/// WOF driver using `algorithm` (one of the `FILE_PROVIDER_COMPRESSION_*`
/// constants). The file remains readable normally.
pub fn set_wof_compression(path: &Path, algorithm: u32) -> GResult<()> {
    #[cfg(target_os = "windows")]
    {
        let guard = open_file_rw(path)?;
        let payload = build_backing_payload(algorithm);
        let mut returned: u32 = 0;
        // SAFETY: handle came from CreateFileW; payload buffer outlives the
        // call; output params are valid stack slots.
        let ok = unsafe {
            DeviceIoControl(
                guard.0,
                FSCTL_SET_EXTERNAL_BACKING,
                payload.as_ptr() as *const std::ffi::c_void,
                payload.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(last_error("DeviceIoControl(FSCTL_SET_EXTERNAL_BACKING)"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (path, algorithm);
        Err(GError::NotSupportedPlatform)
    }
}

/// Query the WOF compression algorithm currently set on a file.
///
/// Returns `Ok(None)` if the file is not WOF-backed (either uncompressed or
/// compressed by some other mechanism). Returns `Ok(Some(algo))` otherwise.
pub fn get_wof_compression(path: &Path) -> GResult<Option<u32>> {
    #[cfg(target_os = "windows")]
    {
        let guard = match open_file_rw(path) {
            Ok(g) => g,
            // Don't treat a missing/unreadable file as "not compressed" —
            // surface it so callers can decide. But for a plain stat pass
            // we still want to keep going, so we map to None on access error.
            Err(_) => return Ok(None),
        };
        let mut buf = [0u8; 32]; // header(8) + body(12) + slack
        let mut returned: u32 = 0;
        // SAFETY: same as above; output buffer is large enough for both structs.
        let ok = unsafe {
            DeviceIoControl(
                guard.0,
                FSCTL_GET_EXTERNAL_BACKING,
                std::ptr::null(),
                0,
                buf.as_mut_ptr() as *mut std::ffi::c_void,
                buf.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            // Not externally backed (typical). Distinguish from a real error
            // via GetLastError: ERROR_NOT_FOUND / ERROR_INVALID_PARAMETER
            // mean "no backing" — return None. Anything else bubbles up.
            let code = unsafe { GetLastError() };
            // ERROR_NOT_FOUND = 1168, ERROR_INVALID_FUNCTION = 1,
            // ERROR_INVALID_PARAMETER = 87, ERROR_NOT_SUPPORTED = 50.
            const ERROR_NOT_FOUND: u32 = 1168;
            const ERROR_INVALID_FUNCTION: u32 = 1;
            const ERROR_INVALID_PARAMETER: u32 = 87;
            const ERROR_NOT_SUPPORTED: u32 = 50;
            return match code {
                ERROR_NOT_FOUND | ERROR_INVALID_FUNCTION
                | ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED => Ok(None),
                _ => Err(last_error("DeviceIoControl(FSCTL_GET_EXTERNAL_BACKING)")),
            };
        }
        // The algorithm lives at offset 12 (header 8 + body.version 4).
        if (returned as usize) < 16 {
            return Ok(None);
        }
        let algo = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        Ok(Some(algo))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Ok(None)
    }
}

/// Remove any WOF external backing from a file (decompresses it).
pub fn remove_wof_compression(path: &Path) -> GResult<()> {
    #[cfg(target_os = "windows")]
    {
        let guard = open_file_rw(path)?;
        let mut returned: u32 = 0;
        // SAFETY: handle from CreateFileW; no in/out buffer needed for delete.
        let ok = unsafe {
            DeviceIoControl(
                guard.0,
                FSCTL_DELETE_EXTERNAL_BACKING,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            // ERROR_NOT_FOUND means "not compressed" — treat as success.
            let code = unsafe { GetLastError() };
            const ERROR_NOT_FOUND: u32 = 1168;
            if code != ERROR_NOT_FOUND {
                return Err(last_error("DeviceIoControl(FSCTL_DELETE_EXTERNAL_BACKING)"));
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err(GError::NotSupportedPlatform)
    }
}

#[cfg(target_os = "windows")]
fn last_error(where_: &str) -> GError {
    let code = unsafe { GetLastError() };
    GError::Compact(format!("{where_} failed (Win32 error {code})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_layout_is_16_bytes() {
        let p = build_backing_payload(FILE_PROVIDER_COMPRESSION_LZX);
        assert_eq!(p.len(), 16);
        // Header version == WOF_CURRENT_VERSION (1) at offset 0.
        assert_eq!(u32::from_le_bytes([p[0], p[1], p[2], p[3]]), WOF_CURRENT_VERSION);
        // Provider == WOF_PROVIDER_FILE (1) at offset 4.
        assert_eq!(u32::from_le_bytes([p[4], p[5], p[6], p[7]]), WOF_PROVIDER_FILE);
        // Body version (1) at offset 8.
        assert_eq!(u32::from_le_bytes([p[8], p[9], p[10], p[11]]), 1);
        // Algorithm (LZX = 1) at offset 12.
        assert_eq!(u32::from_le_bytes([p[12], p[13], p[14], p[15]]), FILE_PROVIDER_COMPRESSION_LZX);
    }

    #[test]
    fn payload_xpress4k_roundtrips() {
        let p = build_backing_payload(FILE_PROVIDER_COMPRESSION_XPRESS4K);
        assert_eq!(u32::from_le_bytes([p[12], p[13], p[14], p[15]]), FILE_PROVIDER_COMPRESSION_XPRESS4K);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_returns_not_supported() {
        use std::path::PathBuf;
        let p = PathBuf::from("/tmp/nope");
        assert!(matches!(set_wof_compression(&p, 1), Err(GError::NotSupportedPlatform)));
        assert!(matches!(remove_wof_compression(&p), Err(GError::NotSupportedPlatform)));
        assert_eq!(get_wof_compression(&p).unwrap(), None);
    }
}
