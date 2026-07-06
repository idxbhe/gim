//! CLI argument definitions using `clap`'s derive API.
//!
//! Each variant of [`Cli`] is a top-level subcommand. Per spec, every
//! command has a distinct signature; we keep them strongly typed so
//! that the command implementations can pattern-match on the parsed
//! args rather than re-parsing strings.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// `g` — Game Files Version Control Tool.
#[derive(Parser, Debug)]
#[command(
    name = "g",
    version,
    about = "Game files version control tool — git for game directories",
    long_about = "A CLI tool for versioning game files. Uses SQLite for metadata and XXH3 for fast file hashing."
)]
pub struct Cli {
    /// Disable colored output (also disabled if `NO_COLOR` is set or
    /// stdout is not a TTY).
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Increase logging verbosity (`-v` = info, `-vv` = debug, `-vvv` = trace).
    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Add a game to the global registry and start tracking it.
    Add {
        /// Unique game alias (used as the CLI argument in other commands).
        alias: String,
        /// Absolute or relative path to the game directory.
        game_dir: PathBuf,
        /// Optional display title. Defaults to the game directory's base name.
        #[arg(long)]
        title: Option<String>,
        /// Optional data directory. Defaults to `[g binary dir]/data/[alias]`.
        #[arg(long = "dataDir")]
        data_dir: Option<PathBuf>,
    },

    /// Remove a game and all its associated data.
    Remove {
        alias: String,
        /// Required: prevents accidental deletion.
        #[arg(long)]
        confirm: bool,
    },

    /// List all tracked games.
    List {
        /// Show all columns from `games.db`.
        #[arg(long)]
        details: bool,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Take a snapshot of the game directory.
    Snap {
        alias: String,
        /// Custom snapshot ID. Must be unique.
        #[arg(long = "id")]
        id: Option<String>,
        /// Snapshot message.
        #[arg(long = "msg", short = 'm')]
        msg: Option<String>,
        /// Number of worker threads for hashing & copying.
        #[arg(long = "threads", short = 't')]
        threads: Option<usize>,
        /// Preview changes without writing.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// Restore the game directory to match a specific snapshot.
    Restore {
        alias: String,
        /// Target snapshot ID.
        snapshot_id: String,
        /// Force full copy, skip current-state hashing.
        #[arg(long = "full")]
        full: bool,
        /// Number of worker threads.
        #[arg(long = "threads", short = 't')]
        threads: Option<usize>,
        /// Preview changes without modifying files.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// Show file changes since the last snapshot.
    Status {
        alias: String,
        /// Number of worker threads.
        #[arg(long = "threads", short = 't')]
        threads: Option<usize>,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Show snapshot history for a game.
    Log {
        alias: String,
        /// One snapshot per line.
        #[arg(long)]
        oneline: bool,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
        /// Limit number of entries.
        #[arg(short = 'n')]
        n: Option<usize>,
    },

    /// Compare two snapshots and show file differences.
    Diff {
        alias: String,
        snapshot_a: String,
        snapshot_b: String,
        /// Show summary statistics only.
        #[arg(long)]
        stat: bool,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Garbage-collect unreferenced objects.
    Gc {
        alias: String,
        /// Preview without deleting.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// Manage ignore patterns for a game.
    Ignore {
        alias: String,
        /// Add a pattern to the per-game `.gignore`.
        #[arg(long = "add")]
        add: Option<String>,
        /// Remove a pattern from the per-game `.gignore`.
        #[arg(long = "remove")]
        remove: Option<String>,
        /// List all active ignore patterns.
        #[arg(long)]
        list: bool,
        /// Open `.gignore` in the system default editor.
        #[arg(long)]
        edit: bool,
    },
}
