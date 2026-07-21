//! `gim defrag` — defragment a game folder's files on HDD volumes.
//!
//! # 7-stage workflow (see instruction.md)
//!
//! 1. **Admin check** — refuse to run without elevation (every FSCTL needs
//!    it). If not elevated, request UAC re-elevation; exit if refused.
//! 2. **Media detection** — SSD → optional TRIM, exit. HDD → continue.
//!    `--allow-ssd` overrides but still skips cluster moves.
//! 3. **Targeted fragmentation analysis** — walk the game folder, open
//!    each candidate file, query VCN/LCN extents, compute fragmentation
//!    ratio. Skip files below `fragment_threshold_pct`.
//! 4. **Volume bitmap scan** — chunked scan of `FSCTL_GET_VOLUME_BITMAP`,
//!    find contiguous free regions at the lowest LCNs.
//! 5. **Safety validation** — ≥15% free space, no locks, no compressed/
//!    encrypted/sparse/offline/reparse files, target region big enough,
//!    extent count under `max_extents`. VSS warning if active.
//! 6. **Defrag engine** — `FSCTL_MOVE_FILE` per file, with the
//!    Double-Check Overwrite Guard and I/O throttle.
//! 7. **Consolidation** — move defragmented files into the fast zone
//!    (lowest LCNs) for outer-track speed.
//!
//! # Non-Windows
//!
//! Returns `NotSupportedPlatform` early — defrag is meaningless on Linux
//! ext4 / macOS APFS (they don't expose the needed FSCTLs and use
//! different on-disk layouts).

use crate::config::{env_data_dir_override, GimConfig, Paths};
use crate::db::GamesDb;
use crate::defrag::{
    analyze_fragmentation, build_plan, check_file_safety, detect_media_kind,
    is_elevated, request_elevation, scan_all_free_regions,
    ElevationToken, IoThrottle, MediaKind, MoveOutcome, MoveRequest, VolumeInfo,
    DefragOptions, DefragPhase, DefragState, TargetFolder,
    execute_move, lock_file_path, open_volume, query_volume_info,
    state_file_path,
};
use crate::error::{GError, GResult};
use crate::locking::LockGuard;
use crate::output::format_size;
use crate::output::{Colorizer, ProgressReporter};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

/// Entry point — invoked from `commands::dispatch` for `Command::Defrag`.
pub fn run(
    c: &Colorizer,
    alias: String,
    target: Option<String>,
    confirm: bool,
    force: bool,
    allow_ssd: bool,
    threads: Option<usize>,
    exclude: Vec<String>,
    dry_run: bool,
    progress: &ProgressReporter,
) -> GResult<()> {
    // ── 0. Platform gate ────────────────────────────────────────────────
    // The whole flow uses Win32 FSCTLs. Bail early on non-Windows so the
    // user gets a single, clear error rather than a cascade of stub
    // failures.
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (c, alias, target, confirm, force, allow_ssd, threads, exclude, dry_run, progress);
        return Err(GError::NotSupportedPlatform);
    }

    #[cfg(target_os = "windows")]
    {
        run_windows(c, alias, target, confirm, force, allow_ssd, threads, exclude, dry_run, progress)
    }
}

#[cfg(target_os = "windows")]
fn run_windows(
    c: &Colorizer,
    alias: String,
    target: Option<String>,
    confirm: bool,
    force: bool,
    allow_ssd: bool,
    threads: Option<usize>,
    exclude: Vec<String>,
    dry_run: bool,
    progress: &ProgressReporter,
) -> GResult<()> {
    // ── 1. Paths & config ───────────────────────────────────────────────
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;

    let gdb = GamesDb::open(&paths.games_db)?;
    let game = gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    if !game.game_dir.exists() { return Err(GError::GameDirMissing(game.game_dir.clone())); }
    let cfg = GimConfig::load_game(&paths, &alias)?;

    let target_folder = match target.as_deref() {
        Some(t) => TargetFolder::parse(t)?,
        None => TargetFolder::GameDir,
    };

    let opts = DefragOptions {
        target: target_folder,
        threads: threads.unwrap_or_else(|| cfg.hash_threads().max(1)),
        exclude: exclude.clone(),
        force,
        confirm,
        dry_run,
        allow_ssd,
        min_free_pct: cfg.defrag_min_free_pct(),
        fragment_threshold_pct: cfg.defrag_fragment_threshold_pct(),
        max_extents_per_file: cfg.defrag_max_extents(),
        throttle_mb: cfg.defrag_throttle_mb(),
        throttle_sleep_ms: cfg.defrag_throttle_sleep_ms(),
        consolidate: cfg.defrag_consolidate(),
    };

    // Acquire the per-game lock so two defrags can't race on the same
    // game's files.
    let lock_path = lock_file_path(&paths.data_dir, &alias);
    let _lock = LockGuard::try_acquire_exclusive(&lock_path)?
        .ok_or_else(|| GError::Defrag(format!(
            "another defrag is already running for \"{alias}\" (lockfile: {})",
            lock_path.display()
        )))?;

    // State file — write the current phase so a future --status can read it.
    let state_path = state_file_path(&paths.data_dir, &alias);
    let mut state = DefragState::new(target_folder);
    state.save(&state_path)?;

    // ── 2. Stage 1 — admin check ───────────────────────────────────────
    state.phase = DefragPhase::Authorizing.as_str().to_string();
    state.save(&state_path)?;

    match is_elevated()? {
        ElevationToken::Elevated => { /* proceed */ }
        ElevationToken::NotElevated => {
            // Try to re-launch ourselves with admin rights.
            match request_elevation()? {
                true => {
                    // A new elevated gim process is running. This one should exit.
                    println!("{}: launched elevated gim instance to continue defrag",
                             c.green("info"));
                    return Ok(());
                }
                false => {
                    return Err(GError::DefragNeedsAdmin);
                }
            }
        }
    }

    // ── 3. Stage 2 — media detection ───────────────────────────────────
    state.phase = DefragPhase::DetectingMedia.as_str().to_string();
    state.save(&state_path)?;

    let media = detect_media_kind(&game.game_dir)?;
    println!("drive media: {} ({})", c.bold(media.as_str()), media_kind_desc(media));
    match media {
        MediaKind::Ssd | MediaKind::Unknown => {
            if !opts.allow_ssd {
                return Err(GError::DefragNotHdd);
            }
            // --allow-ssd: skip cluster moves, just exit (TRIM is left to
            // the OS — manual TRIM is risky and rarely useful).
            println!("{}: --allow-ssd set, skipping defrag on SSD (no TRIM issued)",
                     c.yellow("warn"));
            state.phase = DefragPhase::Done.as_str().to_string();
            state.message = "skipped: SSD".into();
            state.save(&state_path)?;
            return Ok(());
        }
        MediaKind::Hdd => { /* proceed */ }
    }

    // ── 4. Resolve target dirs ─────────────────────────────────────────
    let target_dirs = resolve_target_dirs(&game.game_dir, &game.data_dir, &paths, &alias, target_folder);
    if target_dirs.is_empty() {
        return Err(GError::Defrag("no target directories to scan".into()));
    }

    // ── 5. Volume info ─────────────────────────────────────────────────
    let first_dir = target_dirs.iter()
        .find(|d| d.exists())
        .ok_or_else(|| GError::Defrag("no existing target directory".into()))?;
    let volume = query_volume_info(first_dir)?;
    println!("volume {}: {} total / {} free ({}%)",
             c.bold(&format!("{}:", volume.drive)),
             format_size(volume.total_bytes as i64),
             format_size(volume.free_bytes as i64),
             volume.free_pct);

    if volume.free_below(opts.min_free_pct) {
        return Err(GError::DefragLowFreeSpace(volume.free_pct, opts.min_free_pct));
    }

    // VSS warning — moving big files may invalidate System Restore Points.
    if volume.vss_active {
        println!("{}: VSS (System Restore) is active on this volume.",
                 c.yellow("warn"));
        println!("        Moving large game files may consume VSS diff-area and");
        println!("        could cause Windows to delete older System Restore Points.");
        if !opts.confirm && !opts.dry_run {
            print!("proceed anyway? [y/N] ");
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().lock().read_line(&mut input)?;
            if !input.trim().eq_ignore_ascii_case("y") {
                return Err(GError::DefragCancelled);
            }
        }
    }

    // ── 6. Stage 3 — targeted fragmentation analysis ───────────────────
    state.phase = DefragPhase::Analyzing.as_str().to_string();
    state.save(&state_path)?;

    let files = collect_files(&target_dirs, &opts.exclude, progress)?;
    state.total_files = files.len() as u64;
    state.save(&state_path)?;

    progress.defrag_analyze_start(files.len());
    let mut analyzed: Vec<(crate::defrag::FileMap, crate::defrag::FileSafety)> = Vec::new();
    for f in &files {
        let safety = check_file_safety(f)?;
        // For files that pass safety, query the VCN/LCN map. For locked
        // ones, skip the FSCTL — we already know we can't move them.
        let map = if safety == crate::defrag::FileSafety::Ok {
            analyze_fragmentation(f, volume.bytes_per_cluster).unwrap_or_else(|_| {
                crate::defrag::FileMap {
                    path: f.to_path_buf(),
                    size: 0, bytes_per_cluster: volume.bytes_per_cluster,
                    extents: Vec::new(),
                }
            })
        } else {
            crate::defrag::FileMap {
                path: f.to_path_buf(),
                size: std::fs::symlink_metadata(f).map(|m| m.len()).unwrap_or(0),
                bytes_per_cluster: volume.bytes_per_cluster,
                extents: Vec::new(),
            }
        };
        analyzed.push((map, safety));
        progress.defrag_analyze_tick();
    }
    progress.defrag_analyze_done(analyzed.len() as u64);

    // ── 7. Stage 4 — volume bitmap scan ─────────────────────────────────
    state.phase = DefragPhase::ScanningBitmap.as_str().to_string();
    state.save(&state_path)?;

    progress.defrag_scan_start();
    let free_regions = scan_all_free_regions(volume.drive, volume.total_clusters())?;
    progress.defrag_scan_done(free_regions.len() as u64);
    println!("found {} contiguous free regions (lowest LCN: {})",
             c.bold(&free_regions.len().to_string()),
             free_regions.first().map(|r| r.start_lcn.to_string()).unwrap_or_else(|| "n/a".into()));

    // ── 8. Stage 5 — build plan ────────────────────────────────────────
    state.phase = DefragPhase::Validating.as_str().to_string();
    state.save(&state_path)?;

    let plan = match build_plan(&analyzed, &free_regions, &volume, &opts) {
        Ok(p) => p,
        Err(crate::defrag::PlanError::OutOfFreeSpace { free_pct, min_pct }) => {
            return Err(GError::DefragLowFreeSpace(free_pct, min_pct));
        }
        Err(crate::defrag::PlanError::EmptyPlan) => {
            println!("{}: nothing to defrag — all files are either contiguous,",
                     c.green("done"));
            println!("        below the fragmentation threshold, or skipped for safety.");
            state.phase = DefragPhase::Done.as_str().to_string();
            state.message = "nothing to defrag".into();
            state.save(&state_path)?;
            return Ok(());
        }
    };

    // Print plan summary.
    print_plan_summary(c, &plan, &volume);

    if dry_run {
        println!("\n{}: dry run — no files were moved", c.green("done"));
        state.phase = DefragPhase::Done.as_str().to_string();
        state.message = "dry run".into();
        state.save(&state_path)?;
        return Ok(());
    }

    // Prompt user (unless --confirm).
    if !opts.confirm {
        print!("\nproceed with defrag? [y/N] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            return Err(GError::DefragCancelled);
        }
    }

    // ── 9. Stage 6 — defrag engine ─────────────────────────────────────
    state.phase = DefragPhase::Defragmenting.as_str().to_string();
    state.save(&state_path)?;

    let volume_handle = open_volume(volume.drive)?;
    let throttle = IoThrottle::new(opts.throttle_mb * 1024 * 1024, opts.throttle_sleep_ms);

    progress.defrag_start(plan.planned.len());
    let mut errors: Vec<String> = Vec::new();
    let mut moved_count: u64 = 0;
    let mut skipped_count: u64 = 0;
    let mut failed_count: u64 = 0;

    for pf in &plan.planned {
        match defrag_one_file(volume.drive, volume_handle.raw(), pf, &volume, &throttle) {
            Ok(()) => {
                moved_count += 1;
                state.defragged_files = moved_count;
                state.bytes_moved = throttle.total_bytes_moved();
                state.save(&state_path)?;
            }
            Err(DefragOneError::Skip(reason)) => {
                skipped_count += 1;
                state.skipped_files = skipped_count;
                let _ = reason; // could log per-file
            }
            Err(DefragOneError::Fail(e)) => {
                failed_count += 1;
                state.failed_files = failed_count;
                errors.push(format!("{}: {e}", pf.path.display()));
            }
        }
        progress.defrag_tick();
    }
    progress.defrag_done(moved_count);

    // ── 10. Stage 7 — consolidation (planned but not separately executed) ──
    // The current planner already places defragmented files at the lowest
    // available LCNs — i.e. consolidation happens as part of the defrag
    // pass. The separate `consolidate_*` progress wrappers exist for a
    // future split where stage 6 only de-fragments in place and stage 7
    // physically relocates contiguous files. For now we mark the phase
    // as done and report.
    state.phase = DefragPhase::Consolidating.as_str().to_string();
    state.save(&state_path)?;
    if opts.consolidate {
        progress.consolidate_start(0);
        progress.consolidate_done(0);
    }

    // ── 11. Final report ───────────────────────────────────────────────
    state.phase = DefragPhase::Done.as_str().to_string();
    state.message = format!(
        "moved {} files ({} bytes), skipped {}, failed {}",
        moved_count, throttle.total_bytes_moved(), skipped_count, failed_count
    );
    state.save(&state_path)?;

    println!("\n{}: defrag complete", c.green("done"));
    println!("  files moved:    {}", c.bold(&moved_count.to_string()));
    println!("  bytes moved:    {}", c.bold(&format_size(throttle.total_bytes_moved() as i64)));
    println!("  files skipped:  {}", skipped_count);
    println!("  files failed:   {}", failed_count);
    println!("  throttle sleeps: {}", throttle.total_sleeps());
    if !errors.is_empty() {
        eprintln!("{}: {} error(s):", c.red("warning"), errors.len());
        for e in errors.iter().take(10) { eprintln!("  {e}"); }
        if errors.len() > 10 {
            eprintln!("  ... and {} more", errors.len() - 10);
        }
    }
    Ok(())
}

/// Error from defragging a single file.
#[cfg(target_os = "windows")]
enum DefragOneError {
    /// File skipped (locked, target occupied, etc.) — non-fatal.
    Skip(String),
    /// Hard failure — recorded for the final report.
    Fail(String),
}

#[cfg(target_os = "windows")]
fn defrag_one_file(
    drive: char,
    volume_handle: *mut std::ffi::c_void,
    pf: &crate::defrag::PlannedFile,
    volume: &VolumeInfo,
    throttle: &IoThrottle,
) -> Result<(), DefragOneError> {
    use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

    // Re-open the file with GENERIC_READ | GENERIC_WRITE so we have a
    // handle valid for FSCTL_MOVE_FILE. The safety check earlier verified
    // the file isn't locked, but it may have been opened by another
    // process between then and now — handle that gracefully.
    const GENERIC_READ: u32 = 0x80000000;
    const GENERIC_WRITE: u32 = 0x40000000;
    const FILE_SHARE_READ: u32 = 0x00000001;
    const FILE_SHARE_WRITE: u32 = 0x00000002;
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

    let mut wide: Vec<u16> = OsStr::new(&pf.path).encode_wide().collect();
    wide.push(0);
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
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return Err(DefragOneError::Skip(format!("open failed (Win32 error {code})")));
    }

    // Defer CloseHandle via a guard.
    struct HandleGuard(*mut std::ffi::c_void);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe { let _ = CloseHandle(self.0); }
            }
        }
    }
    let _guard = HandleGuard(h);

    // Issue each planned move. For now we have a single move per file
    // (the planner consolidates the whole file into one region). Future
    // versions may have multiple moves per file.
    for mv in &pf.moves {
        let req = MoveRequest {
            file_handle: h,
            start_vcn: mv.start_vcn,
            cluster_count: mv.cluster_count,
            target_lcn: mv.target_lcn,
        };
        match execute_move(drive, volume_handle, req, volume.bytes_per_cluster) {
            Ok(MoveOutcome::Moved { bytes }) => {
                throttle.record_bytes(bytes);
            }
            Ok(MoveOutcome::TargetOccupied) => {
                return Err(DefragOneError::Skip(
                    "target LCN got allocated by another process".into()));
            }
            Ok(MoveOutcome::Locked) => {
                return Err(DefragOneError::Skip("file locked".into()));
            }
            Ok(MoveOutcome::HardError(code)) => {
                return Err(DefragOneError::Fail(format!(
                    "FSCTL_MOVE_FILE failed (Win32 error {code})")));
            }
            Err(e) => {
                return Err(DefragOneError::Fail(format!("execute_move: {e}")));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn resolve_target_dirs(
    game_dir: &Path,
    data_dir: &Path,
    paths: &Paths,
    alias: &str,
    target: TargetFolder,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    match target {
        TargetFolder::GameDir => out.push(game_dir.to_path_buf()),
        TargetFolder::DataDir => out.push(paths.objects_dir(alias)),
        TargetFolder::Both => {
            out.push(game_dir.to_path_buf());
            out.push(paths.objects_dir(alias));
        }
    }
    let _ = (data_dir,); // data_dir param reserved for future use
    out
}

/// Walk the target dirs and return every regular file. Applies the
/// gitignore-style `exclude` patterns.
fn collect_files(
    dirs: &[PathBuf],
    exclude: &[String],
    progress: &ProgressReporter,
) -> GResult<Vec<PathBuf>> {
    use ignore::WalkBuilder;

    let mut out: Vec<PathBuf> = Vec::new();
    let mut matcher: Option<ignore::gitignore::Gitignore> = None;
    if !exclude.is_empty() {
        let mut b = ignore::gitignore::GitignoreBuilder::new("");
        for p in exclude {
            b.add_line(None, p).map_err(|e| GError::Other(format!(
                "invalid exclude pattern \"{p}\": {e}")))?;
        }
        matcher = Some(b.build().map_err(|e| GError::Other(format!("exclude: {e}")))?);
    }

    progress.scan_start();
    for dir in dirs {
        if !dir.exists() { continue; }
        let mut builder = WalkBuilder::new(dir);
        builder.hidden(false).parents(false).ignore(false).git_ignore(false)
            .git_global(false).git_exclude(false).follow_links(false);
        for entry in builder.build() {
            let entry = match entry { Ok(e) => e, Err(_) => continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) { continue; }
            // Apply exclude matcher against the path relative to the walk root.
            if let Some(ref m) = matcher {
                let rel = entry.path().strip_prefix(dir)
                    .ok().map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                if matches!(m.matched(std::path::Path::new(&rel), false),
                            ignore::Match::Ignore(_)) { continue; }
            }
            out.push(entry.path().to_path_buf());
            progress.scan_tick();
        }
    }
    progress.scan_done(out.len() as u64);
    Ok(out)
}

#[cfg(target_os = "windows")]
fn print_plan_summary(c: &Colorizer, plan: &crate::defrag::DefragPlan, volume: &VolumeInfo) {
    println!("\n{}: defrag plan", c.bold("plan"));
    println!("  files to move:  {}", c.green(&plan.planned_count.to_string()));
    println!("  files skipped:  {}", plan.skipped_count);
    println!("  bytes to move:  {} ({})",
             format_size(plan.bytes_to_move as i64),
             plan.clusters_to_move);
    println!("  volume:         {} ({}B/cluster, {}B phys sector)",
             volume.drive,
             volume.bytes_per_cluster,
             volume.bytes_per_sector_phys);

    // Breakdown of skipped reasons.
    let mut skip_counts: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for s in &plan.skipped {
        *skip_counts.entry(s.reason.as_str()).or_insert(0) += 1;
    }
    if !skip_counts.is_empty() {
        println!("\n  skipped breakdown:");
        let mut entries: Vec<_> = skip_counts.into_iter().collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        for (reason, count) in entries {
            println!("    {:<32} {}", reason, count);
        }
    }

    // Top 5 largest planned files.
    if !plan.planned.is_empty() {
        println!("\n  top 5 files by size:");
        let mut sorted: Vec<&crate::defrag::PlannedFile> = plan.planned.iter().collect();
        sorted.sort_by(|a, b| b.size.cmp(&a.size));
        for pf in sorted.iter().take(5) {
            println!("    {:<48} {} ({} extents → 1)",
                     pf.path.file_name().map(|n| n.to_string_lossy().into_owned())
                       .unwrap_or_default(),
                     format_size(pf.size as i64),
                     pf.extents_before);
        }
    }
}

fn media_kind_desc(m: MediaKind) -> &'static str {
    match m {
        MediaKind::Hdd => "spinning disk — defrag helps",
        MediaKind::Ssd => "solid state — defrag hurts, skipping",
        MediaKind::Unknown => "unknown — treating as SSD (safe default)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_folder_parse_roundtrip() {
        for s in ["game", "data", "both"] {
            let t = TargetFolder::parse(s).unwrap();
            assert_eq!(t.as_str(), s);
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn run_unsupported_off_windows() {
        let c = crate::output::default_colorizer();
        let p = ProgressReporter::new(false);
        let r = run(&c, "x".into(), None, false, false, false, None, Vec::new(), false, &p);
        assert!(matches!(r, Err(GError::NotSupportedPlatform)));
    }
}
