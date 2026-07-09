pub mod color;
pub mod fmt;
pub mod progress;

pub use color::Colorizer;
pub use fmt::{format_size, format_size_compact, format_timestamp};
pub use progress::ProgressReporter;

pub fn default_colorizer() -> Colorizer {
    Colorizer::new(std::env::var_os("NO_COLOR").is_none() && atty_stdout())
}
fn atty_stdout() -> bool { use std::io::IsTerminal; std::io::stdout().is_terminal() }
