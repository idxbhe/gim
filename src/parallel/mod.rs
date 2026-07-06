//! Placeholder for the parallel module. Currently we use Rayon directly
//! from the `walker` module; this file exists so that future parallel
//! utilities (e.g. parallel restore workers) have a natural home.

pub use rayon;
