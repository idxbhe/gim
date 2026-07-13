//! Pretty progress reporting — fully manual, no indicatif.
//!
//! Why no indicatif? Its `finish*` methods and steady-ticker leave
//! residue on Windows cmd.exe because the terminal doesn't handle
//! `\r`-based line overwrite the same way Unix terminals do. By
//! implementing the progress bar manually with raw stderr writes,
//! we have full control and can guarantee clean output.
//!
//! Two phase types:
//! - **Spinner** (unknown total): `|/-\` rotating + label + count
//! - **Progress bar** (known total): `━╌` bar + percentage + count
//!
//! When a phase finishes, the final frame shows `✓` + past-tense label
//! + final count, on its own line.
//!
//! Output goes to stderr (so stdout / `--json` is never corrupted).
//! Auto-disables when stderr is not a TTY or `--no-progress` /
//! `GIM_NO_PROGRESS` is set.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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
    total: u64, // 0 = spinner (unknown total)
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

pub struct ProgressReporter {
    enabled: bool,
    phase: Mutex<Option<PhaseState>>,
}

impl ProgressReporter {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            phase: Mutex::new(None),
        }
    }

    pub fn enabled(&self) -> bool { self.enabled }

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
                draw_frame_raw(frame, &label_for_thread, n, total_u64);
                let _ = std::io::stderr().flush();
                i += 1;
            }
        });

        *self.phase.lock().unwrap() = Some(PhaseState {
            count,
            total: total_u64,
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

    /// Finish the current phase with a checkmark.
    ///
    /// 1. Signal the background thread to stop and join it.
    /// 2. Brute-force clear the current line.
    /// 3. Draw the final frame with `✓` + past-tense label + count.
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
            let total = ps.total;
            drop(guard);

            // Brute-force clear the current line.
            eprint!("\r{}\r", " ".repeat(CLEAR_WIDTH));

            // Draw final frame with checkmark.
            if total > 0 {
                let pct = (count * 100 / total) as usize;
                let filled = ((count as usize) * BAR_WIDTH / total as usize).min(BAR_WIDTH);
                let bar: String = std::iter::repeat(BAR_FILL).take(filled)
                    .chain(std::iter::repeat(BAR_TRACK).take(BAR_WIDTH - filled))
                    .collect();
                eprint!("{CHECKMARK} {past_tense} {bar} {count}/{total} {pct:>3}%");
            } else {
                eprint!("{CHECKMARK} {past_tense} {count}");
            }

            // Newline to move to next line.
            eprintln!();
            let _ = std::io::stderr().flush();
        }
    }

    /// Cancel the current phase — clear the bar without a final frame.
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
        draw_frame_raw(frame, label, count, total);
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
}

impl Drop for ProgressReporter {
    fn drop(&mut self) {
        self.phase_cancel();
    }
}

/// Draw a single frame to stderr with raw writes.
/// Uses `\r` to return to column 0, then writes the frame.
fn draw_frame_raw(spinner: &str, label: &str, count: u64, total: u64) {
    if total > 0 {
        let pct = if total == 0 { 100 } else { (count * 100 / total) as usize };
        let filled = if total == 0 { BAR_WIDTH } else {
            ((count as usize) * BAR_WIDTH / total as usize).min(BAR_WIDTH)
        };
        let bar: String = std::iter::repeat(BAR_FILL).take(filled)
            .chain(std::iter::repeat(BAR_TRACK).take(BAR_WIDTH - filled))
            .collect();
        // Clear line first, then draw.
        eprint!("\r{}\r", " ".repeat(CLEAR_WIDTH));
        eprint!("{spinner} {label:<10} {bar} {count}/{total} {pct:>3}%");
    } else {
        eprint!("\r{}\r", " ".repeat(CLEAR_WIDTH));
        eprint!("{spinner} {label}... {count}");
    }
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
}
