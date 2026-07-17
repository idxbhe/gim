//! Pretty progress reporting — fully manual, no indicatif, with colors.
//!
//! Two phase types:
//! - **Spinner** (unknown total): cyan `|/-\` rotating + label + count
//! - **Progress bar** (known total): green `━` fill on dim `╌` track
//!   + yellow percentage + count
//!
//! When a phase finishes, the bar is cleared and only a one-line
//! summary is printed: `✓ walked 9744` (green checkmark + bold label
//! + count).
//!
//! Output goes to stderr (so stdout / `--json` is never corrupted).
//! Auto-disables when stderr is not a TTY or `--no-progress` /
//! `GIM_NO_PROGRESS` is set. Colors auto-disable when `NO_COLOR` env
//! var is set.
//!
//! ## Windows ANSI Support
//!
//! On Windows 10+, ANSI escape codes require Virtual Terminal (VT)
//! mode to be explicitly enabled on the console handle. This module
//! calls `enable_windows_ansi()` once during initialization to ensure
//! colors render correctly instead of showing raw codes like `[32m`.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

// ── Windows VT mode enable ────────────────────────────────────────
// Windows 10+ supports ANSI escape codes but requires Virtual Terminal
// processing to be enabled via SetConsoleMode. We call this once at
// startup so raw `\x1b[...m` sequences render as colors, not garbage.
//
// On non-Windows platforms this is a no-op (compiled out).

#[cfg(target_os = "windows")]
fn enable_windows_ansi() {
    use std::sync::atomic::{AtomicBool, Ordering};

    static ENABLED: AtomicBool = AtomicBool::new(false);

    if ENABLED.compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed).is_err() {
        return; // Already attempted
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(nstdhandle: i32) -> *mut std::ffi::c_void;
        fn GetConsoleMode(hconsole_handle: *mut std::ffi::c_void, lpmode: *mut u32) -> i32;
        fn SetConsoleMode(hconsole_handle: *mut std::ffi::c_void, dwmode: u32) -> i32;
    }

    const STD_ERROR_HANDLE: i32 = -12i32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    unsafe {
        let handle = GetStdHandle(STD_ERROR_HANDLE);
        if handle.is_null() {
            return;
        }

        let mut mode: u32 = 0;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return; // Not a console (e.g., redirected to file/pipe)
        }

        // Enable VT processing bit while preserving existing modes.
        let _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
    }
}

#[cfg(not(target_os = "windows"))]
fn enable_windows_ansi() {
    // No-op on Unix/macOS — terminals support ANSI natively.
}

// ── ANSI color codes ────────────────────────────────────────────────
// We use raw ANSI codes instead of the `colored` crate for the
// progress bar frames because they're drawn frequently by a background
// thread and we want minimal allocation overhead.

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_CYAN: &str = "\x1b[36m";
const ANSI_WHITE: &str = "\x1b[37m";

/// ASCII-only spinner frames. Works on every terminal including
/// legacy Windows cmd.exe (braille `⠋` renders as `[?]` there).
const SPINNER_FRAMES: &[&str] = &["|", "/", "-", "\\"];

/// Checkmark shown when a phase completes. U+2713 (CHECK MARK).
const CHECKMARK: &str = "✓";

/// pip-style bar characters.
const BAR_FILL: char = '━';
const BAR_TRACK: char = '╌';

/// Width of the progress bar in characters.
const BAR_WIDTH: usize = 30;

/// Width for brute-force line clear.
const CLEAR_WIDTH: usize = 120;

/// Internal state for a progress phase.
struct PhaseState {
    count: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

pub struct ProgressReporter {
    enabled: bool,
    use_color: bool,
    phase: Mutex<Option<PhaseState>>,
}

impl ProgressReporter {
    pub fn new(enabled: bool) -> Self {
        // Enable Windows VT mode so ANSI escape codes render as colors
        // instead of raw text like `[32m`. No-op on non-Windows.
        enable_windows_ansi();

        // Colors enabled when: progress enabled AND NO_COLOR not set.
        let use_color = enabled && std::env::var_os("NO_COLOR").is_none();
        Self {
            enabled,
            use_color,
            phase: Mutex::new(None),
        }
    }

    pub fn enabled(&self) -> bool { self.enabled }

    // ── Color helpers ───────────────────────────────────────────────

    fn color(&self, code: &str, s: &str) -> String {
        if self.use_color {
            format!("{code}{s}{ANSI_RESET}")
        } else {
            s.to_string()
        }
    }

    // ── Generic phase API ───────────────────────────────────────────

    /// Start a new phase. If `total` is 0, a spinner (unknown total)
    /// is shown; otherwise a progress bar (known total) is shown.
    pub fn phase_start(&self, label: &str, total: usize) {
        if !self.enabled { return; }
        self.phase_cancel(); // clean up any previous phase

        let count = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let label_owned = label.to_string();
        let total_u64 = total as u64;
        let use_color = self.use_color;

        // Initial draw so the bar appears instantly.
        self.draw_frame(&label_owned, 0, total_u64, 0);

        let count_clone = count.clone();
        let stop_clone = stop.clone();
        let label_for_thread = label_owned;

        let handle = thread::spawn(move || {
            let mut i = 0usize;
            loop {
                thread::sleep(Duration::from_millis(80));
                if stop_clone.load(Ordering::Relaxed) { break; }
                let n = count_clone.load(Ordering::Relaxed);
                let frame = SPINNER_FRAMES[i % SPINNER_FRAMES.len()];
                draw_frame_raw(frame, &label_for_thread, n, total_u64, use_color);
                let _ = std::io::stderr().flush();
                i += 1;
            }
        });

        *self.phase.lock().unwrap() = Some(PhaseState {
            count,
            stop,
            handle: Some(handle),
        });
    }

    pub fn phase_tick(&self) {
        if !self.enabled { return; }
        let guard = self.phase.lock().unwrap();
        if let Some(ref ps) = *guard {
            ps.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Finish the current phase — clear the bar and print a one-line
    /// summary with checkmark.
    ///
    /// 1. Signal the background thread to stop and join it.
    /// 2. Brute-force clear the current line (erase the bar).
    /// 3. Print summary: `✓ walked 9744` (green checkmark + bold
    ///    label + count).
    /// 4. Print a newline.
    pub fn phase_done(&self, past_tense: &str) {
        if !self.enabled { return; }
        let mut guard = self.phase.lock().unwrap();
        if let Some(mut ps) = guard.take() {
            // Signal thread to stop.
            ps.stop.store(true, Ordering::Relaxed);
            // Wait for thread to exit so no more draws happen.
            if let Some(h) = ps.handle.take() {
                let _ = h.join();
            }
            let count = ps.count.load(Ordering::Relaxed);
            drop(guard);

            // Brute-force clear the current line — erase the bar.
            eprint!("\r{}\r", " ".repeat(CLEAR_WIDTH));

            // Print summary: green ✓ + bold past-tense + count.
            // Example: "✓ walked 9,744"
            let check = self.color(ANSI_GREEN, CHECKMARK);
            let label = self.color(ANSI_BOLD, past_tense);
            let count_str = format_count(count);
            eprint!("{check} {label} {count_str}");

            // Newline to move to next line.
            eprintln!();
            let _ = std::io::stderr().flush();
        }
    }

    /// Cancel the current phase — clear the bar without a summary.
    /// Used for error paths and transitions (e.g. preparing → walk).
    pub fn phase_cancel(&self) {
        let mut guard = self.phase.lock().unwrap();
        if let Some(mut ps) = guard.take() {
            ps.stop.store(true, Ordering::Relaxed);
            if let Some(h) = ps.handle.take() {
                let _ = h.join();
            }
            drop(guard);
            // Brute-force clear — NO newline, stay on same line.
            eprint!("\r{}\r", " ".repeat(CLEAR_WIDTH));
            let _ = std::io::stderr().flush();
        }
    }

    /// Draw a single frame (used for initial draw).
    fn draw_frame(&self, label: &str, count: u64, total: u64, spinner_idx: usize) {
        let frame = SPINNER_FRAMES[spinner_idx % SPINNER_FRAMES.len()];
        draw_frame_raw(frame, label, count, total, self.use_color);
        let _ = std::io::stderr().flush();
    }

    // ── Convenience wrappers ────────────────────────────────────────

    pub fn walk_start(&self) { self.phase_start("walking", 0); }
    pub fn walk_tick(&self) { self.phase_tick(); }
    pub fn walk_done(&self, _count: u64) { self.phase_done("walked"); }

    pub fn hash_start(&self, total: usize) { self.phase_start("hashing", total); }
    pub fn hash_tick(&self) { self.phase_tick(); }
    pub fn hash_done(&self, _count: u64) { self.phase_done("hashed"); }

    pub fn copy_start(&self, total: usize) { self.phase_start("copying", total); }
    pub fn copy_tick(&self) { self.phase_tick(); }
    pub fn copy_done(&self, _count: u64) { self.phase_done("copied"); }

    pub fn store_start(&self, total: usize) { self.phase_start("storing", total); }
    pub fn store_tick(&self) { self.phase_tick(); }
    pub fn store_done(&self, _count: u64) { self.phase_done("stored"); }

    pub fn scan_start(&self) { self.phase_start("scanning", 0); }
    pub fn scan_tick(&self) { self.phase_tick(); }
    pub fn scan_done(&self, _count: u64) { self.phase_done("scanned"); }

    pub fn delete_start(&self, total: usize) { self.phase_start("deleting", total); }
    pub fn delete_tick(&self) { self.phase_tick(); }
    pub fn delete_done(&self, _count: u64) { self.phase_done("deleted"); }

    pub fn compact_start(&self, total: usize) { self.phase_start("compacting", total); }
    pub fn compact_tick(&self) { self.phase_tick(); }
    pub fn compact_done(&self, _count: u64) { self.phase_done("compacted"); }

    pub fn decompress_start(&self, total: usize) { self.phase_start("decompressing", total); }
    pub fn decompress_tick(&self) { self.phase_tick(); }
    pub fn decompress_done(&self, _count: u64) { self.phase_done("decompressed"); }
}

impl Drop for ProgressReporter {
    fn drop(&mut self) {
        self.phase_cancel();
    }
}

/// Draw a single frame to stderr with raw writes and colors.
///
/// Layout for progress bar (known total):
/// ```text
/// <cyan spinner> <bold label> <green fill><dim track> <count>/<total> <yellow pct>%
/// ```
///
/// Layout for spinner (unknown total):
/// ```text
/// <cyan spinner> <bold label>... <count>
/// ```
fn draw_frame_raw(spinner: &str, label: &str, count: u64, total: u64, use_color: bool) {
    // Clear line first.
    eprint!("\r{}\r", " ".repeat(CLEAR_WIDTH));

    if total > 0 {
        // Progress bar phase.
        let pct = if total == 0 { 100 } else { (count * 100 / total) as usize };
        let filled = if total == 0 { BAR_WIDTH } else {
            ((count as usize) * BAR_WIDTH / total as usize).min(BAR_WIDTH)
        };

        if use_color {
            // Colored bar: green fill + dim track.
            let fill: String = std::iter::repeat(BAR_FILL).take(filled).collect();
            let track: String = std::iter::repeat(BAR_TRACK).take(BAR_WIDTH - filled).collect();
            eprint!(
                "{ANSI_CYAN}{spinner}{ANSI_RESET} {ANSI_BOLD}{label:<10}{ANSI_RESET} {ANSI_GREEN}{fill}{ANSI_RESET}{ANSI_DIM}{track}{ANSI_RESET} {count}/{total} {ANSI_YELLOW}{pct:>3}%{ANSI_RESET}"
            );
        } else {
            // No color.
            let bar: String = std::iter::repeat(BAR_FILL).take(filled)
                .chain(std::iter::repeat(BAR_TRACK).take(BAR_WIDTH - filled))
                .collect();
            eprint!("{spinner} {label:<10} {bar} {count}/{total} {pct:>3}%");
        }
    } else {
        // Spinner phase.
        if use_color {
            eprint!(
                "{ANSI_CYAN}{spinner}{ANSI_RESET} {ANSI_BOLD}{label}{ANSI_RESET}... {ANSI_WHITE}{count}{ANSI_RESET}"
            );
        } else {
            eprint!("{spinner} {label}... {count}");
        }
    }
}

/// Format a number with thousands separator (12345 → "12,345").
fn format_count(n: u64) -> String {
    let s = n.to_string();
    let chars: Vec<char> = s.chars().rev().collect();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in chars.iter().enumerate() {
        if i > 0 && i % 3 == 0 { out.insert(0, ','); }
        out.insert(0, *ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_is_noop() {
        let r = ProgressReporter::new(false);
        r.walk_start(); r.walk_tick(); r.walk_done(0);
        r.hash_start(100); r.hash_tick(); r.hash_done(0);
        r.copy_start(50); r.copy_tick(); r.copy_done(0);
        r.store_start(10); r.store_tick(); r.store_done(0);
        r.scan_start(); r.scan_tick(); r.scan_done(0);
        r.delete_start(5); r.delete_tick(); r.delete_done(0);
        r.compact_start(5); r.compact_tick(); r.compact_done(0);
        r.decompress_start(3); r.decompress_tick(); r.decompress_done(0);
    }

    #[test]
    fn spinner_frames_are_ascii() {
        for frame in SPINNER_FRAMES {
            assert!(frame.is_ascii(), "spinner frame \"{frame}\" contains non-ASCII");
        }
    }

    #[test]
    fn enabled_bar_starts_and_finishes() {
        let r = ProgressReporter::new(true);
        r.hash_start(100);
        r.hash_tick();
        r.hash_tick();
        r.hash_done(2);
    }

    #[test]
    fn enabled_spinner_starts_and_finishes() {
        let r = ProgressReporter::new(true);
        r.walk_start();
        std::thread::sleep(Duration::from_millis(200));
        r.walk_tick();
        r.walk_done(2);
    }

    #[test]
    fn phase_transitions_work() {
        let r = ProgressReporter::new(true);
        r.walk_start();
        std::thread::sleep(Duration::from_millis(50));
        r.hash_start(100);
        r.hash_tick();
        r.hash_done(1);
    }

    #[test]
    fn phase_cancel_works() {
        let r = ProgressReporter::new(true);
        r.walk_start();
        std::thread::sleep(Duration::from_millis(50));
        r.phase_cancel();
    }

    #[test]
    fn format_count_thousands() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(12345), "12,345");
        assert_eq!(format_count(9744), "9,744");
    }
}
