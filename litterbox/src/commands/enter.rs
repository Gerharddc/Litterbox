use crate::{
    daemon, files,
    podman::{
        get_container, is_container_running, start_daemon, wait_for_podman, wait_for_podman_async,
    },
    utils::trace_arguments,
};
use anyhow::{Context as _, Result, anyhow};
use clap::Args;
use log::{debug, info, warn};
use nix::unistd::{Pid, getgid, getuid};
use shared::entrypoint::CommonEntrypointOptions;
use std::{
    fmt::Display,
    path::PathBuf,
    process::Stdio,
    str::{FromStr, ParseBoolError},
};

#[derive(Clone, Debug, Copy)]
pub struct Tty(pub bool);

impl Display for Tty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Tty {
    type Err = ParseBoolError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

#[derive(Clone, Debug, Copy)]
pub struct Interactive(pub bool);

impl Display for Interactive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Interactive {
    type Err = ParseBoolError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

/// Enter an existing Litterbox
#[derive(Args, Debug)]
pub struct Command {
    /// The name of the Litterbox to enter
    name: String,

    /// Make STDIN available to the contained process. Defaults to "true" if
    /// COMMAND is not supplied
    #[arg(long, short, default_value_t = Interactive(false))]
    interactive: Interactive,

    /// Allocate a pseudo-TTY. Defaults to "true" if COMMAND is not supplied
    #[arg(long, short, default_value_t = Tty(false))]
    tty: Tty,

    /// Working directory inside the container
    #[arg(long, short)]
    workdir: Option<PathBuf>,

    #[clap(flatten)]
    opts: CommonEntrypointOptions,
}

impl Command {
    pub fn run(self) -> Result<()> {
        use std::process::Command;

        let container = get_container(&self.name)?
            .ok_or_else(|| anyhow!("No container found for '{}'", self.name))?;
        let container_id = container.id;

        if !daemon::is_running(&self.name)? {
            if is_container_running(&self.name)? {
                warn!("Daemon was not running but container was. Restarting daemon...");
            }

            start_daemon(&self.name)?;
        }

        let my_pid = Pid::this();
        let session_lock = files::session_lock_path(&self.name)?;
        files::append_pid_to_session_lockfile(&session_lock, my_pid)?;

        if !is_container_running(&self.name)? {
            info!("Container is not running yet; starting now...");

            let mut cmd = Command::new("podman");
            cmd.stdout(Stdio::null());
            cmd.args(["start", &container_id]);
            trace_arguments(&cmd);

            let start_child = cmd.spawn().context("Failed to run podman command")?;
            wait_for_podman(start_child)?;
        } else {
            debug!("Container {container_id:?} is already running; just attaching...")
        }

        tokio::runtime::Runtime::new()
            .expect("Tokio runtime should start")
            .block_on(container_exec_entrypoint(
                container_id,
                self.interactive,
                self.tty,
                self.workdir,
                self.opts,
            ))?;

        files::remove_pid_from_session_lockfile(&session_lock, my_pid)?;

        Ok(())
    }
}

async fn container_exec_entrypoint(
    container_id: String,
    interactive: Interactive,
    tty: Tty,
    workdir: Option<PathBuf>,
    opts: CommonEntrypointOptions,
) -> Result<()> {
    use tokio::process::Command;

    let mut exec_child = Command::new("podman");

    exec_child.arg("exec");

    // Assume -t if we are launching the login shell
    if tty.0 || opts.command.is_none() {
        exec_child.arg("--tty");
    }

    // Assume -i if we are launching the login shell
    if interactive.0 || opts.command.is_none() {
        exec_child.arg("--interactive");
    }

    if let Some(workdir) = workdir {
        exec_child.arg("--workdir");
        exec_child.arg(workdir.into_os_string());
    }

    // We always start as root but drop permissions later if needed
    exec_child.arg("--user");
    exec_child.arg("root");

    exec_child.args([
        &container_id,
        "/lbx-init",
        "entrypoint",
        "--uid",
        &getuid().to_string(),
        "--gid",
        &getgid().to_string(),
        "--wait",
        &opts.wait.to_string(),
    ]);

    // The entrypoint is responsible for dropping root if needed
    if opts.root {
        exec_child.arg("--root");
    }

    if let Some(command) = opts.command {
        exec_child.arg("--");
        exec_child.arg(command);
        exec_child.args(opts.args);
    }

    let mut exec_child = exec_child.spawn().context("Failed to run podman command")?;
    debug!("Entering Litterbox...");

    tokio::select! {
        _ = wait_for_podman_async(&mut exec_child) => {}
        _ = tokio::signal::ctrl_c() => {
            let _ = exec_child.kill().await;
        }
    }

    debug!("Exited Litterbox");

    Ok(())
}
