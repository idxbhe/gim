//! Folder scanning + savings estimation for `gim compact`.
//!
//! The estimate phase walks a target folder and classifies every file as
//! either a **candidate** (worth compressing) or **skip** (already
//! compressed / tiny / unrecognized). For candidates we apply a per-class
//! ratio to estimate post-compression size.
//!
//! The numbers produced here are estimates — actual savings are measured
//! against `size_on_disk` after the fact and reported in the final summary.
//! The estimate's job is to (a) let the user make an informed yes/no
//! decision and (b) warn when a folder is mostly already-compressed assets
//! where compaction would add overhead for little gain.

use crate::compact::algorithm::CompactAlgorithm;
use crate::compact::wof;
use crate::error::GResult;
use crate::output::ProgressReporter;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Files smaller than this are skipped — the chunk-table and per-file
/// metadata overhead negates any gain.
pub const MIN_COMPRESS_SIZE: u64 = 4 * 1024;

/// Reason a file was excluded from compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkipReason {
    /// Already WOF-compressed (or NTFS-compressed on Windows).
    AlreadyCompressed,
    /// Extension is a known precompressed media/container format.
    PrecompressedExt,
    /// File is below [`MIN_COMPRESS_SIZE`].
    TooSmall,
    /// No read access (locked, permission denied).
    Inaccessible,
}

impl SkipReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::AlreadyCompressed => "already compressed",
            Self::PrecompressedExt => "precompressed asset",
            Self::TooSmall => "small file (<4 KB)",
            Self::Inaccessible => "inaccessible",
        }
    }
}

/// One scanned file. Only fields needed for estimation are kept.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub size: u64,
    pub kind: FileKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Worth compressing — has a ratio we can estimate from.
    Candidate(FileClass),
    /// Skipped for the given reason.
    Skipped(SkipReason),
}

/// Coarse content class used to pick an estimated compression ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileClass {
    /// `.exe`, `.dll`, `.com`, `.sys`. Code + some compressible resources.
    Executable,
    /// `.txt`, `.log`, `.xml`, `.json`, `.ini`, `.csv`, `.lua`, `.html`.
    Text,
    /// `.bmp`, `.tga`, `.dds` (uncompressed variants), `.wav`.
    UncompressedMedia,
    /// Anything else we couldn't classify — conservative ratio.
    Other,
}

impl FileClass {
    /// Estimated compressed-size fraction (0.0–1.0) for the given algorithm.
    /// Lower = better compression.
    pub fn ratio(self, algo: CompactAlgorithm) -> f64 {
        match algo {
            CompactAlgorithm::Lzx => match self {
                Self::Executable => 0.55,
                Self::Text => 0.30,
                Self::UncompressedMedia => 0.55,
                Self::Other => 0.85,
            },
            CompactAlgorithm::Xpress4k => match self {
                Self::Executable => 0.65,
                Self::Text => 0.40,
                Self::UncompressedMedia => 0.65,
                Self::Other => 0.90,
            },
            CompactAlgorithm::Xpress8k => match self {
                Self::Executable => 0.62,
                Self::Text => 0.37,
                Self::UncompressedMedia => 0.62,
                Self::Other => 0.88,
            },
            CompactAlgorithm::Xpress16k => match self {
                Self::Executable => 0.60,
                Self::Text => 0.34,
                Self::UncompressedMedia => 0.60,
                Self::Other => 0.87,
            },
            CompactAlgorithm::Ntfs => match self {
                Self::Executable => 0.60,
                Self::Text => 0.35,
                Self::UncompressedMedia => 0.60,
                Self::Other => 0.90,
            },
            // Decompress: post-size ≈ original logical size.
            CompactAlgorithm::None => 1.00,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Estimate {
    pub total_files: u64,
    pub candidate_files: u64,
    pub skipped_files: u64,
    pub total_size: u64,
    pub candidate_size: u64,
    pub skipped_size: u64,
    /// Estimated logical bytes after compression (candidates only; skipped
    /// files are assumed to keep their current size).
    pub estimated_after: u64,
    /// `total_size - estimated_after`.
    pub estimated_savings: u64,
    /// Breakdown of skipped files by reason: (reason, count, size).
    pub skipped_breakdown: Vec<(SkipReason, u64, u64)>,
    /// Per-class candidate breakdown: (class, count, size).
    pub candidate_breakdown: Vec<(FileClass, u64, u64)>,
}

impl Estimate {
    pub fn savings_pct(&self) -> f64 {
        if self.total_size == 0 { 0.0 }
        else { (self.estimated_savings as f64 / self.total_size as f64) * 100.0 }
    }

    /// `true` when estimated savings are below the "worth it" threshold.
    pub fn low_savings(&self) -> bool {
        self.savings_pct() < 5.0
    }
}

/// Extension set for formats that are almost always already compressed —
/// compressing them again wastes CPU and rarely shrinks the file.
const PRECOMPRESSED_EXTS: &[&str] = &[
    // Archives
    "zip", "7z", "rar", "gz", "bz2", "xz", "zst", "tar", "cab", "iso",
    // Media — image
    "jpg", "jpeg", "png", "webp", "gif", "heic", "avif", "cr2", "nef",
    // Media — video
    "mp4", "mkv", "avi", "mov", "webm", "wmv", "m4v", "flv", "ts", "mpg", "mpeg",
    // Media — audio
    "mp3", "ogg", "opus", "aac", "m4a", "flac", "wma",
    // Game-specific packed assets (already compressed internally)
    "pak", "vpk", "bsa", "ba2", "cas", "cat", "sid", "sis",
    // Encrypted/packed
    "enc", "pex",
];

const EXECUTABLE_EXTS: &[&str] = &["exe", "dll", "com", "sys", "ocx", "drv"];
const TEXT_EXTS: &[&str] = &[
    "txt", "log", "xml", "json", "ini", "cfg", "conf", "csv", "lua",
    "html", "htm", "js", "ts", "py", "md", "yaml", "yml", "toml",
];
const UNCOMPRESSED_MEDIA_EXTS: &[&str] = &["bmp", "tga", "wav", "pcx", "psd"];

/// Classify a file by extension into a candidate class, or return `None`
/// if the extension is in the precompressed-skip list.
pub fn classify_ext(ext: &str) -> Option<FileClass> {
    let e = ext.to_ascii_lowercase();
    if PRECOMPRESSED_EXTS.iter().any(|x| *x == e) {
        return None;
    }
    if EXECUTABLE_EXTS.iter().any(|x| *x == e) {
        return Some(FileClass::Executable);
    }
    if TEXT_EXTS.iter().any(|x| *x == e) {
        return Some(FileClass::Text);
    }
    if UNCOMPRESSED_MEDIA_EXTS.iter().any(|x| *x == e) {
        return Some(FileClass::UncompressedMedia);
    }
    Some(FileClass::Other)
}

/// Walk `root` and classify every regular file. Files matching the
/// gitignore-style `exclude` patterns (via the same `ignore::WalkBuilder`
/// used elsewhere in the project) are not visited at all.
///
/// `already_compressed` checks (WOF backing probe) are only meaningful on
/// Windows; on other platforms that check is a no-op and we fall back to
/// extension-based classification.
pub fn scan(root: &Path, exclude: &[String], progress: &ProgressReporter) -> GResult<Vec<ScannedFile>> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder.hidden(false).parents(false).ignore(false).git_ignore(false)
        .git_global(false).git_exclude(false).follow_links(false)
        .threads(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));

    let mut out: Vec<ScannedFile> = Vec::new();
    // Excludes are applied post-walk via a gitignore matcher (the `ignore`
    // crate's parallel walker can't easily take a non-'static matcher).
    let mut exclude_matcher = build_exclude_matcher(exclude)?;

    progress.scan_start();
    for entry in builder.build() {
        let entry = match entry { Ok(e) => e, Err(_) => continue };
        let ft = match entry.file_type() { Some(ft) => ft, None => continue };
        if !ft.is_file() { continue; }

        let rel = match entry.path().strip_prefix(root) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if exclude_matcher_matches(&mut exclude_matcher, &rel) { continue; }

        let meta = match std::fs::symlink_metadata(entry.path()) {
            Ok(m) => m,
            Err(_) => {
                out.push(ScannedFile {
                    path: entry.path().to_path_buf(),
                    size: 0,
                    kind: FileKind::Skipped(SkipReason::Inaccessible),
                });
                progress.scan_tick();
                continue;
            }
        };
        let size = meta.len();

        // Tiny file → skip regardless of extension.
        if size < MIN_COMPRESS_SIZE {
            out.push(ScannedFile {
                path: entry.path().to_path_buf(),
                size,
                kind: FileKind::Skipped(SkipReason::TooSmall),
            });
            progress.scan_tick();
            continue;
        }

        // Already compressed by WOF → skip (Windows only; no-op elsewhere).
        if let Ok(Some(_)) = wof::get_wof_compression(entry.path()) {
            out.push(ScannedFile {
                path: entry.path().to_path_buf(),
                size,
                kind: FileKind::Skipped(SkipReason::AlreadyCompressed),
            });
            progress.scan_tick();
            continue;
        }

        // Extension-based classification.
        let ext = entry.path().extension()
            .and_then(|e| e.to_str()).unwrap_or("");
        match classify_ext(ext) {
            None => out.push(ScannedFile {
                path: entry.path().to_path_buf(),
                size,
                kind: FileKind::Skipped(SkipReason::PrecompressedExt),
            }),
            Some(cls) => out.push(ScannedFile {
                path: entry.path().to_path_buf(),
                size,
                kind: FileKind::Candidate(cls),
            }),
        }
        progress.scan_tick();
    }
    progress.scan_done(out.len() as u64);
    Ok(out)
}

/// Build a gitignore-style matcher for `--exclude` patterns. Empty input
/// yields a matcher that matches nothing.
fn build_exclude_matcher(patterns: &[String])
    -> GResult<Option<ignore::gitignore::Gitignore>>
{
    if patterns.is_empty() { return Ok(None); }
    let mut b = ignore::gitignore::GitignoreBuilder::new("");
    for p in patterns {
        b.add_line(None, p).map_err(|e| crate::error::GError::Other(
            format!("invalid exclude pattern \"{p}\": {e}")))?;
    }
    Ok(Some(b.build().map_err(|e| crate::error::GError::Other(
        format!("exclude build: {e}")))?))
}

fn exclude_matcher_matches(m: &mut Option<ignore::gitignore::Gitignore>, rel: &str) -> bool {
    match m {
        Some(g) => matches!(
            g.matched(Path::new(rel), false),
            ignore::Match::Ignore(_)
        ),
        None => false,
    }
}

/// Aggregate a scanned list into an [`Estimate`] using `algo`'s ratios.
pub fn summarize(files: &[ScannedFile], algo: CompactAlgorithm) -> Estimate {
    let mut total_files = 0u64;
    let mut candidate_files = 0u64;
    let mut skipped_files = 0u64;
    let mut total_size = 0u64;
    let mut candidate_size = 0u64;
    let mut skipped_size = 0u64;
    let mut estimated_after = 0u64;
    let mut skipped_map: HashMap<SkipReason, (u64, u64)> = HashMap::new();
    let mut candidate_map: HashMap<FileClass, (u64, u64)> = HashMap::new();

    for f in files {
        total_files += 1;
        total_size += f.size;
        match f.kind {
            FileKind::Candidate(cls) => {
                candidate_files += 1;
                candidate_size += f.size;
                let post = (f.size as f64 * cls.ratio(algo)).round() as u64;
                estimated_after += post;
                let e = candidate_map.entry(cls).or_insert((0, 0));
                e.0 += 1; e.1 += f.size;
            }
            FileKind::Skipped(reason) => {
                skipped_files += 1;
                skipped_size += f.size;
                // Skipped files keep their current size.
                estimated_after += f.size;
                let e = skipped_map.entry(reason).or_insert((0, 0));
                e.0 += 1; e.1 += f.size;
            }
        }
    }

    let estimated_savings = total_size.saturating_sub(estimated_after);

    let mut skipped_breakdown: Vec<(SkipReason, u64, u64)> =
        skipped_map.into_iter().map(|(r, (c, s))| (r, c, s)).collect();
    skipped_breakdown.sort_by_key(|(_, c, _)| std::cmp::Reverse(*c));

    let mut candidate_breakdown: Vec<(FileClass, u64, u64)> =
        candidate_map.into_iter().map(|(cls, (c, s))| (cls, c, s)).collect();
    candidate_breakdown.sort_by_key(|(_, _, s)| std::cmp::Reverse(*s));

    Estimate {
        total_files, candidate_files, skipped_files,
        total_size, candidate_size, skipped_size,
        estimated_after, estimated_savings,
        skipped_breakdown, candidate_breakdown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_precompressed() {
        assert_eq!(classify_ext("mp4"), None);
        assert_eq!(classify_ext("ZIP"), None);
        assert_eq!(classify_ext("pak"), None);
    }

    #[test]
    fn classify_executable_and_text() {
        assert_eq!(classify_ext("exe"), Some(FileClass::Executable));
        assert_eq!(classify_ext("DLL"), Some(FileClass::Executable));
        assert_eq!(classify_ext("json"), Some(FileClass::Text));
        assert_eq!(classify_ext("Lua"), Some(FileClass::Text));
    }

    #[test]
    fn classify_other() {
        assert_eq!(classify_ext("dat"), Some(FileClass::Other));
        assert_eq!(classify_ext(""), Some(FileClass::Other));
    }

    #[test]
    fn lzx_ratio_text_best() {
        // Text should compress better than executable under LZX.
        assert!(FileClass::Text.ratio(CompactAlgorithm::Lzx)
                < FileClass::Executable.ratio(CompactAlgorithm::Lzx));
    }

    #[test]
    fn none_ratio_is_identity() {
        for cls in [FileClass::Executable, FileClass::Text,
                    FileClass::UncompressedMedia, FileClass::Other] {
            assert!((cls.ratio(CompactAlgorithm::None) - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn summarize_mixed() {
        let files = vec![
            ScannedFile { path: PathBuf::from("a.exe"), size: 1000,
                kind: FileKind::Candidate(FileClass::Executable) },
            ScannedFile { path: PathBuf::from("b.txt"), size: 1000,
                kind: FileKind::Candidate(FileClass::Text) },
            ScannedFile { path: PathBuf::from("c.mp4"), size: 5000,
                kind: FileKind::Skipped(SkipReason::PrecompressedExt) },
            ScannedFile { path: PathBuf::from("d.bin"), size: 100,
                kind: FileKind::Skipped(SkipReason::TooSmall) },
        ];
        let est = summarize(&files, CompactAlgorithm::Lzx);
        assert_eq!(est.total_files, 4);
        assert_eq!(est.candidate_files, 2);
        assert_eq!(est.skipped_files, 2);
        assert_eq!(est.total_size, 7100);
        assert_eq!(est.candidate_size, 2000);
        assert_eq!(est.skipped_size, 5100);
        // Candidates: 1000*0.55 + 1000*0.30 = 850, + skipped 5100 = 5950.
        assert_eq!(est.estimated_after, 5950);
        assert_eq!(est.estimated_savings, 7100 - 5950);
    }

    #[test]
    fn low_savings_flag() {
        // All-skipped → 0% savings.
        let files = vec![
            ScannedFile { path: PathBuf::from("a.mp4"), size: 10000,
                kind: FileKind::Skipped(SkipReason::PrecompressedExt) },
        ];
        let est = summarize(&files, CompactAlgorithm::Lzx);
        assert!(est.low_savings());
    }

    #[test]
    fn empty_folder_zero_estimate() {
        let est = summarize(&[], CompactAlgorithm::Lzx);
        assert_eq!(est.total_files, 0);
        assert_eq!(est.total_size, 0);
        assert_eq!(est.estimated_savings, 0);
        assert!(est.low_savings()); // 0% < 5%
    }
}
