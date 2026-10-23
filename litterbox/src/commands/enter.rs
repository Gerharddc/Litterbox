use crate::{
    daemon, files,
    podman::{
        commit_container, create_container, get_container, get_image, is_container_running,
        start_daemon, validate_container_prerequisites, wait_for_podman, wait_for_podman_async,
    },
    settings::LitterboxSettings,
    utils::{podman_name, trace_arguments},
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

        let mut container = match get_container(&self.name)? {
            Some(container) => container,
            None => {
                // A previous re-creation may have failed after removing the old
                // container. Recreate it from the image and saved settings so
                // that entering still works.
                let image_name = get_image(&self.name)?
                    .and_then(|image| image.names.into_iter().next())
                    .ok_or_else(|| {
                        anyhow!(
                            "No container found for '{}'. Run `litterbox build {}` first.",
                            self.name,
                            self.name
                        )
                    })?;
                let settings = LitterboxSettings::load(&self.name)?.ok_or_else(|| {
                    anyhow!(
                        "No container found for '{}' and no settings file to recreate it from. \
                         Run `litterbox build {}` first.",
                        self.name,
                        self.name
                    )
                })?;

                info!(
                    "No container found for '{}'; recreating it from the existing image...",
                    self.name
                );
                create_container(&self.name, &podman_name(&self.name), &image_name, &settings)?;

                get_container(&self.name)?
                    .ok_or_else(|| anyhow!("Failed to recreate container for '{}'", self.name))?
            }
        };

        // Container configuration (network mode, capabilities, etc.) is baked
        // into the podman container at creation time. Reload the settings on
        // every enter so that edits to the RON file take effect without having
        // to run `litterbox build` again.
        let settings = LitterboxSettings::load(&self.name)?;

        if let Some(settings) = settings.as_ref()
            && container.labels.settings.as_deref() != Some(settings.fingerprint().as_str())
        {
            if is_container_running(&self.name)? {
                warn!(
                    "The settings for '{}' changed, but the container is running. The new \
                     settings will be applied the next time the container is stopped.",
                    self.name
                );
            } else {
                // Check before committing so that a missing build artifact does
                // not retag the image for nothing.
                validate_container_prerequisites()?;

                let container_name = container
                    .names
                    .first()
                    .cloned()
                    .ok_or_else(|| anyhow!("Container for '{}' has no name", self.name))?;
                let image_name = get_image(&self.name)?
                    .and_then(|image| image.names.into_iter().next())
                    .ok_or_else(|| anyhow!("No image found for '{}'", self.name))?;

                info!(
                    "Settings for '{}' changed; recreating container...",
                    self.name
                );

                // Preserve changes made inside the container (outside the
                // bind-mounted home directory) before the container is
                // replaced.
                commit_container(&container.id, &image_name)?;

                create_container(&self.name, &container_name, &image_name, settings)?;

                container = get_container(&self.name)?
                    .ok_or_else(|| anyhow!("No container found for '{}'", self.name))?;

                info!("Container recreated with the new settings.");
            }
        }

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
