use colored::Colorize;

/// Git-like color palette helper.
///
/// Color conventions (matching git where possible):
/// - **green**: success, added files, current branch
/// - **red**: errors, deleted files
/// - **yellow**: warnings, modified files, snapshot IDs (like git commit hashes)
/// - **cyan**: secondary info, branch markers
/// - **magenta**: branch names (git uses green for current branch, but
///   magenta is more distinctive for non-current branches in `gim log`)
/// - **bold**: headers, important identifiers
/// - **dim**: secondary text, timestamps, sizes
pub struct Colorizer { enabled: bool }

impl Colorizer {
    pub fn new(e: bool) -> Self { colored::control::set_override(e); Self { enabled: e } }
    pub fn enabled(&self) -> bool { self.enabled }

    pub fn green(&self, s: &str) -> String { if self.enabled { s.green().to_string() } else { s.to_string() } }
    pub fn red(&self, s: &str) -> String { if self.enabled { s.red().to_string() } else { s.to_string() } }
    pub fn yellow(&self, s: &str) -> String { if self.enabled { s.yellow().to_string() } else { s.to_string() } }
    pub fn cyan(&self, s: &str) -> String { if self.enabled { s.cyan().to_string() } else { s.to_string() } }
    pub fn magenta(&self, s: &str) -> String { if self.enabled { s.magenta().to_string() } else { s.to_string() } }
    pub fn blue(&self, s: &str) -> String { if self.enabled { s.blue().to_string() } else { s.to_string() } }
    pub fn bold(&self, s: &str) -> String { if self.enabled { s.bold().to_string() } else { s.to_string() } }
    pub fn dim(&self, s: &str) -> String { if self.enabled { s.dimmed().to_string() } else { s.to_string() } }

    /// Git-style status label: green "added", red "deleted", yellow "modified".
    /// Returns a right-padded label for alignment.
    pub fn status_label(&self, kind: &str) -> String {
        let padded = format!("{:<8}", kind);
        match kind {
            "added" => self.green(&padded),
            "deleted" => self.red(&padded),
            "modified" => self.yellow(&padded),
            _ => self.dim(&padded),
        }
    }
}
