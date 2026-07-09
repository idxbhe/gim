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
use crate::output::{self, ProgressReporter};
use std::io::IsTerminal;

pub fn run(cmd: Command, no_progress: bool) -> GResult<()> {
    if let Some(n) = threads_from_command(&cmd) { crate::parallel::configure(n); }
    let colorizer = output::default_colorizer();
    let progress = build_progress_reporter(no_progress);
    dispatch(&cmd, &colorizer, &progress)
}

/// Build a ProgressReporter. Disabled when:
/// - `--no-progress` was passed
/// - `GIM_NO_PROGRESS` env var is set
/// - stderr is not a TTY (piped to file/other command)
fn build_progress_reporter(no_progress: bool) -> ProgressReporter {
    let enabled = !no_progress
        && std::env::var_os("GIM_NO_PROGRESS").is_none()
        && std::io::stderr().is_terminal();
    ProgressReporter::new(enabled)
}

fn dispatch(cmd: &Command, c: &output::Colorizer, p: &ProgressReporter) -> GResult<()> {
    // We need owned values for some commands. Clone the needed bits.
    match cmd {
        Command::Add { alias, game_dir, title, data_dir } => add::run(c, alias.clone(), game_dir.clone(), title.clone(), data_dir.clone()),
        Command::Remove { alias, confirm } => remove::run(c, alias.clone(), *confirm),
        Command::List { details, json } => list::run(c, *details, *json),
        Command::Snap { alias, id, msg, threads, dry_run, full_hash } => snap::run(c, alias.clone(), id.clone(), msg.clone(), *threads, *dry_run, *full_hash, p),
        Command::Restore { alias, snapshot_id, full, threads, dry_run } => restore::run(c, alias.clone(), snapshot_id.clone(), *full, *threads, *dry_run, p),
        Command::Status { alias, threads, json, full_hash } => status::run(c, alias.clone(), *threads, *json, *full_hash, p),
        Command::Log { alias, oneline, json, n } => log_cmd::run(c, alias.clone(), *oneline, *json, *n),
        Command::Diff { alias, snapshot_a, snapshot_b, stat, json } => diff::run(c, alias.clone(), snapshot_a.clone(), snapshot_b.clone(), *stat, *json),
        Command::Delete { alias, snapshot_id, dry_run, force } => delete::run(c, alias.clone(), snapshot_id.clone(), *dry_run, *force),
        Command::Branch { alias, create, delete, switch, from, force, json } => branch::run(c, alias.clone(), create.clone(), delete.clone(), switch.clone(), from.clone(), *force, *json, p),
        Command::Gc { alias, dry_run } => gc::run(c, alias.clone(), *dry_run, p),
        Command::Ignore { alias, add, remove, list, edit } => ignore_cmd::run(c, alias.clone(), add.clone(), remove.clone(), *list, *edit),
    }
}

fn threads_from_command(cmd: &Command) -> Option<usize> {
    match cmd {
        Command::Snap { threads, .. } | Command::Restore { threads, .. } | Command::Status { threads, .. } => *threads,
        _ => None,
    }
}

pub fn print_error(err: &GError) { eprintln!("error: {}", err); }
pub fn err_exit_code(err: &GError) -> i32 { exit_code(err) }
