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
    worker: bool,
    lock_file: Option<String>,
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
    // Worker mode: this process WAS spawned by a `--background` parent to
    // do the actual compaction in a separate, long-lived process. It holds
    // the lock and writes `compact.state` for `--status` to read.
    if worker {
        let lock_path = lock_file
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| lock_file_path(&paths.data_dir, &alias));
        return run_worker(c, &paths, &alias, opts.clone(), auto_pause, lock_path);
    }

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

    // Report per-file errors WITH their paths (BUG 3 fix): previously only
    // the message was printed (no path) and only the first 5 were shown.
    // Win32 error 344 (ERROR_FILE_SYSTEM_LIMITATION) means the file simply
    // cannot be WOF-backed — we report those as skipped, not as failures.
    let mut skip_limit = 0u64;
    let mut real_errors: Vec<(String, String)> = Vec::new();
    for (f, r) in candidates.iter().zip(result.iter()) {
        if let Err(e) = r {
            let msg = e.to_string();
            if msg.contains("Win32 error 344") {
                skip_limit += 1;
            } else {
                real_errors.push((f.path.display().to_string(), msg));
            }
        }
    }
    for (path, msg) in &real_errors {
        eprintln!("  {} {}: {}", c.dim("error:"), c.dim(path), msg);
    }
    if skip_limit > 0 {
        eprintln!("  {} {} file(s) skipped — filesystem limitation (Win32 error 344): cannot be WOF-backed",
                  c.yellow("!"), skip_limit);
    }

    println!();
    let verb = if is_decompress { "decompressed" } else { "compacted" };
    if compressed_n > 0 {
        println!("{} {} {} files ({})",
                 c.green("✓"), c.bold(&verb), c.green(&compressed_n.to_string()),
                 format_size(estimate.candidate_size as i64));
        let real_failures = failed_n.saturating_sub(skip_limit);
        if skip_limit > 0 {
            println!("  {} {} file(s) skipped — filesystem limitation (Win32 error 344): cannot be WOF-backed",
                     c.yellow("!"), skip_limit);
        }
        if real_failures > 0 {
            println!("  {} {} file(s) failed (access denied, locked, or unsupported filesystem)",
                     c.yellow("!"), real_failures);
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

/// Spawn a *detached* `gim` worker process that performs the compaction in
/// the background, then return immediately so the foreground command can
/// exit. The worker is a SEPARATE process (not a thread), so it keeps
/// running after the foreground `gim` exits — fixing the old bug where a
/// `std::thread` died with the parent and did no work.
///
/// The worker re-resolves nothing: we pass the already-resolved options on
/// the command line. It acquires and holds `compact.lock` for its lifetime
/// and writes `compact.state` for `gim compact --status` to read.
fn spawn_background(
    c: &Colorizer,
    paths: &Paths,
    alias: &str,
    opts: CompactOptions,
    _files: Vec<crate::compact::ScannedFile>,
    _estimate: Estimate,
    _auto_pause: bool,
) -> GResult<()> {
    // Exclusivity check: refuse if a worker is already holding the lock.
    // We acquire, confirm it's free, then release so the spawned worker can
    // take the lock for the duration of the background run.
    let lock_path = lock_file_path(&paths.data_dir, alias);
    {
        let probe = LockGuard::try_acquire_exclusive(&lock_path)?;
        match probe {
            Some(_) => {} // free — guard drops here, releasing the lock
            None => {
                return Err(GError::CompactRunning(alias.to_string(), lock_path));
            }
        }
    }

    let state_path = state_file_path(&paths.data_dir, alias);
    let _ = std::fs::remove_file(&state_path); // fresh state for the new run

    println!("starting background compaction...");
    println!("  algorithm: {}", c.dim(opts.algorithm.label()));
    println!("  target:    {}", c.dim(opts.target.as_str()));
    println!("  auto-pause: {}", c.dim(if _auto_pause { "enabled" } else { "disabled" }));

    // Build the worker command line.
    let exe = std::env::current_exe()
        .map_err(|e| GError::Compact(format!("cannot locate gim executable: {e}")))?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("compact").arg(alias)
        .arg("--worker")
        .arg("--confirm") // background run is implicitly confirmed; don't prompt
        .arg("--algorithm").arg(opts.algorithm.as_str())
        .arg("--target").arg(opts.target.as_str())
        .arg("--lock-file").arg(&lock_path);
    if opts.threads > 0 {
        cmd.arg("--threads").arg(opts.threads.to_string());
    }
    for ex in &opts.exclude {
        cmd.arg("--exclude").arg(ex);
    }
    if opts.force {
        cmd.arg("--force");
    }
    // Detach: redirect worker output to a log so it doesn't interleave with
    // the user's terminal, and so failures are observable.
    let worker_log = paths.data_dir.join(alias).join("compact.worker.log");
    if let Some(parent) = worker_log.parent() { let _ = std::fs::create_dir_all(parent); }
    match std::fs::File::create(&worker_log) {
        Ok(f) => { cmd.stdout(f.try_clone().unwrap_or_else(|_| std::fs::File::open(&worker_log).unwrap())); cmd.stderr(f); }
        Err(_) => { cmd.stdin(std::process::Stdio::null()); }
    }
    cmd.stdin(std::process::Stdio::null());

    match cmd.spawn() {
        Ok(child) => {
            // Detach: we do NOT wait. The child outlives this process.
            let _ = child.id();
            std::mem::forget(child);
        }
        Err(e) => {
            return Err(GError::Compact(format!(
                "failed to spawn background worker: {e}")));
        }
    }

    println!();
    println!("  check status: {}", c.dim(&format!("gim compact {} --status", alias)));
    println!("  lockfile: {}", c.dim(&lock_path.to_string_lossy()));
    println!("  worker log: {}", c.dim(&worker_log.to_string_lossy()));
    println!();
    println!("{} background compaction started", c.green("✓"));
    Ok(())
}

/// Worker entry point. Runs in a SEPARATE `gim` process spawned by
/// `spawn_background`. Performs the actual scan + compress/decompress, holds
/// the lock for its lifetime, and writes `compact.state` for `--status`.
fn run_worker(
    c: &Colorizer,
    paths: &Paths,
    alias: &str,
    opts: CompactOptions,
    auto_pause: bool,
    lock_path: PathBuf,
) -> GResult<()> {
    // Adopt the lock so `--status` sees an active run and a second
    // `--background` is refused while we're working.
    let _lock = match LockGuard::try_acquire_exclusive(&lock_path)? {
        Some(g) => g,
        None => {
            return Err(GError::CompactRunning(alias.to_string(), lock_path));
        }
    };

    let state_path = state_file_path(&paths.data_dir, alias);
    let data_dir = paths.data_dir.clone();
    let is_decompress = opts.algorithm.is_decompress();
    let progress = ProgressReporter::new(false); // no live bar in worker

    // Resolve target dirs and scan.
    let game = crate::db::GamesDb::open(&paths.games_db)?
        .get(alias)?
        .ok_or_else(|| GError::AliasNotFound(alias.to_string()))?;
    let target_dirs = resolve_target_dirs(&game.game_dir, &game.data_dir, paths, alias, opts.target);
    let mut all_files: Vec<crate::compact::ScannedFile> = Vec::new();
    for dir in &target_dirs {
        if !dir.exists() { continue; }
        let files = scan(dir, &opts.exclude, is_decompress, &progress)?;
        all_files.extend(files);
    }
    let estimate = summarize(&all_files, opts.algorithm);
    let candidates: Vec<crate::compact::ScannedFile> = all_files
        .iter()
        .filter(|f| matches!(f.kind, FileKind::Candidate(_)))
        .cloned()
        .collect();
    let total = candidates.len() as u64;

    // Initial state.
    let mut state = CompactState::new(opts.algorithm, opts.target);
    state.total_files = total;
    state.total_size = estimate.candidate_size;
    state.phase = CompactPhase::Running.as_str().to_string();
    let _ = state.save(&state_path);

    let compressed = AtomicU64::new(0);
    let failed = AtomicU64::new(0);
    let failed_details: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
    let pause_flag = Arc::new(AtomicBool::new(false));

    let batch_size = 64usize;
    let mut idx = 0usize;
    while idx < candidates.len() {
        if auto_pause {
            let gdb = match GamesDb::open(&data_dir.join("games.db")) {
                Ok(db) => db,
                Err(_) => { std::thread::sleep(Duration::from_secs(1)); continue; }
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

        let end = (idx + batch_size).min(candidates.len());
        for f in &candidates[idx..end] {
            let r = if is_decompress {
                decompress_file(&f.path)
            } else {
                compress_file(&f.path, &opts)
            };
            match r {
                Ok(()) => { compressed.fetch_add(1, Ordering::Relaxed); }
                Err(e) => {
                    failed.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut v) = failed_details.lock() {
                        v.push((f.path.display().to_string(), e.to_string()));
                    }
                }
            }
        }
        idx = end;

        state.processed_files = idx as u64;
        state.compressed_files = compressed.load(Ordering::Relaxed);
        state.failed_files = failed.load(Ordering::Relaxed);
        state.updated_at = unix_now();
        let _ = state.save(&state_path);
    }

    // Report per-file errors WITH paths (BUG 3 fix). Win32 error 344
    // (ERROR_FILE_SYSTEM_LIMITATION) means the file simply cannot be
    // WOF-backed — report those as skipped, not as failures.
    let details = failed_details.into_inner().unwrap_or_default();
    let mut skip_limit = 0u64;
    let mut real_errors: Vec<(String, String)> = Vec::new();
    for (path, msg) in &details {
        if msg.contains("Win32 error 344") {
            skip_limit += 1;
        } else {
            real_errors.push((path.clone(), msg.clone()));
        }
    }
    for (path, msg) in &real_errors {
        eprintln!("  {} {}: {}", c.dim("error:"), c.dim(path), msg);
    }
    if skip_limit > 0 {
        eprintln!("  {} {} file(s) skipped — filesystem limitation (Win32 error 344): cannot be WOF-backed",
                  c.yellow("!"), skip_limit);
    }

    // Done.
    let compressed_n = compressed.load(Ordering::Relaxed);
    let failed_n = failed.load(Ordering::Relaxed);
    let real_failures = failed_n.saturating_sub(skip_limit);
    state.processed_files = total;
    state.phase = CompactPhase::Done.as_str().to_string();
    state.message = if skip_limit > 0 && real_failures == 0 {
        format!("completed: {} compressed, {} skipped (Win32 344)", compressed_n, skip_limit)
    } else if real_failures > 0 {
        format!("completed: {} compressed, {} failed, {} skipped (Win32 344)",
                compressed_n, real_failures, skip_limit)
    } else {
        format!("completed: {} compressed", compressed_n)
    };
    let _ = state.save(&state_path);

    let verb = if is_decompress { "decompressed" } else { "compacted" };
    println!("{} {} {} files",
             c.green("✓"), verb, compressed_n);
    if skip_limit > 0 {
        println!("  {} {} file(s) skipped — filesystem limitation (Win32 error 344): cannot be WOF-backed",
                 c.yellow("!"), skip_limit);
    }
    if real_failures > 0 {
        println!("  {} {} file(s) failed (access denied, locked, or unsupported filesystem)",
                 c.yellow("!"), real_failures);
    }
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
