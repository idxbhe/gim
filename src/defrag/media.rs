//! Stage 2 — detect whether the volume backing a game folder is HDD or SSD.
//!
//! Defragmenting an SSD has zero performance benefit (random access is
//! already constant-time) and burns write endurance. Windows' own
//! `defrag.exe` refuses to do anything beyond TRIM on SSDs, and so do we.
//!
//! # Detection strategy
//!
//! We query `IOCTL_STORAGE_QUERY_PROPERTY` with
//! `PropertyId = StorageDeviceSeekPenaltyProperty` (`PropertyStandardQuery`).
//! The device returns a `DEVICE_SEEK_PENALTY_DESCRIPTOR` whose
//! `IncursSeekPenalty` field is `TRUE` (1) for HDDs and `FALSE` (0) for
//! SSDs. This is the same mechanism `FSCTL_IS_VOLUME_DIRTY`-style helpers
//! and PowerShell's `Get-PhysicalMedia | SeekPenalty` use.
//!
//! Alternative: `FSCTL_QUERY_STORAGE_CLASSES` (returns a "tier" list),
//! but seek penalty is simpler and matches what Windows itself checks.
//!
//! # Volume handle
//!
//! We open `\\.\X:` (where X is the drive letter extracted from the game
//! directory) with `GENERIC_READ` and broad sharing. No write access is
//! needed for the query — the volume handle here is *not* locked.
//!
//! # Non-Windows
//!
//! `detect_media_kind` returns `MediaKind::Ssd` (defrag will refuse to
//! proceed — same as our SSD path on Windows, which is the safe default).

use crate::error::{GError, GResult};
use std::path::Path;

/// What kind of storage backs a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// Spinning rust — defrag helps.
    Hdd,
    /// Solid state — defrag hurts. TRIM only (or skip entirely).
    Ssd,
    /// Unknown — treat as SSD (safe default: don't defrag).
    Unknown,
}

impl MediaKind {
    pub fn is_hdd(self) -> bool { matches!(self, Self::Hdd) }
    pub fn is_ssd(self) -> bool { matches!(self, Self::Ssd | Self::Unknown) }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hdd => "hdd",
            Self::Ssd => "ssd",
            Self::Unknown => "unknown",
        }
    }
}

// ── Win32 constants ──────────────────────────────────────────────────────
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

// IOCTL_STORAGE_QUERY_PROPERTY = CTL_CODE(IOCTL_STORAGE_BASE, 0x0500, METHOD_BUFFERED, FILE_ANY_ACCESS)
// where IOCTL_STORAGE_BASE = 0x0000002D.
#[cfg(target_os = "windows")]
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D1400;

// StorageDeviceProperty = 0 (used for bus-type fallback).
#[cfg(target_os = "windows")]
const STORAGE_DEVICE_PROPERTY: u32 = 0;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_UNKNOWN: u32 = 0;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_SCSI: u32 = 1;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_ATAPI: u32 = 2;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_ATA: u32 = 3;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_1394: u32 = 4;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_SSA: u32 = 5;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_FIBRE: u32 = 6;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_USB: u32 = 7;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_RAID: u32 = 8;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_ISCSI: u32 = 9;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_SAS: u32 = 10;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_SATA: u32 = 11;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_SD: u32 = 12; // Secure Digital.
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_MMC: u32 = 13; // Multimedia Card.
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_VIRTUAL: u32 = 14;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_FILE_BACKED_VIRTUAL: u32 = 15;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_UFS: u32 = 16;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_SCM: u32 = 17;
#[cfg(target_os = "windows")]
const STORAGE_BUS_TYPE_NVME: u32 = 18;

/// Bus type reported by
/// [STORAGE_DEVICE_DESCRIPTOR::BusType](https://learn.microsoft.com/windows/win32/api/winioctl/ns-winioctl-storage_device_descriptor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum BusType {
    Unknown = STORAGE_BUS_TYPE_UNKNOWN,
    Scsi = STORAGE_BUS_TYPE_SCSI,
    Atapi = STORAGE_BUS_TYPE_ATAPI,
    Ata = STORAGE_BUS_TYPE_ATA,
    Ieee1394 = STORAGE_BUS_TYPE_1394,
    Ssa = STORAGE_BUS_TYPE_SSA,
    Fibre = STORAGE_BUS_TYPE_FIBRE,
    Usb = STORAGE_BUS_TYPE_USB,
    Raid = STORAGE_BUS_TYPE_RAID,
    Iscsi = STORAGE_BUS_TYPE_ISCSI,
    Sas = STORAGE_BUS_TYPE_SAS,
    Sata = STORAGE_BUS_TYPE_SATA,
    Sd = STORAGE_BUS_TYPE_SD,
    Mmc = STORAGE_BUS_TYPE_MMC,
    Virtual = STORAGE_BUS_TYPE_VIRTUAL,
    FileBackedVirtual = STORAGE_BUS_TYPE_FILE_BACKED_VIRTUAL,
    Ufs = STORAGE_BUS_TYPE_UFS,
    Scm = STORAGE_BUS_TYPE_SCM,
    Nvme = STORAGE_BUS_TYPE_NVME,
}

impl From<u32> for BusType {
    fn from(v: u32) -> Self {
        match v {
            0 => Self::Unknown,
            1 => Self::Scsi,
            2 => Self::Atapi,
            3 => Self::Ata,
            4 => Self::Ieee1394,
            5 => Self::Ssa,
            6 => Self::Fibre,
            7 => Self::Usb,
            8 => Self::Raid,
            9 => Self::Iscsi,
            10 => Self::Sas,
            11 => Self::Sata,
            12 => Self::Sd,
            13 => Self::Mmc,
            14 => Self::Virtual,
            15 => Self::FileBackedVirtual,
            16 => Self::Ufs,
            17 => Self::Scm,
            18 => Self::Nvme,
            _ => Self::Unknown,
        }
    }
}

impl From<u8> for BusType {
    fn from(v: u8) -> Self { Self::from(v as u32) }
}

impl From<u16> for BusType {
    fn from(v: u16) -> Self { Self::from(v as u32) }
}

impl BusType {
    /// Best-effort mapping from bus to likely media kind.
    /// Returns [`None`] when the bus type gives no strong signal (USB, virtual,
    /// ATA/ATAPI, etc.) — those remain [`MediaKind::Unknown`].
    pub fn media_kind(self) -> Option<MediaKind> {
        // Known HDD buses:
        match self {
            Self::Sata | Self::Scsi | Self::Sas | Self::Ata | Self::Atapi => Some(MediaKind::Hdd),
            // Known solid-state / non-spinning buses:
            Self::Nvme | Self::Ufs | Self::Scm | Self::Virtual | Self::Raid => Some(MediaKind::Ssd),
            // USB / 1394 / Fibre / VHD / SD / MMC could be HDD or SSD — don't guess:
            _ => None,
        }
    }
}

/// Header shared by every `STORAGE_DEVICE_DESCRIPTOR` queried with
/// `IOCTL_STORAGE_QUERY_PROPERTY / StorageDeviceProperty`.
///
/// The descriptor sits in a single output buffer; we only need the first
/// four fields for the bus-type fallback.
#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct StorageDeviceDescriptorHeader {
    version: u32,
    size: u32,
    device_type: u8,
    device_type_modifier: u8,
    bus_type: u8,
    _pad: [u8; 5],
}

// PropertyId::StorageDeviceSeekPenaltyProperty = 3 (StorageDeviceProperty=0, StorageAdapterProperty=1,
// StorageDeviceIdProperty=2, StorageDeviceSeekPenaltyProperty=3).
#[cfg(target_os = "windows")]
const STORAGE_DEVICE_SEEK_PENALTY_PROPERTY: u32 = 3;

// PropertyQueryType::PropertyStandardQuery = 0 (PropertyExistsQuery=1, PropertyMaskQuery=2,
// PropertyQueryMax=3).
#[cfg(target_os = "windows")]
const PROPERTY_STANDARD_QUERY: u32 = 0;

/// `STORAGE_PROPERTY_QUERY` (8 bytes: PropertyId u32 + PropertyQueryType u32).
#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct StoragePropertyQuery {
    property_id: u32,
    query_type: u32,
    // AdditionalParameters[] — empty for the query we make.
}

/// `DEVICE_SEEK_PENALGY_DESCRIPTOR` (8 bytes: Version u32 + Size u32 +
/// IncursSeekPenalty u8 + 3 bytes padding).
#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
struct DeviceSeekPenaltyDescriptor {
    version: u32,
    size: u32,
    incurs_seek_penalty: u8,
    // 3 bytes of implicit padding to align the struct to 4 bytes.
    _pad: [u8; 3],
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

/// RAII guard for a Win32 `HANDLE`.
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

/// Detect the media kind backing `path`.
///
/// `path` may be a file or directory; we extract its drive letter and
/// open the volume `\\.\X:`.
pub fn detect_media_kind(path: &Path) -> GResult<MediaKind> {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

        let drive = drive_letter(path)?;
        let volume_path = format!("\\\\.\\{}:", drive);
        let mut wide: Vec<u16> = OsStr::new(&volume_path).encode_wide().collect();
        wide.push(0);

        // Open the volume. We use FILE_SHARE_WRITE so we don't block other
        // processes (defrag pre-flight should not interfere with running
        // apps).
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
            // Don't abort — treat unknown as SSD (safe default).
            return Ok(MediaKind::Unknown);
        }
        let _guard = HandleGuard(h);

        // Build the STORAGE_PROPERTY_QUERY.
        let query = StoragePropertyQuery {
            property_id: STORAGE_DEVICE_SEEK_PENALTY_PROPERTY,
            query_type: PROPERTY_STANDARD_QUERY,
        };
        let mut descriptor = DeviceSeekPenaltyDescriptor {
            version: 0, size: 0, incurs_seek_penalty: 0, _pad: [0; 3],
        };
        let mut returned: u32 = 0;

        // SAFETY: handle from CreateFileW; in/out buffers are valid stack
        // slots with the documented sizes.
        let ok = unsafe {
            DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &query as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<StoragePropertyQuery>() as u32,
                &mut descriptor as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<DeviceSeekPenaltyDescriptor>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            // The sought-penalty IOCTL can fail on certain drives / drivers
            // (older USB-SATA bridges, some RAID/HBA setups, etc.) even when
            // the device is an HDD. Fall back to StorageDeviceProperty and
            // infer media kind from the bus type — SATA/SCSI/SAS/ATA are
            // very likely HDDs; NVMe/UFS/SCM are SSDs. Other buses (USB,
            // 1394, SD/MMC, virtual) can be either way, so we stay Unknown.
            return Ok(fallback_bus(h));
        }
        Ok(if descriptor.incurs_seek_penalty != 0 {
            MediaKind::Hdd
        } else {
            MediaKind::Ssd
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Ok(MediaKind::Ssd)
    }
}

#[cfg(target_os = "windows")]
fn fallback_bus(h: *mut std::ffi::c_void) -> MediaKind {
    let query = StoragePropertyQuery {
        property_id: STORAGE_DEVICE_PROPERTY,
        query_type: PROPERTY_STANDARD_QUERY,
    };

    let mut buf = [0u8; 256];
    let mut ret: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<StoragePropertyQuery>() as u32,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            buf.len() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        const ERROR_MORE_DATA: i32 = 234;
        if err != ERROR_MORE_DATA {
            return MediaKind::Unknown;
        }
        let required = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
        if required == 0 || required > 64 * 1024 {
            return MediaKind::Unknown;
        }
        let mut buf2 = vec![0u8; required];
        let ok2 = unsafe {
            DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &query as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<StoragePropertyQuery>() as u32,
                buf2.as_mut_ptr() as *mut std::ffi::c_void,
                buf2.len() as u32,
                &mut ret,
                std::ptr::null_mut(),
            )
        };
        if ok2 == 0 {
            return MediaKind::Unknown;
        }
        return bus_type_from_descriptor(&buf2);
    }

    bus_type_from_descriptor(&buf)
}

#[cfg(target_os = "windows")]
fn bus_type_from_descriptor(buf: &[u8]) -> MediaKind {
    // STORAGE_DEVICE_DESCRIPTOR layout (offsets 0-based):
    //   0..4  Version
    //   4..8  Size
    //   8     DeviceType
    //   9     DeviceTypeModifier
    //   10    BusType
    if buf.len() <= 10 {
        return MediaKind::Unknown;
    }
    let bus_type = buf[10];
    match BusType::from(bus_type).media_kind() {
        Some(kind) => kind,
        None => MediaKind::Unknown,
    }
}

/// Extract the drive letter (e.g. `"C"`) from a Windows path.
///
/// Accepts both `C:\foo` and `C:/foo`. Returns an error if the path is a
/// UNC path (`\\server\share`) or a relative path with no drive.
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

/// Drive letter extraction exposed for tests on non-Windows too.
#[cfg(not(target_os = "windows"))]
pub fn drive_letter_str(path: &Path) -> Option<char> {
    let s = path.to_string_lossy();
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        Some((bytes[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_kind_classification() {
        assert!(MediaKind::Hdd.is_hdd());
        assert!(!MediaKind::Hdd.is_ssd());
        assert!(MediaKind::Ssd.is_ssd());
        assert!(MediaKind::Unknown.is_ssd()); // safe default
    }

    #[test]
    fn media_kind_as_str() {
        assert_eq!(MediaKind::Hdd.as_str(), "hdd");
        assert_eq!(MediaKind::Ssd.as_str(), "ssd");
        assert_eq!(MediaKind::Unknown.as_str(), "unknown");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn drive_letter_extracts_uppercase() {
        assert_eq!(drive_letter_str(Path::new("c:\\Games")), Some('C'));
        assert_eq!(drive_letter_str(Path::new("D:/foo")), Some('D'));
        assert_eq!(drive_letter_str(Path::new("\\\\server\\share")), None);
        assert_eq!(drive_letter_str(Path::new("relative")), None);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn detect_returns_ssd_off_windows() {
        // Safe default: don't claim HDD on a platform we can't actually
        // query — that would let the defrag engine proceed into a no-op
        // (or worse, an unsupported FSCTL path).
        assert_eq!(detect_media_kind(Path::new("/tmp")).unwrap(), MediaKind::Ssd);
    }
}
