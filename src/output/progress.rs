//! Pretty progress reporting — pip-style thin colored bar + ASCII-safe spinner.
//!
//! Design:
//! - **pip-style bar**: thin `━` fill, cyan colored, dim track.
//!   Single line, overwrites with `\r`. Example:
//!   ```text
//!   | hashing ━━━━━━━━━━━━━━━━━╌╌╌╌╌╌╌╌╌╌  342/500  67%
//!   ```
//! - **ASCII-safe spinner**: `|/-\` rotating. The braille spinner `⠋`
//!   shows as `[?]` on legacy Windows cmd.exe because the console
//!   codepage doesn't include U+2800. ASCII spinner works everywhere.
//! - **Generic phase API**: `phase_start(label, total)`, `phase_tick()`,
//!   `phase_done(summary)`.
//! - **Interior mutability**: all methods take `&self` (shared across
//!   Rayon worker threads). `Mutex<Option<ProgressBar>>` holds the bar.
//! - **Auto-disable**: when stderr is not a TTY, or `--no-progress` /
//!   `GIM_NO_PROGRESS` is set, all methods are no-ops.

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::sync::Mutex;
use std::time::Duration;

/// ASCII-only spinner frames. Works on every terminal including
/// legacy Windows cmd.exe (which renders braille as `[?]`).
const SPINNER_FRAMES: &[&str] = &["|", "/", "-", "\\"];

/// pip-style bar characters. `indicatif`'s `progress_chars()` requires
/// at least 2 characters: the first is the fill, the second is the
/// track (background). A third (optional) is the "partial" char.
///
/// - `━` (U+2501, heavy horizontal) = fill — widely supported
/// - `╌` (U+254C, light dashed) = track — widely supported
///
/// Both are in the Box Drawing / Block Elements blocks, supported by
/// virtually every monospace font including Windows Terminal and
/// modern cmd.exe. We use 2 chars (no partial) for the clean pip look.
const BAR_CHARS: &str = "━╌";

pub struct ProgressReporter {
    enabled: bool,
    bar: Mutex<Option<ProgressBar>>,
}

impl ProgressReporter {
    pub fn new(enabled: bool) -> Self {
        Self { enabled, bar: Mutex::new(None) }
    }

    pub fn enabled(&self) -> bool { self.enabled }

    // ── Generic phase API ───────────────────────────────────────────

    /// Start a new phase with a known total. `label` is shown
    /// left-aligned. If `total` is 0, a spinner (unknown total) is shown.
    pub fn phase_start(&self, label: &str, total: usize) {
        if !self.enabled { return; }
        self.phase_clear();

        let bar = if total == 0 {
            let pb = ProgressBar::new_spinner();
            pb.enable_steady_tick(Duration::from_millis(80));
            pb.set_style(
                ProgressStyle::with_template("{spinner:.cyan} {msg} {pos}")
                    .unwrap()
                    .tick_strings(SPINNER_FRAMES),
            );
            pb.set_message(format!("{label}..."));
            pb
        } else {
            let pb = ProgressBar::new(total as u64);
            // Enable steady tick so the {spinner} keeps rotating even
            // when a single large file takes a long time to process.
            // Without this, the spinner only advances on inc() calls,
            // making it look frozen during slow operations.
            pb.enable_steady_tick(Duration::from_millis(80));
            pb.set_style(
                ProgressStyle::with_template(
                    "{spinner:.cyan} {msg:.bold} {bar:30.cyan/dim} {pos}/{len} {percent:>3}%",
                )
                .unwrap()
                .tick_strings(SPINNER_FRAMES)
                .progress_chars(BAR_CHARS),
            );
            pb.set_message(label.to_string());
            pb.set_draw_target(ProgressDrawTarget::stderr());
            pb
        };
        *self.bar.lock().unwrap() = Some(bar);
    }

    pub fn phase_tick(&self) {
        if !self.enabled { return; }
        if let Some(ref b) = *self.bar.lock().unwrap() { b.inc(1); }
    }

    /// Finish the current phase. `summary` is printed to stderr after
    /// the bar is cleared. Pass empty string to skip.
    ///
    /// On Windows cmd.exe, `indicatif`'s `finish_and_clear()` may not
    /// properly erase the bar line (ANSI clear sequences aren't always
    /// supported). We manually overwrite the line with spaces as a
    /// fallback, then print the summary on a fresh line.
    pub fn phase_done(&self, summary: &str) {
        if !self.enabled { return; }
        let mut guard = self.bar.lock().unwrap();
        if let Some(b) = guard.take() {
            b.finish_and_clear();
            drop(guard);
            // Fallback clear: overwrite the entire line with spaces,
            // then carriage-return to start. This handles terminals
            // where indicatif's clear sequence didn't work (notably
            // legacy Windows cmd.exe).
            eprint!("\r{}\r", " ".repeat(100));
            if !summary.is_empty() {
                eprintln!("  {summary}");
            } else {
                // No summary — still need a newline to move past the
                // cleared line.
                eprintln!();
            }
        }
    }

    fn phase_clear(&self) {
        if let Some(b) = self.bar.lock().unwrap().take() {
            b.finish_and_clear();
            eprint!("\r{}\r", " ".repeat(100));
        }
    }

    // ── Convenience wrappers ────────────────────────────────────────

    pub fn walk_start(&self) { self.phase_start("walking", 0); }
    pub fn walk_tick(&self) { self.phase_tick(); }
    pub fn walk_done(&self, count: u64) {
        self.phase_done(&format!("walked {} files", format_count(count)));
    }

    pub fn hash_start(&self, total: usize) { self.phase_start("hashing", total); }
    pub fn hash_tick(&self) { self.phase_tick(); }
    pub fn hash_done(&self, count: u64) {
        self.phase_done(&format!("hashed {} files", format_count(count)));
    }

    pub fn copy_start(&self, total: usize) { self.phase_start("copying", total); }
    pub fn copy_tick(&self) { self.phase_tick(); }
    pub fn copy_done(&self, count: u64) {
        self.phase_done(&format!("copied {} files", format_count(count)));
    }

    pub fn store_start(&self, total: usize) { self.phase_start("storing", total); }
    pub fn store_tick(&self) { self.phase_tick(); }
    pub fn store_done(&self, count: u64) {
        self.phase_done(&format!("stored {} objects", format_count(count)));
    }

    pub fn scan_start(&self) { self.phase_start("scanning", 0); }
    pub fn scan_tick(&self) { self.phase_tick(); }
    pub fn scan_done(&self, count: u64) {
        self.phase_done(&format!("scanned {} objects", format_count(count)));
    }

    pub fn delete_start(&self, total: usize) { self.phase_start("deleting", total); }
    pub fn delete_tick(&self) { self.phase_tick(); }
    pub fn delete_done(&self, count: u64) {
        self.phase_done(&format!("deleted {} objects", format_count(count)));
    }
}

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
    }

    #[test]
    fn format_count_thousands() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(12345), "12,345");
    }

    #[test]
    fn spinner_frames_are_ascii() {
        for frame in SPINNER_FRAMES {
            assert!(frame.is_ascii(), "spinner frame \"{frame}\" contains non-ASCII");
        }
    }

    #[test]
    fn bar_chars_has_at_least_two() {
        // indicatif's progress_chars() panics with fewer than 2 chars.
        // This test guards against regression.
        let count = BAR_CHARS.chars().count();
        assert!(count >= 2, "BAR_CHARS must have at least 2 chars, got {count}: {BAR_CHARS:?}");
    }

    #[test]
    fn enabled_reporter_does_not_panic() {
        // Regression test: creating a progress bar with a known total
        // must not panic. This catches the "at least 2 progress chars
        // required" bug that crashed `gim status` on Windows.
        let r = ProgressReporter::new(true);
        r.hash_start(100);
        r.hash_tick();
        r.hash_tick();
        r.hash_done(2);
    }
}
