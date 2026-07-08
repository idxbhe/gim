//! Parallel utilities.
//!
//! Provides a lazily-initialized global Rayon thread pool so that the
//! pool is created **once** per process (not per command call). The
//! pool is configured with the thread count from the first `--threads`
//! flag the user passes, or defaults to `num_cpus` if never set.

use rayon::ThreadPool;
use std::sync::OnceLock;

static GLOBAL_POOL: OnceLock<ThreadPool> = OnceLock::new();

/// Configure the global thread pool with `n` threads. Should be called
/// once at startup (from main.rs) if the user passed `--threads N`.
///
/// If called multiple times, only the first call has effect — the
/// pool is immutable after initialization. Subsequent calls are
/// silently ignored (the first `--threads` wins).
pub fn configure(num_threads: usize) {
    if GLOBAL_POOL.get().is_some() { return; }
    let _ = GLOBAL_POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .expect("cannot build global thread pool")
    });
}

/// Get a reference to the global thread pool. If `configure()` was
/// never called, the pool is lazily initialized with `num_cpus` threads.
pub fn global() -> &'static ThreadPool {
    GLOBAL_POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .build()
            .expect("cannot build default thread pool")
    })
}

pub use rayon;
