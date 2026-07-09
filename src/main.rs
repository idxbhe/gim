use clap::Parser;
use gim::cli::Cli;
use gim::commands;

fn main() {
    let cli = Cli::parse();
    let ll = match cli.verbose { 0 => "warn", 1 => "info", 2 => "debug", _ => "trace" };
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(ll)).format_timestamp(None).format_level(false).try_init();
    if cli.no_color { colored::control::set_override(false); }
    match commands::run(cli.command, cli.no_progress) {
        Ok(()) => {}
        Err(e) => { commands::print_error(&e); std::process::exit(commands::err_exit_code(&e)); }
    }
}
