//! Command implementations.

pub mod add;
pub mod branch;
pub mod delete;
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

pub fn run(cmd: Command) -> GResult<()> {
    // Configure global thread pool based on the first --threads flag
    // we encounter. This avoids per-call pool creation.
    if let Some(n) = threads_from_command(&cmd) {
        crate::parallel::configure(n);
    }

    let colorizer = output::default_colorizer();
    match cmd {
        Command::Add { alias, game_dir, title, data_dir } => add::run(&colorizer, alias, game_dir, title, data_dir),
        Command::Remove { alias, confirm } => remove::run(&colorizer, alias, confirm),
        Command::List { details, json } => list::run(&colorizer, details, json),
        Command::Snap { alias, id, msg, threads, dry_run, full_hash } => snap::run(&colorizer, alias, id, msg, threads, dry_run, full_hash),
        Command::Restore { alias, snapshot_id, full, threads, dry_run } => restore::run(&colorizer, alias, snapshot_id, full, threads, dry_run),
        Command::Status { alias, threads, json, full_hash } => status::run(&colorizer, alias, threads, json, full_hash),
        Command::Log { alias, oneline, json, n } => log_cmd::run(&colorizer, alias, oneline, json, n),
        Command::Diff { alias, snapshot_a, snapshot_b, stat, json } => diff::run(&colorizer, alias, snapshot_a, snapshot_b, stat, json),
        Command::Delete { alias, snapshot_id, dry_run, force } => delete::run(&colorizer, alias, snapshot_id, dry_run, force),
        Command::Branch { alias, create, delete, switch, from, force, json } => branch::run(&colorizer, alias, create, delete, switch, from, force, json),
        Command::Gc { alias, dry_run } => gc::run(&colorizer, alias, dry_run),
        Command::Ignore { alias, add, remove, list, edit } => ignore_cmd::run(&colorizer, alias, add, remove, list, edit),
    }
}

/// Extract the `--threads` value from any subcommand that supports it.
fn threads_from_command(cmd: &Command) -> Option<usize> {
    match cmd {
        Command::Snap { threads, .. } => *threads,
        Command::Restore { threads, .. } => *threads,
        Command::Status { threads, .. } => *threads,
        _ => None,
    }
}

pub fn print_error(err: &GError) { eprintln!("error: {}", err); }
pub fn err_exit_code(err: &GError) -> i32 { exit_code(err) }
