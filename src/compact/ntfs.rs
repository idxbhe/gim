//! NTFS live compression via raw Win32 FFI.
//!
//! Equivalent to `compact.exe /C`. The only supported algorithm is
//! **LZNT1**. The compressed data lives in the file's own data stream, so
//! it works on any NTFS volume and survives copies that preserve streams.
//!
//! # API
//!
//! 1. `CreateFileW` with `GENERIC_READ | GENERIC_WRITE` and broad sharing.
//! 2. `DeviceIoControl(handle, FSCTL_SET_COMPRESSION, &format, ...)`,
//!    where `format` is a single `u16` (`COMPRESSION_FORMAT_LZNT1` or
//!    `COMPRESSION_FORMAT_NONE`).
//!
//! See: <https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_set_compression>
//!
//! Windows-only; on other targets every entry point returns
//! [`crate::error::GError::NotSupportedPlatform`].

use crate::error::{GError, GResult};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

// FSCTL_SET_COMPRESSION = CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 16,
//                                  METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA)
const FSCTL_SET_COMPRESSION: u32 = 0x0009C040;

/// `u16` compression-format values accepted by `FSCTL_SET_COMPRESSION`.
pub const COMPRESSION_FORMAT_NONE: u16 = 0;
pub const COMPRESSION_FORMAT_LZNT1: u16 = 2;

const GENERIC_READ: u32 = 0x80000000;
const GENERIC_WRITE: u32 = 0x40000000;
const FILE_SHARE_READ: u32 = 0x00000001;
const FILE_SHARE_WRITE: u32 = 0x00000002;
const FILE_SHARE_DELETE: u32 = 0x00000004;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = -1isize as *mut std::ffi::c_void;

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

#[cfg(target_os = "windows")]
fn open_file_rw(path: &Path) -> GResult<HandleGuard> {
    let mut wide: Vec<u16> = OsStr::new(path).encode_wide().collect();
    wide.push(0);

    let access = GENERIC_READ | GENERIC_WRITE;
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    // SAFETY: NUL-terminated UTF-16 path; remaining pointer args null/zero.
    let h = unsafe {
        CreateFileW(wide.as_ptr(), access, share, std::ptr::null_mut(),
                    OPEN_EXISTING, 0, std::ptr::null_mut())
    };
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW"));
    }
    Ok(HandleGuard(h))
}

/// Set NTFS compression state on a file.
///
/// Pass [`COMPRESSION_FORMAT_LZNT1`] to compress, or
/// [`COMPRESSION_FORMAT_NONE`] to decompress.
pub fn set_ntfs_compression(path: &Path, format: u16) -> GResult<()> {
    debug_assert!(format == COMPRESSION_FORMAT_LZNT1 || format == COMPRESSION_FORMAT_NONE);
    #[cfg(target_os = "windows")]
    {
        let guard = open_file_rw(path)?;
        let mut bytes = [0u8; 2];
        bytes.copy_from_slice(&format.to_le_bytes());
        let mut returned: u32 = 0;
        // SAFETY: handle from CreateFileW; in-buffer is a 2-byte u16 slot.
        let ok = unsafe {
            DeviceIoControl(
                guard.0,
                FSCTL_SET_COMPRESSION,
                bytes.as_ptr() as *const std::ffi::c_void,
                bytes.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(last_error("DeviceIoControl(FSCTL_SET_COMPRESSION)"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (path, format);
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
    fn format_constants_match_winioctl() {
        // Sanity-check the hand-typed constants against the documented values.
        assert_eq!(COMPRESSION_FORMAT_NONE, 0);
        assert_eq!(COMPRESSION_FORMAT_LZNT1, 2);
        assert_eq!(FSCTL_SET_COMPRESSION, 0x0009C040);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_returns_not_supported() {
        use std::path::PathBuf;
        let p = PathBuf::from("/tmp/nope");
        assert!(matches!(
            set_ntfs_compression(&p, COMPRESSION_FORMAT_LZNT1),
            Err(GError::NotSupportedPlatform)
        ));
    }
}
