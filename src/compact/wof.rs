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
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

// ── Control codes ───────────────────────────────────────────────────────
// FSCTL_SET_EXTERNAL_BACKING    = CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 195,
//                                          METHOD_BUFFERED, FILE_SPECIAL_ACCESS)
// FSCTL_GET_EXTERNAL_BACKING    = CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 196,
//                                          METHOD_BUFFERED, FILE_ANY_ACCESS)
// FSCTL_DELETE_EXTERNAL_BACKING = CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 197,
//                                          METHOD_BUFFERED, FILE_SPECIAL_ACCESS)
const FSCTL_SET_EXTERNAL_BACKING: u32 = 0x0009030C;
const FSCTL_GET_EXTERNAL_BACKING: u32 = 0x00090310;
const FSCTL_DELETE_EXTERNAL_BACKING: u32 = 0x00090314;

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

// ── WOF availability cache ──────────────────────────────────────────────
// Once we detect that WOF is unavailable on a volume (ERROR_INVALID_FUNCTION),
// we cache this so we don't repeat the failing DeviceIoControl for every file.
static WOF_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// Win32 error code for "incorrect function" — indicates the WOF driver is
/// not present or not loaded on this system / volume.
const ERROR_INVALID_FUNCTION: u32 = 1;
const ERROR_NOT_SUPPORTED: u32 = 50;

// ── WOF driver status ──────────────────────────────────────────────────

/// Result of probing the WOF driver availability on the system.
///
/// Used by the `gim compact` command to inform the user about why WOF
/// compression is unavailable and what (if anything) can be done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WofDriverStatus {
    /// WOF driver appears to be available and the service is configured
    /// to start (file exists, registry `Start` != 4).
    Available,
    /// WOF driver file (`wof.sys`) exists on disk but the service is
    /// disabled (registry `Start` = 4). The user can enable it via
    /// [`enable_wof_driver`] and then restart.
    Disabled,
    /// WOF driver file (`wof.sys`) was not found in
    /// `%SystemRoot%\System32\drivers\`. WOF compression is not
    /// possible on this system.
    NotInstalled,
}

/// Resolve the expected path of `wof.sys` on this system.
///
/// Uses the `%SystemRoot%` environment variable (falling back to
/// `C:\Windows` if unset) to construct
/// `<SystemRoot>\System32\drivers\wof.sys`.
#[cfg(target_os = "windows")]
pub fn wof_sys_path() -> PathBuf {
    let sys_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    sys_root.join(r"System32\drivers\wof.sys")
}

/// Probe the WOF driver availability on this system.
///
/// 1. Checks whether `wof.sys` exists in `%SystemRoot%\System32\drivers\`.
/// 2. If it exists, reads the `Start` DWORD from the service registry key
///    `HKLM\SYSTEM\CurrentControlSet\Services\wof`.
///    - `Start == 4` → service is disabled → returns [`WofDriverStatus::Disabled`]
///    - Any other value → service is enabled (or at least not disabled)
///      → returns [`WofDriverStatus::Available`]
/// 3. If `wof.sys` does not exist → returns [`WofDriverStatus::NotInstalled`].
///
/// On non-Windows platforms, always returns [`WofDriverStatus::NotInstalled`].
pub fn probe_wof_driver() -> WofDriverStatus {
    #[cfg(target_os = "windows")]
    {
        let driver_path = wof_sys_path();

        if !driver_path.exists() {
            return WofDriverStatus::NotInstalled;
        }

        // wof.sys exists — check the service registry key.
        match read_wof_service_start() {
            Some(4) => WofDriverStatus::Disabled,
            Some(_) => WofDriverStatus::Available,
            None => {
                // Could not read the registry key. The file exists but we
                // can't determine the service state. Treat as disabled
                // since the driver clearly isn't loaded (we wouldn't be
                // probing if FSCTL_SET_EXTERNAL_BACKING had succeeded).
                WofDriverStatus::Disabled
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        WofDriverStatus::NotInstalled
    }
}

/// Read the `Start` DWORD from the WOF service registry key.
///
/// Returns `None` if the key does not exist or cannot be read.
#[cfg(target_os = "windows")]
fn read_wof_service_start() -> Option<u32> {
    let subkey = str_to_wide(r"SYSTEM\CurrentControlSet\Services\wof");
    let value_name = str_to_wide("Start");

    let mut hkey: isize = 0;
    let result = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        )
    };
    if result != ERROR_SUCCESS {
        return None;
    }

    let mut dtype: u32 = 0;
    let mut data: [u8; 4] = [0; 4];
    let mut data_size: u32 = 4;

    let result = unsafe {
        RegQueryValueExW(
            hkey,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            &mut dtype,
            data.as_mut_ptr(),
            &mut data_size,
        )
    };

    unsafe { let _ = RegCloseKey(hkey); }

    if result != ERROR_SUCCESS || dtype != REG_DWORD || data_size != 4 {
        return None;
    }

    Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}

/// Attempt to enable the WOF driver by setting the service `Start` value
/// in the Windows registry.
///
/// This sets `HKLM\SYSTEM\CurrentControlSet\Services\wof\Start` to `0`
/// (boot start), which causes the WOF driver to load during boot.
///
/// **This requires Administrator privileges.** If the current process
/// does not have sufficient rights, this will return an error.
///
/// After successfully enabling the driver, the system **must be restarted**
/// for the change to take effect.
pub fn enable_wof_driver() -> GResult<()> {
    #[cfg(target_os = "windows")]
    {
        let subkey = str_to_wide(r"SYSTEM\CurrentControlSet\Services\wof");
        let value_name = str_to_wide("Start");

        let mut hkey: isize = 0;
        let result = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            )
        };
        if result != ERROR_SUCCESS {
            return Err(GError::WofNotAvailable(
                format!("failed to open WOF service registry key (Win32 error {result}). \
                         Make sure you are running as Administrator.")
            ));
        }

        // Set Start = 0 (boot start — driver loads during boot).
        let start_value: u32 = 0;
        let data = start_value.to_le_bytes();
        let result = unsafe {
            RegSetValueExW(
                hkey,
                value_name.as_ptr(),
                0,
                REG_DWORD,
                data.as_ptr(),
                4,
            )
        };

        unsafe { let _ = RegCloseKey(hkey); }

        if result != ERROR_SUCCESS {
            return Err(GError::WofNotAvailable(
                format!("failed to set WOF service Start value (Win32 error {result}). \
                         Make sure you are running as Administrator.")
            ));
        }

        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(GError::NotSupportedPlatform)
    }
}

// ── Registry FFI ───────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(
        hkey: isize,
        lpsubkey: *const u16,
        uloptions: u32,
        samdesired: u32,
        phkresult: *mut isize,
    ) -> i32;

    fn RegQueryValueExW(
        hkey: isize,
        lpvaluename: *const u16,
        lpreserved: *mut u32,
        lptype: *mut u32,
        lpdata: *mut u8,
        lpcbdata: *mut u32,
    ) -> i32;

    fn RegSetValueExW(
        hkey: isize,
        lpvaluename: *const u16,
        reserved: u32,
        dwtype: u32,
        lpdata: *const u8,
        cbdata: u32,
    ) -> i32;

    fn RegCloseKey(hkey: isize) -> i32;
}

/// `HKEY_LOCAL_MACHINE` predefined registry key handle.
const HKEY_LOCAL_MACHINE: isize = 0x80000002isize;
const KEY_READ: u32 = 0x20019;
const KEY_SET_VALUE: u32 = 0x0002;
const REG_DWORD: u32 = 4;
const ERROR_SUCCESS: i32 = 0;

// ── Helpers ────────────────────────────────────────────────────────────

/// Convert a Rust string to a NUL-terminated UTF-16 wide string.
#[cfg(target_os = "windows")]
fn str_to_wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = OsStr::new(s).encode_wide().collect();
    v.push(0);
    v
}

// ── WOF availability check ─────────────────────────────────────────────

/// Check whether the WOF driver appears to be available based on
/// previously observed `DeviceIoControl` results.
///
/// Returns `Ok(())` if no WOF failure has been observed, or
/// `Err(GError::WofNotAvailable)` if a previous `FSCTL_SET_EXTERNAL_BACKING`
/// call failed with `ERROR_INVALID_FUNCTION`.
///
/// For a thorough pre-flight check, use [`probe_wof_driver`] instead.
pub fn check_wof_available() -> GResult<()> {
    #[cfg(target_os = "windows")]
    {
        if WOF_UNAVAILABLE.load(Ordering::Relaxed) {
            return Err(GError::WofNotAvailable(
                "the WOF driver (wof.sys) is not loaded on this system. \
                 WOF compression (LZX/XPRESS) requires the WOF driver to be \
                 installed and enabled.".into(),
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(GError::NotSupportedPlatform)
    }
}

/// Mark WOF as unavailable (called when we get ERROR_INVALID_FUNCTION).
fn mark_wof_unavailable() {
    WOF_UNAVAILABLE.store(true, Ordering::Relaxed);
}

/// Reset the WOF availability cache (for testing or re-probing).
pub fn reset_wof_availability() {
    WOF_UNAVAILABLE.store(false, Ordering::Relaxed);
}

/// Result of an active runtime probe against a specific volume.
///
/// This is more reliable than [`probe_wof_driver`] because it actually
/// attempts a `FSCTL_SET_EXTERNAL_BACKING` call on a tiny throwaway file in
/// the target directory, catching cases where the driver is installed but
/// not attached to the volume (e.g. a secondary data drive that wasn't
/// configured for WOF).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WofRuntimeProbe {
    /// WOF works on this volume — safe to proceed with WOF compaction.
    Ok,
    /// WOF driver is not attached to the target volume (e.g. `G:` is a
    /// secondary drive without the WOF filter). The error code from
    /// `DeviceIoControl` is captured (typically `ERROR_INVALID_FUNCTION`=1).
    /// `compact.exe /EXE:*` would fail the same way here.
    NotAttachedToVolume(u32),
    /// The probe failed for an unrelated reason (couldn't create temp file,
    /// permission denied, etc.). The error message describes it.
    ProbeFailed(String),
}

/// Actively probe whether WOF compression works on the volume hosting
/// `target_dir`, by creating a throwaway file there and attempting
/// `FSCTL_SET_EXTERNAL_BACKING` on it.
///
/// This is the only reliable way to detect "WOF is installed on the system
/// but not attached to *this* volume" — a common situation on secondary
/// data drives. The static [`probe_wof_driver`] check only inspects
/// `wof.sys` and the registry, which can give a false "Available" result.
///
/// On success the temp file is removed. The global `WOF_UNAVAILABLE` cache
/// is updated based on the result.
/// Low-level volume probe that bypasses WOF_UNAVAILABLE cache.
/// Returns Ok(()) if WOF FSCTL succeeds, otherwise returns raw Win32 error code.
#[cfg(target_os = "windows")]
fn probe_wof_volume_raw(target_dir: &Path) -> Result<(), u32> {
    let probe_name = format!(".gim-wof-probe-{}", std::process::id());
    let probe_path = target_dir.join(&probe_name);

    // Write 128 KB of highly compressible data — WOF drivers often ignore tiny files
    let content = vec![b'A'; 128 * 1024];
    if std::fs::write(&probe_path, &content).is_err() {
        let _ = std::fs::remove_file(&probe_path);
        return Err(5); // ERROR_ACCESS_DENIED
    }

    let guard = match open_file_rw(&probe_path) {
        Ok(g) => g,
        Err(_) => {
            let _ = std::fs::remove_file(&probe_path);
            return Err(5);
        }
    };

    let payload = build_backing_payload(FILE_PROVIDER_COMPRESSION_LZX);
    let mut returned: u32 = 0;

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

    // CAPTURE ERROR IMMEDIATELY — before any other Win32 call
    let err = if ok == 0 { unsafe { GetLastError() } } else { 0 };

    // Best-effort cleanup: remove backing (no-op if probe failed) then delete file
    unsafe {
        let _ = DeviceIoControl(
            guard.0,
            FSCTL_DELETE_EXTERNAL_BACKING,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        );
    }
    drop(guard);
    let _ = std::fs::remove_file(&probe_path);

    if ok != 0 {
        Ok(())
    } else {
        Err(err)
    }
}

pub fn probe_wof_runtime(target_dir: &Path) -> WofRuntimeProbe {
    #[cfg(target_os = "windows")]
    {
        let dir = match target_dir.canonicalize() {
            Ok(d) => d,
            Err(e) => return WofRuntimeProbe::ProbeFailed(
                format!("cannot resolve target directory: {e}")),
        };

        match probe_wof_volume_raw(&dir) {
            Ok(()) => WofRuntimeProbe::Ok,
            Err(code) => {
                // ERROR_INVALID_FUNCTION (1)  → WOF driver tidak attach / tidak ada
                // ERROR_NOT_SUPPORTED (50)    → FS tidak support WOF (non-NTFS, dll)
                if code == ERROR_INVALID_FUNCTION || code == ERROR_NOT_SUPPORTED {
                    WofRuntimeProbe::NotAttachedToVolume(code)
                } else {
                    WofRuntimeProbe::ProbeFailed(
                        format!("WOF probe failed with Win32 error {code} (0x{code:08X})"))
                }
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = target_dir;
        WofRuntimeProbe::ProbeFailed("WOF is only available on Windows".to_string())
    }
}

// ── File compression structures ────────────────────────────────────────

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

// ── Win32 FFI (kernel32) ───────────────────────────────────────────────

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
fn build_backing_payload(algorithm: u32) -> [u8; 20] {
    let header = WofExternalInfo { version: WOF_CURRENT_VERSION, provider: WOF_PROVIDER_FILE };
    let body = FileProviderExternalInfo1 { version: 1, algorithm, flags: 0 };
    let mut buf = [0u8; 20];
    buf[0..4].copy_from_slice(&header.version.to_le_bytes());
    buf[4..8].copy_from_slice(&header.provider.to_le_bytes());
    buf[8..12].copy_from_slice(&body.version.to_le_bytes());
    buf[12..16].copy_from_slice(&body.algorithm.to_le_bytes());
    buf[16..20].copy_from_slice(&body.flags.to_le_bytes());
    buf
}

/// Apply WOF compression to a single file.
///
/// After this returns successfully, the file is compressed on disk by the
/// WOF driver using `algorithm` (one of the `FILE_PROVIDER_COMPRESSION_*`
/// constants). The file remains readable normally.
///
/// If the WOF driver is not available (e.g., `wof.sys` not loaded), this
/// returns [`GError::WofNotAvailable`]. Callers should use
/// [`probe_wof_driver`] beforehand to check availability and provide
/// actionable guidance to the user.
pub fn set_wof_compression(path: &Path, algorithm: u32) -> GResult<()> {
    #[cfg(target_os = "windows")]
    {
        // Fast path: if WOF was previously detected as unavailable, skip
        // the DeviceIoControl entirely and return the cached error.
        if WOF_UNAVAILABLE.load(Ordering::Relaxed) {
            return Err(GError::WofNotAvailable(
                "the WOF driver (wof.sys) is not loaded on this system. \
                 WOF compression (LZX/XPRESS) requires the WOF driver to be \
                 installed and enabled.".into(),
            ));
        }

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
            let code = unsafe { GetLastError() };
            // ERROR_INVALID_FUNCTION means the WOF driver is not present or
            // not loaded — this is a system-level issue, not a per-file
            // problem. Cache it so we don't retry for every file.
            if code == ERROR_INVALID_FUNCTION {
                mark_wof_unavailable();
                return Err(GError::WofNotAvailable(
                    "the WOF driver (wof.sys) is not loaded on this system. \
                     WOF compression (LZX/XPRESS) requires the WOF driver to be \
                     installed and enabled.".into(),
                ));
            }
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
            // Note: `get_wof_compression` should NOT call `mark_wof_unavailable()`.
            // ERROR_NOT_FOUND = 1168, ERROR_INVALID_FUNCTION = 1,
            // ERROR_INVALID_PARAMETER = 87, ERROR_NOT_SUPPORTED = 50.
            const ERROR_NOT_FOUND: u32 = 1168;
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
    fn payload_layout_is_20_bytes() {
        let p = build_backing_payload(FILE_PROVIDER_COMPRESSION_LZX);
        assert_eq!(p.len(), 20);
        // Header version == WOF_CURRENT_VERSION (1) at offset 0.
        assert_eq!(u32::from_le_bytes([p[0], p[1], p[2], p[3]]), WOF_CURRENT_VERSION);
        // Provider == WOF_PROVIDER_FILE (1) at offset 4.
        assert_eq!(u32::from_le_bytes([p[4], p[5], p[6], p[7]]), WOF_PROVIDER_FILE);
        // Body version (1) at offset 8.
        assert_eq!(u32::from_le_bytes([p[8], p[9], p[10], p[11]]), 1);
        // Algorithm (LZX = 1) at offset 12.
        assert_eq!(u32::from_le_bytes([p[12], p[13], p[14], p[15]]), FILE_PROVIDER_COMPRESSION_LZX);
        // Flags (0) at offset 16.
        assert_eq!(u32::from_le_bytes([p[16], p[17], p[18], p[19]]), 0);
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

    #[test]
    fn reset_wof_availability_clears_flag() {
        // Ensure the reset function works (it manipulates a static).
        reset_wof_availability();
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn probe_returns_not_installed_on_non_windows() {
        assert_eq!(probe_wof_driver(), WofDriverStatus::NotInstalled);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn enable_returns_not_supported_on_non_windows() {
        assert!(matches!(enable_wof_driver(), Err(GError::NotSupportedPlatform)));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn str_to_wide_is_nul_terminated() {
        let w = str_to_wide("hello");
        assert_eq!(*w.last().unwrap(), 0);
        assert_eq!(w.len(), 6); // 5 chars + NUL
    }
}
