use crate::{
    daemon, files, mounts,
    podman::{
        Container, commit_container, create_container, get_container, get_image,
        is_container_running, start_daemon, validate_container_prerequisites, wait_for_podman,
        wait_for_podman_async,
    },
    settings::LitterboxSettings,
    utils::{podman_name, trace_arguments},
};
use anyhow::{Context as _, Result, anyhow, ensure};
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

    /// Mount a host directory into the Litterbox at ~/shared/NAME, defaulting
    /// to the current directory. May be repeated. Requires root privileges on
    /// the host.
    #[arg(
        long,
        value_name = "PATH",
        num_args = 0..=1,
        default_missing_value = ".",
        action = clap::ArgAction::Append
    )]
    mount: Vec<PathBuf>,

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

        let requested_mounts = mounts::resolve(&self.mount)?;
        let shared_host_dir = mounts::shared_dir(&self.name)?;

        let propagation_ready =
            !requested_mounts.is_empty() && mounts::propagation_ready(&shared_host_dir)?;

        let shared_ready = container.labels.shared.as_deref() == Some(mounts::SHARED_MOUNT_LABEL);
        let needs_shared_setup =
            !requested_mounts.is_empty() && (!shared_ready || !propagation_ready);

        if needs_shared_setup {
            ensure!(
                !is_container_running(&self.name)?,
                "This Litterbox does not currently have working dynamic mount support. Exit all \
                 of its sessions and run `litterbox enter {} --mount` again to recreate it.",
                self.name
            );

            mounts::ensure_propagation(&shared_host_dir)?;
        }

        // Container configuration (network mode, capabilities, etc.) is baked
        // into the podman container at creation time. Reload the settings on
        // every enter so that edits to the RON file take effect without having
        // to run `litterbox build` again.
        let settings = LitterboxSettings::load(&self.name)?;
        let mut recreated = false;

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
                info!(
                    "Settings for '{}' changed; recreating container...",
                    self.name
                );

                container = recreate_container(&self.name, &container, settings)?;
                recreated = true;

                info!("Container recreated with the new settings.");
            }
        }

        if needs_shared_setup && !recreated {
            let settings = settings.as_ref().ok_or_else(|| {
                anyhow!(
                    "Cannot recreate '{0}' to enable dynamic mounts because its settings file is \
                     missing. Run `litterbox build {0}` first.",
                    self.name
                )
            })?;

            info!("Recreating '{}' to enable dynamic mounts...", self.name);

            container = recreate_container(&self.name, &container, settings)?;

            info!("Container recreated with dynamic mount support.");
        }

        let shared_configured = container.labels.shared.is_some();
        let supports_dynamic_mounts =
            container.labels.shared.as_deref() == Some(mounts::SHARED_MOUNT_LABEL);
        ensure!(
            requested_mounts.is_empty() || supports_dynamic_mounts,
            "Internal error: dynamic mount support was not enabled for '{}'",
            self.name
        );

        let container_id = container.id.clone();
        let session_lock = files::session_lock_path(&self.name)?;
        let my_pid = Pid::this();

        // Serialise session registration and mount changes. This prevents two
        // concurrent `enter` calls from removing each other's mounts while the
        // container is stopped.
        let mount_lock = mounts::lock(&self.name)?;
        files::cleanup_dead_pids_from_session_lockfile(&session_lock)?;

        let other_sessions = !files::read_pids_from_session_lockfile(&session_lock)?.is_empty();
        let container_running = is_container_running(&self.name)?;

        // A stopped container gets a clean mount set. While the container is
        // running, mounts are shared by all of its sessions and are kept.
        if !container_running && !other_sessions {
            mounts::cleanup_stale(&self.name)?;
        }

        files::append_pid_to_session_lockfile(&session_lock, my_pid)?;

        if !daemon::is_running(&self.name)? {
            if is_container_running(&self.name)? {
                warn!("Daemon was not running but container was. Restarting daemon...");
            }

            start_daemon(&self.name)?;
        }

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

        // Mount the projects after the container has started so that the
        // mounts propagate into its mount namespace. Landlock rules are added
        // by the entrypoint below, once the paths are visible in the
        // container.
        let mut allow_paths = if supports_dynamic_mounts {
            mounts::apply(&self.name, &requested_mounts)?
        } else {
            Vec::new()
        };

        if shared_configured || !requested_mounts.is_empty() {
            allow_paths.insert(0, PathBuf::from(mounts::CONTAINER_SHARED_DIR));
        }

        drop(mount_lock);

        let workdir = self.workdir.or_else(|| {
            requested_mounts
                .first()
                .map(|mount| mount.container_path.clone())
        });

        tokio::runtime::Runtime::new()
            .expect("Tokio runtime should start")
            .block_on(container_exec_entrypoint(
                container_id,
                self.interactive,
                self.tty,
                workdir,
                allow_paths,
                self.opts,
            ))?;

        files::remove_pid_from_session_lockfile(&session_lock, my_pid)?;

        Ok(())
    }
}

fn recreate_container(
    lbx_name: &str,
    container: &Container,
    settings: &LitterboxSettings,
) -> Result<Container> {
    let container_name = container
        .names
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("Container for '{lbx_name}' has no name"))?;
    let image_name = get_image(lbx_name)?
        .and_then(|image| image.names.into_iter().next())
        .ok_or_else(|| anyhow!("No image found for '{lbx_name}'"))?;

    // Check before committing so that a missing build artifact does not retag
    // the image for nothing.
    validate_container_prerequisites()?;

    // Preserve changes made inside the container (outside the bind-mounted
    // home directory) before the container is replaced.
    commit_container(&container.id, &image_name)?;

    create_container(lbx_name, &container_name, &image_name, settings)?;

    get_container(lbx_name)?.ok_or_else(|| anyhow!("No container found for '{lbx_name}'"))
}

async fn container_exec_entrypoint(
    container_id: String,
    interactive: Interactive,
    tty: Tty,
    workdir: Option<PathBuf>,
    allow_paths: Vec<PathBuf>,
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

    // Mounted project directories live outside the container's home mount, so
    // Landlock needs an explicit rule for each one.
    for path in allow_paths {
        exec_child.arg("--allow").arg(path);
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::FromArgMatches;

    fn parse(args: &[&str]) -> Command {
        let mut command = clap::Command::new("enter");
        command = <Command as clap::Args>::augment_args(command);
        let matches = command
            .try_get_matches_from(args)
            .expect("arguments should parse");

        Command::from_arg_matches(&matches).expect("arguments should convert")
    }

    #[test]
    fn mount_defaults_to_current_directory() {
        let command = parse(&["enter", "dev", "--mount"]);

        assert_eq!(command.mount, vec![PathBuf::from(".")]);
    }

    #[test]
    fn mount_followed_by_an_option_uses_the_default() {
        let command = parse(&["enter", "dev", "--mount", "--root"]);

        assert_eq!(command.mount, vec![PathBuf::from(".")]);
        assert!(command.opts.root);
    }

    #[test]
    fn mount_accepts_values_and_repeats() {
        let command = parse(&[
            "enter",
            "dev",
            "--mount",
            "/host/one",
            "--mount",
            "/host/two",
        ]);

        assert_eq!(
            command.mount,
            vec![PathBuf::from("/host/one"), PathBuf::from("/host/two")]
        );
    }

    #[test]
    fn mount_is_optional() {
        let command = parse(&["enter", "dev"]);

        assert!(command.mount.is_empty());
    }
}
