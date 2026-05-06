use clap::Parser;
use std::env as std_env;

mod commands;
mod files;
mod sandbox;
mod utils;

/// Litterbox container entrypoint (for internal use)
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: commands::Command,
}

fn main() -> anyhow::Result<()> {
    let arg0 = std::env::args().next();

    if let Some(arg0) = arg0.as_deref()
        && utils::SU_BINARIES.contains(&arg0)
    {
        eprintln!(
            "{arg0:?} is not supported inside this session. Use 'litterbox enter --root NAME' to enter as root."
        );

        std::process::exit(1);
    }

    let args = Args::parse();

    // Configure default log level for debug and release builds.
    if std_env::var("RUST_LOG").is_err() {
        let default_log_level = if cfg!(debug_assertions) {
            "debug"
        } else {
            "info"
        };

        // SAFETY: No other threads are reading or writing to env variables.
        unsafe {
            std_env::set_var("RUST_LOG", default_log_level);
        }
    }

    env_logger::init();
    commands::run(args.command)
}
