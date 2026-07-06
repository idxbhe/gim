//! Command implementations.
//!
//! Each command lives in its own module so that the codebase scales
//! cleanly: adding a new command only touches `commands/mod.rs` (one
//! new `pub mod` line + one new match arm in `dispatch`).

pub mod add;
pub mod diff;
pub mod gc;
pub mod ignore_cmd;
pub mod list;
pub mod log_cmd;
pub mod remove;
pub mod restore;
pub mod snap;
pub mod status;

use crate::cli::Command;
use crate::error::{exit_code, GError, GResult};
use crate::output;

/// Dispatch a parsed CLI command to its implementation.
///
/// All commands share the same error-handling path: any `Err` returned
/// is converted to a user-facing message and a non-zero exit code by
/// the caller in `main.rs`.
pub fn run(cmd: Command) -> GResult<()> {
    let colorizer = output::default_colorizer();
    match cmd {
        Command::Add {
            alias,
            game_dir,
            title,
            data_dir,
        } => add::run(&colorizer, alias, game_dir, title, data_dir),
        Command::Remove { alias, confirm } => remove::run(&colorizer, alias, confirm),
        Command::List {
            details,
            json,
        } => list::run(&colorizer, details, json),
        Command::Snap {
            alias,
            id,
            msg,
            threads,
            dry_run,
        } => snap::run(&colorizer, alias, id, msg, threads, dry_run),
        Command::Restore {
            alias,
            snapshot_id,
            full,
            threads,
            dry_run,
        } => restore::run(&colorizer, alias, snapshot_id, full, threads, dry_run),
        Command::Status {
            alias,
            threads,
            json,
        } => status::run(&colorizer, alias, threads, json),
        Command::Log {
            alias,
            oneline,
            json,
            n,
        } => log_cmd::run(&colorizer, alias, oneline, json, n),
        Command::Diff {
            alias,
            snapshot_a,
            snapshot_b,
            stat,
            json,
        } => diff::run(&colorizer, alias, snapshot_a, snapshot_b, stat, json),
        Command::Gc { alias, dry_run } => gc::run(&colorizer, alias, dry_run),
        Command::Ignore {
            alias,
            add,
            remove,
            list,
            edit,
        } => ignore_cmd::run(&colorizer, alias, add, remove, list, edit),
    }
}

/// Print an error to stderr in a consistent format.
pub fn print_error(err: &GError) {
    eprintln!("error: {}", err);
}

/// Map a `GError` to a process exit code.
pub fn err_exit_code(err: &GError) -> i32 {
    exit_code(err)
}
