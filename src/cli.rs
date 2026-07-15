use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "gim", version, about = "Game files version control tool")]
pub struct Cli {
    #[arg(long, global = true)] pub no_color: bool,
    /// Disable progress spinner/bar (also disabled if stderr is not a TTY
    /// or if GIM_NO_PROGRESS env var is set).
    #[arg(long, global = true)] pub no_progress: bool,
    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)] pub verbose: u8,
    #[command(subcommand)] pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Add { alias: String, game_dir: PathBuf, #[arg(long)] title: Option<String>, #[arg(long = "dataDir")] data_dir: Option<PathBuf> },
    Remove { alias: String, #[arg(long)] confirm: bool },
    List { #[arg(long)] details: bool, #[arg(long)] json: bool },
    Snap {
        alias: String,
        #[arg(long = "id")] id: Option<String>,
        #[arg(long = "msg", short = 'm')] msg: Option<String>,
        #[arg(long = "threads", short = 't')] threads: Option<usize>,
        #[arg(long = "dry-run")] dry_run: bool,
        #[arg(long = "full-hash")] full_hash: bool,
        /// Exclude files from this snap only (not permanent).
        /// Patterns use gitignore syntax. Can be repeated.
        #[arg(long = "exclude")]
        exclude: Option<Vec<String>>,
        /// Only include files matching these patterns in this snap.
        /// Other files are inherited from parent as unchanged.
        /// Patterns use gitignore syntax. Can be repeated.
        #[arg(long = "include-only")]
        include_only: Option<Vec<String>>,
    },
    Restore { alias: String, snapshot_id: String, #[arg(long = "full")] full: bool, #[arg(long = "threads", short = 't')] threads: Option<usize>, #[arg(long = "dry-run")] dry_run: bool },
    Status { alias: String, #[arg(long = "threads", short = 't')] threads: Option<usize>, #[arg(long)] json: bool, #[arg(long = "full-hash")] full_hash: bool },
    Log { alias: String, #[arg(long)] oneline: bool, #[arg(long)] json: bool, #[arg(short = 'n')] n: Option<usize> },
    Diff { alias: String, snapshot_a: String, snapshot_b: String, #[arg(long)] stat: bool, #[arg(long)] json: bool },
    Delete { alias: String, snapshot_id: String, #[arg(long = "dry-run")] dry_run: bool, #[arg(long)] force: bool },
    Branch { alias: String, #[arg(long = "create")] create: Option<String>, #[arg(long = "delete")] delete: Option<String>, #[arg(long = "switch")] switch: Option<String>, #[arg(long = "from")] from: Option<String>, #[arg(long)] force: bool, #[arg(long)] json: bool },
    Gc { alias: String, #[arg(long = "dry-run")] dry_run: bool },
    Ignore { alias: String, #[arg(long = "add")] add: Option<String>, #[arg(long = "remove")] remove: Option<String>, #[arg(long)] list: bool, #[arg(long)] edit: bool },
    /// Get or set configuration. Without alias, operates on global config.
    /// With alias, operates on per-game config.
    Config {
        /// Game alias. If omitted, operates on global config.
        alias: Option<String>,
        /// Get value for a key.
        #[arg(long)]
        get: Option<String>,
        /// Set a key=value pair. Format: --set key value
        #[arg(long, num_args = 2, value_names = ["KEY", "VALUE"])]
        set: Option<Vec<String>>,
        /// Unset a key (fall back to default/global).
        #[arg(long)]
        unset: Option<String>,
        /// List all config.
        #[arg(long)]
        list: bool,
        /// Skip confirmation prompt (for hash.algorithm changes that
        /// require rehash).
        #[arg(long)]
        yes: bool,
    },
    /// Migrate database schema to latest version.
    Migrate { alias: Option<String> },
    /// Repack game snapshots into a compressed, portable archive.
    /// Uses xtool for precompression. Output goes to
    /// [bin_dir]/repacked/[game_title]/.
    Repack {
        alias: Option<String>,
        /// Compression profile name or filename.
        /// Can be: profile name ("zstd"), filename ("zstd.gimprofile"),
        /// or full path. Use --list-profiles to see available profiles.
        /// If alias is omitted, --list-profiles is implied.
        #[arg(long)]
        profile: Option<String>,
        /// List all available compression profiles and exit.
        #[arg(long = "list-profiles")]
        list_profiles: bool,
        /// Compression level (overrides profile default).
        /// Range depends on codec:
        ///   zstd: 1-22, zlib: 1-9, lz4: 1-12, oodle: 1-8
        #[arg(long)]
        level: Option<u32>,
        /// Only repack this specific snapshot (no history).
        /// Can be repeated for multiple snapshots.
        #[arg(long = "snapshot")]
        snapshots: Option<Vec<String>>,
        /// Number of threads (default: profile setting or total - 1).
        #[arg(long, short = 't')]
        threads: Option<usize>,
        /// Memory limit in MB (default: profile setting or 80% of RAM).
        #[arg(long)]
        memory: Option<u64>,
        /// Output directory (default: [bin_dir]/repacked/[game_title]).
        #[arg(long = "output", short = 'o')]
        output: Option<PathBuf>,
        /// Dry run — show what would be packed.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },
    /// Unpack a .gim archive to a directory.
    Unpack {
        /// Path to the .gim manifest file.
        gim_file: PathBuf,
        /// Output directory.
        output_dir: PathBuf,
        /// Unpack a specific snapshot (default: HEAD/latest).
        #[arg(long = "snapshot")]
        snapshot: Option<String>,
        /// Also restore game tracking (add to gim registry).
        #[arg(long = "track")]
        track: bool,
        /// Number of threads (default: total - 1).
        #[arg(long, short = 't')]
        threads: Option<usize>,
        /// Dry run — show what would be unpacked.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },
    /// Install a .gim archive — unpack + register game + create shortcut.
    /// Same as unpack but with Windows registry registration and shortcut
    /// creation. On non-Windows, behaves like unpack.
    Install {
        /// Path to the .gim manifest file.
        gim_file: PathBuf,
        /// Output directory.
        output_dir: PathBuf,
        /// Install a specific snapshot (default: HEAD/latest).
        #[arg(long = "snapshot")]
        snapshot: Option<String>,
        /// Also restore game tracking (add to gim registry).
        #[arg(long = "track")]
        track: bool,
        /// Number of threads (default: total - 1).
        #[arg(long, short = 't')]
        threads: Option<usize>,
        /// Run interactive setup wizard.
        #[arg(long = "interactive", alias = "setup")]
        interactive: bool,
        /// Dry run — show what would be installed.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },
}
