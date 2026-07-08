//! Output formatting.

pub mod color;
pub mod fmt;

pub use color::Colorizer;
pub use fmt::{format_size, format_size_compact, format_timestamp};

pub fn default_colorizer() -> Colorizer {
    let enable = std::env::var_os("NO_COLOR").is_none() && atty_stdout();
    Colorizer::new(enable)
}

fn atty_stdout() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}
