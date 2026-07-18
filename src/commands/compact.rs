//! `gim compact` — compress game or snapshot data folders via Windows API.
//!
//! Flow:
//! 1. Resolve config & CLI options.
//! 2. Validate game exists; if `--status`, print background state and exit.
//! 3. Scan target folder → estimate savings.
//! 4. Print summary (colorized table).
//! 5. WOF availability check (if using a WOF algorithm).
//! 6. Prompt yes/no (unless `--confirm` or `--dry-run`).
//! 7. Execute (foreground with progress bar, or `--background` worker).
//! 8. Print final results.

use crate::compact::{
    check_running_tracked, compress_file, decompress_file,
    scan, summarize, CompactAlgorithm, CompactMode, CompactOptions, CompactPhase, CompactState,
    Estimate, FileKind, TargetFolder, WofDriverStatus, WofRuntimeProbe,
    lock_file_path, state_file_path,
    enable_wof_driver, probe_wof_driver, probe_wof_runtime,
    reset_wof_availability,
};
use crate::config::{env_data_dir_override, GimConfig, Paths};
use crate::db::GamesDb;
use crate::error::{GError, GResult};
use crate::locking::LockGuard;
use crate::output::{Colorizer, ProgressReporter};
use crate::output::format_size;
use rayon::prelude::*;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub fn run(
    c: &Colorizer,
    alias: String,
    algorithm: Option<String>,
    target: Option<String>,
    decompress: bool,
    confirm: bool,
    force: bool,
    threads: Option<usize>,
    exclude: Vec<String>,
    background: bool,
    status: bool,
    dry_run: bool,
    progress: &ProgressReporter,
) -> GResult<()> {
    // ── 1. Resolve paths & config ────────────────────────────────────
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;

    let gdb = GamesDb::open(&paths.games_db)?;
    let game = gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let cfg = GimConfig::load_game(&paths, &alias)?;

    // ── 2. Resolve options ────────────────────────────────────────────
    let algo = if decompress {
        CompactAlgorithm::None
    } else if let Some(ref a) = algorithm {
        a.parse()?
    } else {
        cfg.compact_algorithm()?
    };

    let target_folder = match target.as_deref() {
        Some(t) => TargetFolder::parse(t)?,
        None => TargetFolder::GameDir, // default
    };

    let compact_threads = threads.unwrap_or_else(|| cfg.compact_threads());
    let auto_pause = cfg.compact_auto_pause();

    let opts = CompactOptions {
        algorithm: algo,
        target: target_folder,
        threads: compact_threads,
        exclude: exclude.clone(),
        force,
        confirm,
        background,
        dry_run,
    };

    // ── 3. --status: print background state ──────────────────────────
    if status {
        return print_status(c, &paths, &alias);
    }

    // ── 4. Resolve target dirs ────────────────────────────────────────
    let target_dirs = resolve_target_dirs(&game.game_dir, &game.data_dir, &paths, &alias, target_folder);

    // ── 5. Scan & estimate (for each target dir) ────────────────────
    let mut all_files: Vec<crate::compact::ScannedFile> = Vec::new();
    for dir in &target_dirs {
        if !dir.exists() {
            eprintln!("warning: target directory does not exist: {}", dir.display());
            continue;
        }
        let files = scan(dir, &opts.exclude, opts.algorithm.is_decompress(), progress)?;
        all_files.extend(files);
    }
    let estimate = summarize(&all_files, opts.algorithm);

    // ── 6. Print summary ─────────────────────────────────────────────
    print_estimate(c, &estimate, &opts, &game.game_dir, &alias);

    // ── 7. WOF availability check (pre-flight) ─────────────────────────
    // Two-stage check:
    //   a) Static probe: is wof.sys installed + enabled? (registry check)
    //   b) Runtime probe: try WOF IOCTL on a temp file in the target directory.
    //      This catches "WOF installed but not attached to this volume".
    //
    // The runtime probe is definitive — if it fails, WOF won't work on
    // this volume regardless of what the static probe says.
    if !opts.algorithm.is_decompress() && opts.algorithm.mode() == CompactMode::Wof {
        // Pastikan cache WOF state bersih sebelum probe definitif
        reset_wof_availability();

        // (a) Static probe — fast, no disk I/O.
        let static_status = probe_wof_driver();
        match static_status {
            WofDriverStatus::NotInstalled => {
                println!();
                println!("  {} WOF (Windows Overlay Filter) is not available on this system.",
                         c.red("✗"));
                println!("  The driver file (wof.sys) was not found.");
                println!("  WOF compression ({}) cannot be used without this driver.",
                         opts.algorithm.label());
                return Err(GError::WofNotAvailable(
                    "wof.sys driver not found — WOF compression is not available".into(),
                ));
            }
            WofDriverStatus::Disabled => {
                println!();
                println!("  {} WOF (Windows Overlay Filter) is disabled on this system.",
                         c.yellow("⚠"));
                println!("  The driver file exists but the WOF service is not active.");
                println!();
                print!("  Enable the WOF driver? (requires Administrator + restart) [y/N] ");
                io::stdout().flush()?;
                let mut enable_input = String::new();
                io::stdin().lock().read_line(&mut enable_input)?;
                if enable_input.trim().eq_ignore_ascii_case("y") {
                    match enable_wof_driver() {
                        Ok(()) => {
                            println!();
                            println!("  {} WOF driver enabled in the registry.",
                                     c.green("✓"));
                            println!("  {} Restart your computer, then run `gim compact {}` again.",
                                     c.yellow("important:"), alias);
                            return Ok(());
                        }
                        Err(e) => {
                            println!();
                            println!("  {} Failed to enable WOF driver: {}", c.red("error:"), e);
                            return Err(e);
                        }
                    }
                } else {
                    return Err(GError::CompactCancelled);
                }
            }
            WofDriverStatus::Available => {
                // Static probe passed — now do the runtime probe.
            }
        }

        // (b) Runtime probe — definitive volume-level test.
        // We probe the first target directory that exists.
        let probe_dir = target_dirs.iter().find(|d| d.exists());
        if let Some(dir) = probe_dir {
            match probe_wof_runtime(dir) {
                WofRuntimeProbe::Ok => {
                    // WOF works on this volume — proceed.
                }
                WofRuntimeProbe::NotAttachedToVolume(code) => {
                    println!();
                    println!("  {} WOF (Windows Overlay Filter) is not available on this volume.",
                             c.red("✗"));
                    println!("  Probe failed with Win32 error: {} (0x{:08X})", code, code);
                    println!("  The WOF driver is installed on this system but is not attached to the");
                    println!("  volume hosting the target directory:");
                    println!("    {}", c.dim(&dir.to_string_lossy()));
                    println!();
                    println!("  WOF compression ({}) requires the WOF filter to be attached to the volume.",
                             opts.algorithm.label());
                    println!();
                    println!("  Options:");
                    println!("    • Use {} for NTFS compression (works on any NTFS volume).",
                             c.dim("--algorithm ntfs"));
                    println!("    • Move the game to a WOF-enabled volume (e.g. the system drive).");
                    println!();
                    return Err(GError::WofNotAvailable(
                        format!("WOF is not attached to volume {} (error {code})", dir.to_string_lossy()),
                    ));
                }
                WofRuntimeProbe::ProbeFailed(msg) => {
                    eprintln!("  {} WOF runtime probe failed: {}", c.yellow("warning:"), msg);
                    eprintln!("  Proceeding — compaction may fail per-file.");
                }
            }
        }
    }

    // ── 8. Prompt confirmation ───────────────────────────────────────
    if !opts.confirm && !opts.dry_run {
        if estimate.candidate_files == 0 {
            println!();
            println!("nothing to compact — {} already compressed or too small",
                     c.dim(&format!("{} files", estimate.skipped_files)));
            return Ok(());
        }
        if estimate.low_savings() && !opts.force {
            println!();
            println!("{} estimated savings ({:.1}%) — this folder is mostly precompressed assets.",
                     c.yellow("low"), estimate.savings_pct());
            println!("use {} to proceed anyway.",
                     c.dim("--force"));
            return Ok(());
        }
        println!();
        print!("proceed? [y/N] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("cancelled");
            return Err(GError::CompactCancelled);
        }
    } else if opts.dry_run {
        println!();
        println!("dry run — no changes made");
        return Ok(());
    }

    // ── 9. Execute ───────────────────────────────────────────────────
    if opts.background {
        return spawn_background(c, &paths, &alias, opts.clone(),
                                 all_files, estimate, auto_pause);
    }

    // Foreground execution
    execute_foreground(c, progress, &opts, &all_files, &estimate)
}

/// Resolve which physical directories to compact.
fn resolve_target_dirs(
    game_dir: &Path,
    _game_data_dir: &Path,
    paths: &Paths,
    alias: &str,
    target: TargetFolder,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match target {
        TargetFolder::GameDir => dirs.push(game_dir.to_path_buf()),
        TargetFolder::DataDir => dirs.push(paths.objects_dir(alias)),
        TargetFolder::Both => {
            dirs.push(game_dir.to_path_buf());
            dirs.push(paths.objects_dir(alias));
        }
    }
    dirs
}

/// Print the estimation summary in a nice, git-like format.
fn print_estimate(
    c: &Colorizer,
    est: &Estimate,
    opts: &CompactOptions,
    game_dir: &Path,
    alias: &str,
) {
    let algo_label = opts.algorithm.label();
    let target_label = opts.target.as_str();

    println!();
    println!("compact {} {}", c.bold(&alias), c.dim(&format!("({target_label}, {algo_label})")));
    println!("  game directory: {}", c.dim(&game_dir.to_string_lossy()));
    println!();

    // File counts
    println!("  files: {}", est.total_files);
    println!("    candidates: {} ({})",
             c.green(&est.candidate_files.to_string()),
             format_size(est.candidate_size as i64));
    println!("    skipped:    {} ({})",
             c.dim(&est.skipped_files.to_string()),
             format_size(est.skipped_size as i64));
    println!();

    // Size summary
    println!("  total size:     {}", format_size(est.total_size as i64));
    println!("  estimated after: {}", format_size(est.estimated_after as i64));
    println!("  estimated savings: {} ({:.1}%)",
             c.green(&format_size(est.estimated_savings as i64)),
             est.savings_pct());
    println!();

    // Skipped breakdown (top reasons)
    if !est.skipped_breakdown.is_empty() {
        println!("  skipped breakdown:");
        for (reason, count, size) in &est.skipped_breakdown {
            println!("    {:<20} {} files ({})",
                     c.dim(reason.label()), count, format_size(*size as i64));
        }
        println!();
    }

    // Candidate breakdown (top classes)
    if !est.candidate_breakdown.is_empty() {
        println!("  candidate breakdown:");
        for (cls, count, size) in &est.candidate_breakdown {
            println!("    {:<20} {} files ({})",
                     c.cyan(cls_label(*cls)), count, format_size(*size as i64));
        }
        println!();
    }

    if est.low_savings() {
        println!("  {} many game assets are already compressed — compaction may add overhead.",
                 c.yellow("⚠"));
    }
}

fn cls_label(cls: crate::compact::FileClass) -> &'static str {
    match cls {
        crate::compact::FileClass::Executable => "executable",
        crate::compact::FileClass::Text => "text",
        crate::compact::FileClass::UncompressedMedia => "uncompressed media",
        crate::compact::FileClass::Other => "other",
    }
}

/// Execute compaction in the foreground with a progress bar.
fn execute_foreground(
    c: &Colorizer,
    progress: &ProgressReporter,
    opts: &CompactOptions,
    files: &[crate::compact::ScannedFile],
    estimate: &Estimate,
) -> GResult<()> {
    let candidates: Vec<&crate::compact::ScannedFile> = files.iter()
        .filter(|f| matches!(f.kind, FileKind::Candidate(_)))
        .collect();

    let total = candidates.len();
    let is_decompress = opts.algorithm.is_decompress();

    if is_decompress {
        progress.decompress_start(total);
    } else {
        progress.compact_start(total);
    }

    let compressed = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));

    // Configure thread pool if specified.
    let result: Vec<GResult<()>> = if opts.threads > 0 {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(opts.threads)
            .build()
            .map_err(|e| GError::Compact(format!("thread pool: {e}")))?;
        pool.install(|| {
            candidates.par_iter().map(|f| {
                let r = if is_decompress {
                    decompress_file(&f.path)
                } else {
                    compress_file(&f.path, opts)
                };
                match &r {
                    Ok(()) => {
                        compressed.fetch_add(1, Ordering::Relaxed);
                        progress.compact_tick();
                    }
                    Err(_) => {
                        failed.fetch_add(1, Ordering::Relaxed);
                        progress.compact_tick();
                    }
                }
                r
            }).collect()
        })
    } else {
        candidates.par_iter().map(|f| {
            let r = if is_decompress {
                decompress_file(&f.path)
            } else {
                compress_file(&f.path, opts)
            };
            match &r {
                Ok(()) => {
                    compressed.fetch_add(1, Ordering::Relaxed);
                    progress.compact_tick();
                }
                Err(_) => {
                    failed.fetch_add(1, Ordering::Relaxed);
                    progress.compact_tick();
                }
            }
            r
        }).collect()
    };

    let compressed_n = compressed.load(Ordering::Relaxed);
    let failed_n = failed.load(Ordering::Relaxed);

    if is_decompress {
        progress.decompress_done(compressed_n);
    } else {
        progress.compact_done(compressed_n);
    }

    // Report first few errors.
    let errors: Vec<_> = result.iter().filter_map(|r| r.as_ref().err()).take(5).collect();
    for e in &errors {
        eprintln!("  {} {}", c.dim("error:"), e);
    }

    println!();
    let verb = if is_decompress { "decompressed" } else { "compacted" };
    if compressed_n > 0 {
        println!("{} {} {} files ({})",
                 c.green("✓"), c.bold(&verb), c.green(&compressed_n.to_string()),
                 format_size(estimate.candidate_size as i64));
        if failed_n > 0 {
            println!("  {} {} files failed (access denied, locked, or unsupported filesystem)",
                     c.yellow("!"), failed_n);
        }
    } else {
        println!("{} no files were {}",
                 c.red("✗"), verb);
        if failed_n > 0 {
            println!("  all {} candidate files failed — the compression method may not be", failed_n);
            println!("  available on this volume or filesystem. Check the errors above.");
        } else {
            println!("  no candidates to compress.");
        }
    }

    Ok(())
}

/// Spawn a background worker thread for compaction with auto-pause.
fn spawn_background(
    c: &Colorizer,
    paths: &Paths,
    alias: &str,
    opts: CompactOptions,
    files: Vec<crate::compact::ScannedFile>,
    estimate: Estimate,
    auto_pause: bool,
) -> GResult<()> {
    // Acquire lock to prevent duplicate workers.
    let lock_path = lock_file_path(&paths.data_dir, alias);
    match LockGuard::try_acquire_exclusive(&lock_path)? {
        Some(_) => {}
        None => {
            return Err(GError::CompactRunning(alias.to_string(), lock_path));
        }
    }

    let state_path = state_file_path(&paths.data_dir, alias);
    let data_dir = paths.data_dir.clone();
    let is_decompress = opts.algorithm.is_decompress();

    println!("starting background compaction...");
    println!("  algorithm: {}", c.dim(opts.algorithm.label()));
    println!("  target:    {}", c.dim(opts.target.as_str()));
    println!("  auto-pause: {}", c.dim(if auto_pause { "enabled" } else { "disabled" }));

    // Spawn worker thread. It runs detached — the thread owns the LockGuard
    // (dropped on completion, releasing the lock).
    std::thread::spawn(move || {
        let candidates: Vec<crate::compact::ScannedFile> = files.iter()
            .filter(|f| matches!(f.kind, FileKind::Candidate(_)))
            .cloned()
            .collect();
        let total = candidates.len() as u64;

        // Save initial state.
        let mut state = CompactState::new(opts.algorithm, opts.target);
        state.total_files = total;
        state.total_size = estimate.candidate_size;
        if let Err(e) = state.save(&state_path) {
            eprintln!("compact: failed to write state: {e}");
        }
        state.phase = CompactPhase::Running.as_str().to_string();
        if let Err(e) = state.save(&state_path) {
            eprintln!("compact: failed to write state: {e}");
        }

        let compressed = AtomicU64::new(0);
        let failed = AtomicU64::new(0);
        let pause_flag = Arc::new(AtomicBool::new(false));

        // Process files in batches with auto-pause checks.
        let batch_size = 64usize;
        let mut idx = 0usize;

        while idx < candidates.len() {
            // Auto-pause: check if any tracked game is running.
            if auto_pause {
                let gdb = match GamesDb::open(&data_dir.join("games.db")) {
                    Ok(db) => db,
                    Err(_) => {
                        std::thread::sleep(Duration::from_secs(1));
                        continue;
                    }
                };
                match check_running_tracked(&gdb) {
                    Ok(rs) if rs.any() && !pause_flag.load(Ordering::Relaxed) => {
                        pause_flag.store(true, Ordering::Relaxed);
                        state.phase = CompactPhase::Paused.as_str().to_string();
                        state.message = format!("paused: {}", rs.summary());
                        let _ = state.save(&state_path);
                    }
                    Ok(rs) if !rs.any() && pause_flag.load(Ordering::Relaxed) => {
                        pause_flag.store(false, Ordering::Relaxed);
                        state.phase = CompactPhase::Running.as_str().to_string();
                        state.message = String::new();
                        let _ = state.save(&state_path);
                    }
                    _ => {}
                }
                if pause_flag.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            }

            // Process one batch.
            let end = (idx + batch_size).min(candidates.len());
            for f in &candidates[idx..end] {
                let r = if is_decompress {
                    decompress_file(&f.path)
                } else {
                    compress_file(&f.path, &opts)
                };
                match r {
                    Ok(()) => { compressed.fetch_add(1, Ordering::Relaxed); }
                    Err(_) => { failed.fetch_add(1, Ordering::Relaxed); }
                }
            }
            idx = end;

            // Update state file.
            state.processed_files = idx as u64;
            state.compressed_files = compressed.load(Ordering::Relaxed);
            state.failed_files = failed.load(Ordering::Relaxed);
            state.updated_at = unix_now();
            let _ = state.save(&state_path);
        }

        // Done.
        state.processed_files = total;
        state.phase = CompactPhase::Done.as_str().to_string();
        state.message = format!("completed: {} compressed, {} failed",
                                compressed.load(Ordering::Relaxed),
                                failed.load(Ordering::Relaxed));
        let _ = state.save(&state_path);

        // Lock is released here when LockGuard is dropped.
    });

    println!();
    println!("  check status: {}", c.dim(&format!("gim compact {} --status", alias)));
    println!("  lockfile: {}", c.dim(&lock_path.to_string_lossy()));
    println!();
    println!("{} background compaction started", c.green("✓"));
    Ok(())
}

/// Print the status of a running background compaction.
fn print_status(c: &Colorizer, paths: &Paths, alias: &str) -> GResult<()> {
    // Validate game exists.
    let gdb = GamesDb::open(&paths.games_db)?;
    gdb.get(alias)?.ok_or_else(|| GError::AliasNotFound(alias.to_string()))?;

    let state_path = state_file_path(&paths.data_dir, alias);
    let lock_path = lock_file_path(&paths.data_dir, alias);

    let lock_exists = lock_path.exists();
    let state = CompactState::load(&state_path)?;

    if !lock_exists && state.is_none() {
        println!("no compaction running or recorded for {}", c.bold(alias));
        return Ok(());
    }

    match state {
        Some(s) => {
            let phase = s.phase();
            let phase_str = match phase {
                CompactPhase::Running => c.green("running"),
                CompactPhase::Paused => c.yellow("paused"),
                CompactPhase::Done => c.green("done"),
                CompactPhase::Failed => c.red("failed"),
                CompactPhase::Starting => c.cyan("starting"),
            };
            println!("compaction {} — {}", c.bold(alias), phase_str);
            println!("  algorithm: {}", s.algorithm);
            println!("  target:    {}", s.target);
            println!("  progress:   {}/{} files ({} failed)",
                     c.green(&s.processed_files.to_string()),
                     s.total_files,
                     s.failed_files);
            if s.total_size > 0 {
                let pct = (s.processed_files as f64 / s.total_files as f64 * 100.0).min(100.0);
                println!("  percentage: {:.0}%", pct);
            }
            if !s.message.is_empty() {
                println!("  message:    {}", c.dim(&s.message));
            }
            let elapsed = unix_now() - s.started_at;
            if elapsed > 0 {
                println!("  elapsed:    {}", c.dim(&format_duration(elapsed)));
            }

            if phase == CompactPhase::Done {
                println!();
                println!("  lockfile has been released. you can start a new compaction.");
            }
        }
        None => {
            if lock_exists {
                println!("compaction {} — lockfile exists but no state file found",
                         c.yellow(alias));
                println!("  a compaction may be starting or the state was lost");
            } else {
                println!("no compaction running for {}", c.bold(alias));
            }
        }
    }
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn format_duration(secs: i64) -> String {
    if secs < 60 { format!("{}s", secs) }
    else if secs < 3600 { format!("{}m {}s", secs / 60, secs % 60) }
    else { format!("{}h {}m", secs / 3600, (secs % 3600) / 60) }
}
