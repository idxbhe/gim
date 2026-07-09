use rayon::ThreadPool;
use std::sync::OnceLock;

static GLOBAL_POOL: OnceLock<ThreadPool> = OnceLock::new();

pub fn configure(n: usize) {
    if GLOBAL_POOL.get().is_some() { return; }
    let _ = GLOBAL_POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().num_threads(n).build().expect("pool"));
}
pub fn global() -> &'static ThreadPool {
    GLOBAL_POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().build().expect("pool"))
}
pub use rayon;
