use clap::Subcommand;

mod build;
mod confirm;
mod daemon;
mod define;
mod delete;
#[cfg(target_os = "linux")]
mod device;
mod enter;
mod keys;
mod list;

#[derive(Subcommand, Debug)]
pub enum Command {
    Build(#[clap(flatten)] build::Command),

    #[clap(visible_alias("def"))]
    Define(#[clap(flatten)] define::Command),

    #[clap(visible_alias("del"), visible_alias("rm"))]
    Delete(#[clap(flatten)] delete::Command),

    #[cfg(target_os = "linux")]
    #[clap(visible_alias("dev"))]
    Device(#[clap(flatten)] device::Command),

    Enter(#[clap(flatten)] enter::Command),

    #[clap(visible_alias("ls"))]
    List(#[clap(flatten)] list::Command),

    #[command(subcommand)]
    Keys(keys::Command),

    #[clap(hide = true)]
    Confirm(#[clap(flatten)] confirm::Command),

    #[clap(hide = true)]
    Daemon(#[clap(flatten)] daemon::Command),

}

impl Command {
    pub fn run(self) -> anyhow::Result<()> {
        match self {
            Command::Define(command) => command.run(),
            Command::Build(command) => command.run(),
            Command::List(command) => command.run(),
            Command::Enter(command) => command.run(),
            Command::Delete(command) => command.run(),
            Command::Keys(command) => command.run(),
            #[cfg(target_os = "linux")]
            Command::Device(command) => command.run(),
            Command::Confirm(command) => command.run(),
            Command::Daemon(command) => command.run(),
        }
    }
}
