//! Output formatting — terminal colors, size formatting, JSON helpers.
//!
//! All user-facing output goes through this module so that colors can
//! be globally toggled (e.g. via `--no-color` or `NO_COLOR` env var),
//! and so that size/date formatting is consistent across commands.


pub mod color;
pub mod fmt;

pub use color::Colorizer;
pub use fmt::{format_size, format_size_compact, format_timestamp};

/// Default colorizer used by the CLI. Respects `NO_COLOR` env var
/// and whether stdout is a TTY.
pub fn default_colorizer() -> Colorizer {
    let enable = std::env::var_os("NO_COLOR").is_none()
        && atty_stdout();
    Colorizer::new(enable)
}

fn atty_stdout() -> bool {
    // Best-effort TTY detection without an extra dependency.
    // `std::io::IsTerminal` is stable since 1.70.
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}
