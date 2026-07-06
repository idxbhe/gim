//! Terminal color helpers — thin wrapper around `colored`.
//!
//! All color usage goes through this so we can disable it globally.

use colored::Colorize;

pub struct Colorizer {
    enabled: bool,
}

impl Colorizer {
    pub fn new(enabled: bool) -> Self {
        colored::control::set_override(enabled);
        Self { enabled }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn green(&self, s: &str) -> String {
        if self.enabled {
            s.green().to_string()
        } else {
            s.to_string()
        }
    }

    pub fn red(&self, s: &str) -> String {
        if self.enabled {
            s.red().to_string()
        } else {
            s.to_string()
        }
    }

    pub fn yellow(&self, s: &str) -> String {
        if self.enabled {
            s.yellow().to_string()
        } else {
            s.to_string()
        }
    }

    pub fn cyan(&self, s: &str) -> String {
        if self.enabled {
            s.cyan().to_string()
        } else {
            s.to_string()
        }
    }

    pub fn bold(&self, s: &str) -> String {
        if self.enabled {
            s.bold().to_string()
        } else {
            s.to_string()
        }
    }

    pub fn dim(&self, s: &str) -> String {
        if self.enabled {
            s.dimmed().to_string()
        } else {
            s.to_string()
        }
    }
}
