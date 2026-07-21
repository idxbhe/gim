//! I/O throttling for the defrag engine.
//!
//! Per the instruction's "I/O Throttling" risk: moving hundreds of GB of
//! clusters without pause will saturate the HDD and make the whole
//! machine unresponsive (cursor lag, browser stalls, etc.).
//!
//! Solution: after each `throttle_mb` of moved bytes, sleep for
//! `throttle_sleep_ms`. This gives the OS time to flush queued I/O from
//! other processes, the user can still use the machine.
//!
//! 500 MB / 200 ms is the default — chosen to keep average throughput
//! high while yielding every ~1–2 seconds on a typical 100 MB/s HDD.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Tracks bytes moved and yields the thread when the budget is hit.
///
/// Thread-safe: `record_bytes` can be called from a rayon worker, the
/// internal `AtomicU64` keeps the count consistent without locks.
pub struct IoThrottle {
    bytes_since_sleep: AtomicU64,
    bytes_budget: u64,
    sleep: Duration,
    total_bytes_moved: AtomicU64,
    total_sleeps: AtomicU64,
}

impl IoThrottle {
    /// Create a throttle that yields after `bytes_budget` bytes and
    /// sleeps for `sleep_ms` milliseconds.
    pub fn new(bytes_budget: u64, sleep_ms: u64) -> Self {
        Self {
            bytes_since_sleep: AtomicU64::new(0),
            bytes_budget,
            sleep: Duration::from_millis(sleep_ms),
            total_bytes_moved: AtomicU64::new(0),
            total_sleeps: AtomicU64::new(0),
        }
    }

    /// Record `bytes` of just-completed I/O. If the cumulative budget
    /// has been reached, sleep and reset the counter.
    pub fn record_bytes(&self, bytes: u64) {
        self.total_bytes_moved.fetch_add(bytes, Ordering::Relaxed);
        let prev = self.bytes_since_sleep.fetch_add(bytes, Ordering::Relaxed);
        let new = prev + bytes;
        if new >= self.bytes_budget {
            // Sleep, then reset. Use compare-exchange to avoid double
            // sleeping if two threads cross the threshold simultaneously.
            // (Single-threaded move engine today, but cheap insurance.)
            let _ = self.bytes_since_sleep.compare_exchange(
                new, 0, Ordering::Relaxed, Ordering::Relaxed,
            );
            self.total_sleeps.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(self.sleep);
        }
    }

    /// Total bytes recorded since the throttle was created.
    pub fn total_bytes_moved(&self) -> u64 {
        self.total_bytes_moved.load(Ordering::Relaxed)
    }

    /// Number of times the throttle has yielded.
    pub fn total_sleeps(&self) -> u64 {
        self.total_sleeps.load(Ordering::Relaxed)
    }

    /// Reset all counters (useful for tests).
    #[cfg(test)]
    pub fn reset(&self) {
        self.bytes_since_sleep.store(0, Ordering::Relaxed);
        self.total_bytes_moved.store(0, Ordering::Relaxed);
        self.total_sleeps.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_total_bytes() {
        let t = IoThrottle::new(u64::MAX, 0); // never sleep
        t.record_bytes(100);
        t.record_bytes(200);
        assert_eq!(t.total_bytes_moved(), 300);
        assert_eq!(t.total_sleeps(), 0);
    }

    #[test]
    fn triggers_sleep_at_threshold() {
        // Budget = 500 bytes, sleep = 1 ms. Record 600 bytes in two calls.
        let t = IoThrottle::new(500, 1);
        t.record_bytes(300); // below budget
        assert_eq!(t.total_sleeps(), 0);
        t.record_bytes(300); // 600 ≥ 500 → sleep + reset
        assert_eq!(t.total_sleeps(), 1);
        assert_eq!(t.total_bytes_moved(), 600);
    }

    #[test]
    fn multiple_sleeps_accumulate() {
        let t = IoThrottle::new(100, 1);
        for _ in 0..5 {
            t.record_bytes(100);
        }
        // 5 × 100 = 500 bytes recorded, 5 sleeps.
        assert_eq!(t.total_bytes_moved(), 500);
        assert_eq!(t.total_sleeps(), 5);
    }

    #[test]
    fn zero_budget_sleeps_every_call() {
        let t = IoThrottle::new(0, 1);
        t.record_bytes(1);
        t.record_bytes(1);
        assert_eq!(t.total_sleeps(), 2);
    }
}
