//! Stage 5 — pre-move safety checks per file.
//!
//! The instruction lists several file attributes that **must not** be
//! touched by `FSCTL_MOVE_FILE`:
//!
//! - `FILE_ATTRIBUTE_COMPRESSED` — NTFS compression maintains VCN/LCN
//!   mappings that move synchronously with the file's compression state.
//!   Moving compressed clusters can desync that mapping.
//! - `FILE_ATTRIBUTE_ENCRYPTED` — same problem, plus EFS key material
//!   tracking.
//! - `FILE_ATTRIBUTE_SPARSE_FILE` — sparse files have "holes" (VCN ranges
//!   with no LCN backing). `FSCTL_MOVE_FILE` semantics on sparse holes
//!   are subtle; safer to skip.
//!
//! We also refuse files currently held open by another process (sharing
//! violation). Deferring them to the end of the queue is fine; failing
//! the whole run on one locked file is not.
//!
//! # Non-Windows
//!
//! `check_file_safety` returns `FileSafety::Ok` (so the planner can still
//! produce a plan for inspection), but the command dispatcher gates the
//! whole flow on Windows.

use crate::error::{GError, GResult};
use std::path::Path;

/// Subset of Win32 file attributes we care about. Other flags (HIDDEN,
/// SYSTEM, ARCHIVE, etc.) are irrelevant for defrag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileAttrs {
    pub compressed: bool,
    pub encrypted: bool,
    pub sparse: bool,
    pub readonly: bool,
    pub offline: bool,
    pub reparse_point: bool,
}

impl FileAttrs {
    /// `true` if any attribute makes this file unsafe to move.
    pub fn blocks_defrag(&self) -> bool {
        self.compressed || self.encrypted || self.sparse || self.offline || self.reparse_point
    }

    /// Human-readable reason for refusal (or empty string if OK).
    pub fn block_reason(&self) -> &'static str {
        if self.compressed { return "NTFS-compressed" }
        if self.encrypted { return "EFS-encrypted" }
        if self.sparse { return "sparse" }
        if self.offline { return "offline (HSM)" }
        if self.reparse_point { return "reparse point (symlink/junction)" }
        ""
    }
}

/// Result of one file's safety check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSafety {
    /// File is safe to defrag.
    Ok,
    /// File has attributes that block defrag. Caller should skip it.
    SkipAttrs(&'static str),
    /// File is currently locked by another process. Caller may defer to
    /// the end of the queue or skip.
    Locked,
    /// File can't be opened (permission denied, not found, etc.).
    Inaccessible,
}

// ── Win32 FFI ────────────────────────────────────────────────────────────
#[cfg(target_os = "windows")]
const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFFFFFF;

#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_READONLY: u32 = 0x0001;
#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0002;
#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0004;
#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;
#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0020;
#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_COMPRESSED: u32 = 0x0800;
#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_ENCRYPTED: u32 = 0x4000;
#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x0200;

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn GetFileAttributesW(lppathname: *const u16) -> u32;
}

/// Read the file's attributes via `GetFileAttributesW` and translate them
/// into our `FileAttrs` struct.
pub fn read_file_attrs(path: &Path) -> GResult<FileAttrs> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut wide: Vec<u16> = std::ffi::OsStr::new(path).encode_wide().collect();
        wide.push(0);
        // SAFETY: NUL-terminated UTF-16 buffer.
        let attrs = unsafe { GetFileAttributesW(wide.as_ptr()) };
        if attrs == INVALID_FILE_ATTRIBUTES {
            return Err(GError::Defrag(format!(
                "GetFileAttributesW({}) failed (Win32 error {})",
                path.display(),
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            )));
        }
        Ok(decode_attrs(attrs))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Ok(FileAttrs::default())
    }
}

#[cfg(target_os = "windows")]
fn decode_attrs(attrs: u32) -> FileAttrs {
    FileAttrs {
        compressed: attrs & FILE_ATTRIBUTE_COMPRESSED != 0,
        encrypted: attrs & FILE_ATTRIBUTE_ENCRYPTED != 0,
        sparse: attrs & FILE_ATTRIBUTE_SPARSE_FILE != 0,
        readonly: attrs & FILE_ATTRIBUTE_READONLY != 0,
        offline: attrs & FILE_ATTRIBUTE_OFFLINE != 0,
        reparse_point: attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0,
    }
}

/// Run the full safety check for one file.
///
/// This combines:
/// 1. Attribute check (skip COMPRESSED / ENCRYPTED / SPARSE / OFFLINE /
///    REPARSE_POINT).
/// 2. Lock check (try to open with no sharing — if it fails with a
///    sharing violation, the file is in use).
pub fn check_file_safety(path: &Path) -> GResult<FileSafety> {
    // Attribute filter first — cheap, doesn't touch the file.
    let attrs = read_file_attrs(path)?;
    if attrs.blocks_defrag() {
        return Ok(FileSafety::SkipAttrs(attrs.block_reason()));
    }

    // Lock check.
    match check_locked(path) {
        Ok(false) => Ok(FileSafety::Ok),
        Ok(true) => Ok(FileSafety::Locked),
        Err(_) => Ok(FileSafety::Inaccessible),
    }
}

/// `true` if `path` is currently held open by another process (sharing
/// violation when we try to open it for read with no sharing).
pub fn check_locked(path: &Path) -> GResult<bool> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        const GENERIC_READ: u32 = 0x80000000;
        const OPEN_EXISTING: u32 = 3;
        const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = -1isize as *mut std::ffi::c_void;

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
            fn CloseHandle(h: *mut std::ffi::c_void) -> i32;
        }

        let mut wide: Vec<u16> = std::ffi::OsStr::new(path).encode_wide().collect();
        wide.push(0);
        // Open with zero share mode — if anyone else has the file, this
        // fails with ERROR_SHARING_VIOLATION (32).
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                0, // no share
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            // 32 = ERROR_SHARING_VIOLATION, 33 = ERROR_LOCK_VIOLATION.
            return Ok(code == 32 || code == 33);
        }
        unsafe { let _ = CloseHandle(h); }
        Ok(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attrs_default_allows_defrag() {
        let a = FileAttrs::default();
        assert!(!a.blocks_defrag());
        assert_eq!(a.block_reason(), "");
    }

    #[test]
    fn attrs_compressed_blocks() {
        let a = FileAttrs { compressed: true, ..Default::default() };
        assert!(a.blocks_defrag());
        assert_eq!(a.block_reason(), "NTFS-compressed");
    }

    #[test]
    fn attrs_encrypted_blocks() {
        let a = FileAttrs { encrypted: true, ..Default::default() };
        assert!(a.blocks_defrag());
        assert_eq!(a.block_reason(), "EFS-encrypted");
    }

    #[test]
    fn attrs_sparse_blocks() {
        let a = FileAttrs { sparse: true, ..Default::default() };
        assert!(a.blocks_defrag());
        assert_eq!(a.block_reason(), "sparse");
    }

    #[test]
    fn attrs_reparse_blocks() {
        let a = FileAttrs { reparse_point: true, ..Default::default() };
        assert!(a.blocks_defrag());
        assert_eq!(a.block_reason(), "reparse point (symlink/junction)");
    }

    #[test]
    fn attrs_offline_blocks() {
        let a = FileAttrs { offline: true, ..Default::default() };
        assert!(a.blocks_defrag());
        assert_eq!(a.block_reason(), "offline (HSM)");
    }

    #[test]
    fn attrs_readonly_does_not_block() {
        // Readonly files can still be moved (the move only changes
        // physical placement, not content).
        let a = FileAttrs { readonly: true, ..Default::default() };
        assert!(!a.blocks_defrag());
    }

    #[test]
    fn attrs_compressed_takes_priority_over_others() {
        // The block_reason() short-circuits: compressed wins over the rest.
        let a = FileAttrs {
            compressed: true, encrypted: true, sparse: true,
            ..Default::default()
        };
        assert_eq!(a.block_reason(), "NTFS-compressed");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn read_file_attrs_empty_off_windows() {
        let a = read_file_attrs(Path::new("/tmp/somefile")).unwrap();
        assert!(!a.blocks_defrag());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn check_locked_returns_false_off_windows() {
        assert_eq!(check_locked(Path::new("/tmp/somefile")).unwrap(), false);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn check_file_safety_ok_off_windows() {
        // On non-Windows the attribute check returns default (no flags)
        // and the lock check returns false → file is "Ok" for planning
        // purposes. The command dispatcher still gates the actual move
        // engine on Windows.
        let s = check_file_safety(Path::new("/tmp/somefile")).unwrap();
        assert_eq!(s, FileSafety::Ok);
    }
}
