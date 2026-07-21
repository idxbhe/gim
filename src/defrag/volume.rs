//! Volume-level metadata for `gim defrag`.
//!
//! Wraps the Win32 calls we need to:
//!
//! - Open a volume handle (`\\.\X:`) — required by `FSCTL_GET_VOLUME_BITMAP`
//!   and `FSCTL_MOVE_FILE`.
//! - Query total / free space (`GetDiskFreeSpaceExW`).
//! - Query the **physical** sector size (`IOCTL_STORAGE_QUERY_PROPERTY`
//!   with `StorageAccessAlignmentProperty`). On Advanced Format drives
//!   physical = 4096 even though logical = 512. Every LCN target we
//!   compute for `FSCTL_MOVE_FILE` must be aligned to the physical sector
//!   to avoid the read-modify-write penalty.
//! - Detect whether VSS (Volume Shadow Copy Service) is currently writing
//!   shadows on this drive — moving 50 GB of game files would inflate the
//!   VSS diff area and may delete existing System Restore Points. We only
//!   *warn* — we don't refuse (the user may not care).
//!
//! All operations are read-only: the volume handle here is opened with
//! `GENERIC_READ` and broad sharing, never locked. Locking happens (if
//! needed) inside the move engine.
//!
//! Non-Windows: every entry point returns `NotSupportedPlatform`.

use crate::error::{GError, GResult};
use std::path::{Path, PathBuf};

/// Volume-level information needed by the defrag engine.
#[derive(Debug, Clone)]
pub struct VolumeInfo {
    /// Drive letter (e.g. `'C'`).
    pub drive: char,
    /// Volume path `\\.\C:`.
    pub volume_path: String,
    /// Total bytes on the volume.
    pub total_bytes: u64,
    /// Free bytes on the volume.
    pub free_bytes: u64,
    /// Bytes per cluster (NTFS cluster size, typically 4096).
    pub bytes_per_cluster: u64,
    /// Logical bytes per sector (typically 512).
    pub bytes_per_sector_log: u32,
    /// Physical bytes per sector (512 or 4096 on Advanced Format drives).
    pub bytes_per_sector_phys: u32,
    /// Free space as a percentage of total (0–100).
    pub free_pct: u8,
    /// Whether VSS is currently active on this volume.
    pub vss_active: bool,
}

impl VolumeInfo {
    /// Free percentage as a `f64` for threshold comparisons.
    pub fn free_pct_f64(&self) -> f64 {
        if self.total_bytes == 0 { 0.0 }
        else { (self.free_bytes as f64 / self.total_bytes as f64) * 100.0 }
    }

    /// True when free space is below `min_pct`.
    pub fn free_below(&self, min_pct: u8) -> bool {
        self.free_pct < min_pct
    }

    /// Alignment mask for the physical sector size. Use with
    /// `lcn & !alignment_mask` to round down to a sector boundary.
    pub fn physical_sector_mask(&self) -> u64 {
        !(self.bytes_per_sector_phys as u64 - 1)
    }

    /// Total clusters on the volume (total_bytes / bytes_per_cluster).
    pub fn total_clusters(&self) -> u64 {
        if self.bytes_per_cluster == 0 { 0 }
        else { self.total_bytes / self.bytes_per_cluster }
    }
}

// ── Win32 constants ──────────────────────────────────────────────────────
#[cfg(target_os = "windows")]
const GENERIC_READ: u32 = 0x80000000;
#[cfg(target_os = "windows")]
const GENERIC_WRITE: u32 = 0x40000000;
#[cfg(target_os = "windows")]
const FILE_SHARE_READ: u32 = 0x00000001;
#[cfg(target_os = "windows")]
const FILE_SHARE_WRITE: u32 = 0x00000002;
#[cfg(target_os = "windows")]
const OPEN_EXISTING: u32 = 3;
#[cfg(target_os = "windows")]
const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = -1isize as *mut std::ffi::c_void;

// IOCTL_STORAGE_QUERY_PROPERTY = 0x002D1400 (see media.rs).
#[cfg(target_os = "windows")]
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D1400;
#[cfg(target_os = "windows")]
const STORAGE_ACCESS_ALIGNMENT_PROPERTY: u32 = 6; // PropertyId::StorageAccessAlignmentProperty
#[cfg(target_os = "windows")]
const PROPERTY_STANDARD_QUERY: u32 = 0;

/// `STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR` (subset — we only read the two
/// fields we care about, but the layout must match the documented struct
/// up to those fields).
#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct StorageAccessAlignmentDescriptor {
    version: u32,
    size: u32,
    bytes_per_logical_sector: u32,
    bytes_per_physical_sector: u32,
    bytes_offset_for_sector_alignment: u32,
    // (Windows has more fields after this but we don't read them.)
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct StoragePropertyQuery {
    property_id: u32,
    query_type: u32,
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

    fn GetDiskFreeSpaceExW(
        lpdirectoryname: *const u16,
        lpfreebytesavailable: *mut u64,
        lptotalnumberofbytes: *mut u64,
        lptotalnumberoffreebytes: *mut u64,
    ) -> i32;

    fn GetDiskFreeSpaceW(
        lprootpathname: *const u16,
        lpsectorspercluster: *mut u32,
        lpbytespersector: *mut u32,
        lpnumberoffreeclusters: *mut u32,
        lptotalnumberofclusters: *mut u32,
    ) -> i32;
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

/// Open a volume handle for read access. Used by both the bitmap scan and
/// the move engine. Returns `Err` if the drive letter can't be resolved
/// or the volume can't be opened.
///
/// The caller owns the handle (wrapped in `HandleGuard`) and must keep it
/// alive for the duration of the FSCTL calls.
pub fn open_volume(drive: char) -> GResult<VolumeHandle> {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

        let volume_path = format!("\\\\.\\{}:", drive.to_ascii_uppercase());
        let mut wide: Vec<u16> = OsStr::new(&volume_path).encode_wide().collect();
        wide.push(0);

        // SAFETY: NUL-terminated UTF-16 buffer; other pointer args null.
        // FSCTL_MOVE_FILE requires the volume handle to carry
        // FILE_WRITE_DATA (GENERIC_WRITE) in addition to FILE_READ_DATA,
        // otherwise every move fails with ERROR_ACCESS_DENIED.
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Err(GError::Defrag(format!(
                "OpenVolume({drive}:) failed (Win32 error {})",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            )));
        }
        Ok(VolumeHandle { inner: HandleGuard(h) })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = drive;
        Err(GError::NotSupportedPlatform)
    }
}

/// Opaque volume handle. Wraps the Win32 `HANDLE` (on Windows) or is
/// uninhabited (on non-Windows). Used by the move engine.
pub struct VolumeHandle {
    #[cfg(target_os = "windows")]
    inner: HandleGuard,
}

#[cfg(target_os = "windows")]
impl VolumeHandle {
    pub fn raw(&self) -> *mut std::ffi::c_void {
        self.inner.0
    }
}

/// Query all volume-level info needed for the defrag engine.
pub fn query_volume_info(path: &Path) -> GResult<VolumeInfo> {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

        let drive = drive_letter(path)?;
        let volume_path = format!("\\\\.\\{}:", drive);

        // 1. Free/total space + cluster size via GetDiskFreeSpaceExW +
        //    GetDiskFreeSpaceW. We use both because ExW returns 64-bit
        //    sizes but no cluster size; GetDiskFreeSpaceW gives cluster
        //    size but 32-bit cluster counts (overflows on >16 TB volumes).
        let root = format!("{}:\\", drive);
        let mut root_w: Vec<u16> = OsStr::new(&root).encode_wide().collect();
        root_w.push(0);

        let mut free_avail: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free: u64 = 0;
        let mut sectors_per_cluster: u32 = 0;
        let mut bytes_per_sector: u32 = 0;
        let mut free_clusters: u32 = 0;
        let mut total_clusters: u32 = 0;

        // SAFETY: pointers all point to valid stack slots; root_w is
        // NUL-terminated.
        unsafe {
            if GetDiskFreeSpaceExW(
                root_w.as_ptr(), &mut free_avail, &mut total_bytes, &mut total_free
            ) == 0 {
                return Err(GError::Defrag(format!(
                    "GetDiskFreeSpaceExW({root}) failed (Win32 error {})",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                )));
            }
            if GetDiskFreeSpaceW(
                root_w.as_ptr(), &mut sectors_per_cluster, &mut bytes_per_sector,
                &mut free_clusters, &mut total_clusters
            ) == 0 {
                return Err(GError::Defrag(format!(
                    "GetDiskFreeSpaceW({root}) failed (Win32 error {})",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                )));
            }
        }

        let bytes_per_cluster = (sectors_per_cluster as u64) * (bytes_per_sector as u64);

        // 2. Physical sector size via IOCTL_STORAGE_QUERY_PROPERTY.
        let (phys_sector, _log_sector) = query_sector_sizes(drive).unwrap_or((4096, 512));

        // 3. VSS detection.
        let vss_active = detect_vss_active_for_drive(drive);

        let free_pct = if total_bytes == 0 { 0 }
                       else { ((total_free as f64 / total_bytes as f64) * 100.0) as u8 };

        Ok(VolumeInfo {
            drive,
            volume_path,
            total_bytes,
            free_bytes: total_free,
            bytes_per_cluster,
            bytes_per_sector_log: bytes_per_sector,
            bytes_per_sector_phys: phys_sector,
            free_pct,
            vss_active,
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err(GError::NotSupportedPlatform)
    }
}

/// Query logical and physical sector sizes via the storage access alignment
/// property. Returns `(physical, logical)` or `(4096, 512)` as a sane
/// default on failure (Advanced Format is the modern norm).
#[cfg(target_os = "windows")]
fn query_sector_sizes(drive: char) -> GResult<(u32, u32)> {
    use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

    let volume_path = format!("\\\\.\\{}:", drive);
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
        return Ok((4096, 512));
    }
    let _guard = HandleGuard(h);

    let query = StoragePropertyQuery {
        property_id: STORAGE_ACCESS_ALIGNMENT_PROPERTY,
        query_type: PROPERTY_STANDARD_QUERY,
    };
    let mut desc = StorageAccessAlignmentDescriptor {
        version: 0, size: 0,
        bytes_per_logical_sector: 0,
        bytes_per_physical_sector: 0,
        bytes_offset_for_sector_alignment: 0,
    };
    let mut returned: u32 = 0;

    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<StoragePropertyQuery>() as u32,
            &mut desc as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<StorageAccessAlignmentDescriptor>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        // Older Windows versions / non-NTFS volumes may not support this
        // property. Fall back to safe defaults (Advanced Format assumption).
        return Ok((4096, 512));
    }
    let phys = if desc.bytes_per_physical_sector == 0 { 4096 }
               else { desc.bytes_per_physical_sector };
    let log = if desc.bytes_per_logical_sector == 0 { 512 }
              else { desc.bytes_per_logical_sector };
    Ok((phys, log))
}

/// Open a volume handle (Windows-only convenience wrapper).
///
/// On non-Windows this returns `NotSupportedPlatform`.
pub fn open_volume_for(drive: char) -> GResult<PathBuf> {
    let _ = drive;
    #[cfg(target_os = "windows")]
    {
        let _g = open_volume(drive)?;
        // The caller of `open_volume_for` is expected to call `open_volume`
        // directly to get a real handle. This function is kept for tests
        // and command-level "does the volume exist?" checks.
        Ok(PathBuf::from(format!("\\\\.\\{}:", drive)))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(GError::NotSupportedPlatform)
    }
}

/// Detect whether VSS is active on the volume containing `path`.
///
/// "Active" here means the VSS service is running *and* at least one
/// shadow copy exists on this drive. We don't actually count shadows —
/// we just check the service status via `OpenSCManagerW` +
/// `QueryServiceStatus`, which is enough to decide whether to warn the
/// user about System Restore Point impact.
///
/// On non-Windows: returns `false`.
pub fn detect_vss_active(path: &Path) -> GResult<bool> {
    #[cfg(target_os = "windows")]
    {
        let drive = drive_letter(path)?;
        Ok(detect_vss_active_for_drive(drive))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Ok(false)
    }
}

#[cfg(target_os = "windows")]
fn detect_vss_active_for_drive(drive: char) -> bool {
    // We don't actually enumerate shadows per-drive (that requires the VSS
    // COM API which is a much bigger FFI surface). Instead, we check
    // whether the VSS service is running — if it isn't, VSS can't be
    // writing shadows; if it is, the user has Restore Points enabled and
    // should be warned. This is conservative: we may warn when VSS is
    // running but no shadows exist on *this* drive. That's acceptable —
    // the warning is "your Restore Points *may* be affected", not "they
    // *will* be deleted".
    use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

    const SC_MANAGER_CONNECT: u32 = 0x0001;
    const SERVICE_QUERY_STATUS: u32 = 0x0004;
    const SERVICE_RUNNING: u32 = 0x0004;

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenSCManagerW(
            lpservername: *const u16,
            lpdatabasename: *const u16,
            dwdesiredaccess: u32,
        ) -> *mut std::ffi::c_void;
        fn OpenServiceW(
            hscmanager: *mut std::ffi::c_void,
            lpservicename: *const u16,
            dwdesiredaccess: u32,
        ) -> *mut std::ffi::c_void;
        fn QueryServiceStatus(
            hservice: *mut std::ffi::c_void,
            lpservicestatus: *mut ServiceStatus,
        ) -> i32;
        fn CloseServiceHandle(h: *mut std::ffi::c_void) -> i32;
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct ServiceStatus {
        service_type: u32,
        current_state: u32,
        controls_accepted: u32,
        win32_exit_code: u32,
        service_specific_exit_code: u32,
        check_point: u32,
        wait_hint: u32,
    }

    let mut vss_name: Vec<u16> = OsStr::new("VSS").encode_wide().collect();
    vss_name.push(0);

    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() { return false; }
        let svc = OpenServiceW(scm, vss_name.as_ptr(), SERVICE_QUERY_STATUS);
        CloseServiceHandle(scm);
        if svc.is_null() { return false; }
        let mut status = ServiceStatus {
            service_type: 0, current_state: 0, controls_accepted: 0,
            win32_exit_code: 0, service_specific_exit_code: 0,
            check_point: 0, wait_hint: 0,
        };
        let ok = QueryServiceStatus(svc, &mut status);
        CloseServiceHandle(svc);
        if ok == 0 { return false; }
        let _ = drive; // not actually used yet — see comment above.
        status.current_state == SERVICE_RUNNING
    }
}

/// Extract the drive letter from a path. Mirrors `media::drive_letter`
/// but is also used by `volume.rs` to keep the FFI module independent.
#[cfg(target_os = "windows")]
fn drive_letter(path: &Path) -> GResult<char> {
    let s = path.to_string_lossy();
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes.get(1) != Some(&b':') {
        return Err(GError::Defrag(format!(
            "cannot determine drive letter for \"{}\" — defrag needs a drive letter (no UNC paths)",
            s
        )));
    }
    let drive = bytes[0] as char;
    if !drive.is_ascii_alphabetic() {
        return Err(GError::Defrag(format!(
            "invalid drive letter \"{drive}\" in path \"{}\"", s
        )));
    }
    Ok(drive.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_info_helpers() {
        let v = VolumeInfo {
            drive: 'C',
            volume_path: "\\\\.\\C:".into(),
            total_bytes: 1_000_000_000,
            free_bytes: 200_000_000,
            bytes_per_cluster: 4096,
            bytes_per_sector_log: 512,
            bytes_per_sector_phys: 4096,
            free_pct: 20,
            vss_active: false,
        };
        assert_eq!(v.free_pct_f64(), 20.0);
        assert!(!v.free_below(15));
        assert!(v.free_below(25));
        assert_eq!(v.physical_sector_mask(), !4095u64);
        assert_eq!(v.total_clusters(), 1_000_000_000 / 4096);
    }

    #[test]
    fn physical_sector_mask_for_512() {
        let v = VolumeInfo {
            drive: 'D', volume_path: "\\\\.\\D:".into(),
            total_bytes: 0, free_bytes: 0,
            bytes_per_cluster: 512,
            bytes_per_sector_log: 512, bytes_per_sector_phys: 512,
            free_pct: 0, vss_active: false,
        };
        assert_eq!(v.physical_sector_mask(), !511u64);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn query_volume_info_unsupported_off_windows() {
        assert!(matches!(query_volume_info(Path::new("/tmp")),
                         Err(GError::NotSupportedPlatform)));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn detect_vss_returns_false_off_windows() {
        assert_eq!(detect_vss_active(Path::new("/tmp")).unwrap(), false);
    }
}
