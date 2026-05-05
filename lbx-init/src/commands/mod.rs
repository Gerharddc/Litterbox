use clap::Subcommand;

mod entrypoint;
mod wait;

#[derive(Subcommand, Debug)]
pub enum Command {
    Entrypoint(entrypoint::Command),
    Wait(wait::Command),
}

pub fn run(command: Command) -> anyhow::Result<()> {
    match command {
        Command::Entrypoint(cmd) => cmd.run(),
        Command::Wait(cmd) => cmd.run(),
    }
}
