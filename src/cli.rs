//! CLI argument definitions using `clap`'s derive API.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// `gim` — Game Files Version Control Tool.
#[derive(Parser, Debug)]
#[command(
    name = "gim",
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
        alias: String,
        game_dir: PathBuf,
        #[arg(long)]
        title: Option<String>,
        #[arg(long = "dataDir")]
        data_dir: Option<PathBuf>,
    },

    /// Remove a game and all its associated data.
    Remove {
        alias: String,
        #[arg(long)]
        confirm: bool,
    },

    /// List all tracked games.
    List {
        #[arg(long)]
        details: bool,
        #[arg(long)]
        json: bool,
    },

    /// Take a snapshot of the game directory.
    Snap {
        alias: String,
        #[arg(long = "id")]
        id: Option<String>,
        #[arg(long = "msg", short = 'm')]
        msg: Option<String>,
        #[arg(long = "threads", short = 't')]
        threads: Option<usize>,
        #[arg(long = "dry-run")]
        dry_run: bool,
        /// Force full hashing of every file, ignoring the mtime+size
        /// fast pre-filter. Slower but useful when the user suspects
        /// the stored mtime is misleading.
        #[arg(long = "full-hash")]
        full_hash: bool,
    },

    /// Restore the game directory to match a specific snapshot.
    Restore {
        alias: String,
        snapshot_id: String,
        /// Force full copy from the snapshot — skip current-state
        /// hashing entirely. Use this when the on-disk state is
        /// suspected to be corrupted.
        #[arg(long = "full")]
        full: bool,
        #[arg(long = "threads", short = 't')]
        threads: Option<usize>,
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// Show file changes since the last snapshot.
    Status {
        alias: String,
        #[arg(long = "threads", short = 't')]
        threads: Option<usize>,
        #[arg(long)]
        json: bool,
        /// Force full hashing of every file, ignoring the mtime+size
        /// fast pre-filter.
        #[arg(long = "full-hash")]
        full_hash: bool,
    },

    /// Show snapshot history for a game.
    Log {
        alias: String,
        #[arg(long)]
        oneline: bool,
        #[arg(long)]
        json: bool,
        #[arg(short = 'n')]
        n: Option<usize>,
    },

    /// Compare two snapshots and show file differences.
    Diff {
        alias: String,
        snapshot_a: String,
        snapshot_b: String,
        #[arg(long)]
        stat: bool,
        #[arg(long)]
        json: bool,
    },

    /// Garbage-collect unreferenced objects.
    Gc {
        alias: String,
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// Manage ignore patterns for a game.
    Ignore {
        alias: String,
        #[arg(long = "add")]
        add: Option<String>,
        #[arg(long = "remove")]
        remove: Option<String>,
        #[arg(long)]
        list: bool,
        #[arg(long)]
        edit: bool,
    },
}
