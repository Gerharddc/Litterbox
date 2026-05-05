use clap::Parser;
use std::env::VarError;

mod agent;
mod commands;
mod daemon;
#[cfg(target_os = "linux")]
mod devices;
mod env;
mod files;
mod keys;
mod podman;
mod settings;
mod template;
mod utils;

/// Simple sandbox utility aimed at software development
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: crate::commands::Command,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Configure default log level for debug and release builds.
    if std::env::var("RUST_LOG").is_err_and(|e| e == VarError::NotPresent) {
        // SAFETY: No other threads are reading or writing to env variables.
        unsafe {
            #[cfg(debug_assertions)]
            std::env::set_var("RUST_LOG", "debug");

            #[cfg(not(debug_assertions))]
            std::env::set_var("RUST_LOG", "info");
        }
    }

    env_logger::init();
    args.command.run()
}
