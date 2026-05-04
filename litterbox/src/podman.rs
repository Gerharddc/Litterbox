use anyhow::{Context, Result, anyhow, bail, ensure};
use inquire::Confirm;
use log::info;
use log::{debug, warn};
use nix::unistd::{getgid, getuid};
use serde::Deserialize;
use std::{
    ffi::OsString,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

use crate::{
    env,
    files::{self, SshSockFile},
    keys::Keys,
    settings::LitterboxSettings,
    utils::{extract_stdout, podman_name, trace_arguments},
};
use crate::{
    files::{dockerfile_path, write_file},
    template::Template,
};

const LBX_USER: &str = "user";

/// Represents the GPU device configuration for the container
enum GpuDevice {
    /// Standard Linux GPU device at /dev/dri
    Dri,
    /// WSL2 DirectX device at /dev/dxg
    Dxg,
}

impl GpuDevice {
    /// Detects the available GPU device based on what exists on the system
    fn try_detect() -> Option<Self> {
        if Path::new("/dev/dri").exists() {
            debug!("/dev/dri available");
            Some(Self::Dri)
        } else if Path::new("/dev/dxg").exists() {
            debug!("/dev/dxg available (WSL)");
            Some(Self::Dxg)
        } else {
            None
        }
    }

    fn device_path(&self) -> &'static str {
        match self {
            GpuDevice::Dri => "/dev/dri",
            GpuDevice::Dxg => "/dev/dxg",
        }
    }

    fn volume_mount(&self) -> &'static str {
        match self {
            GpuDevice::Dri => "/dev/dri:/dev/dri",
            GpuDevice::Dxg => "/dev/dxg:/dev/dxg",
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct ContainerLabels {
    #[serde(rename = "work.litterbox.name")]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
// https://github.com/containers/podman/blob/0eeff22607b98f312b655f01a4f29b5da2553330/libpod/define/containerstate.go#L39-L70
pub enum ContainerState {
    Created,
    Initialized,
    Running,
    Stopped,
    Paused,
    Exited,
    Removing,
    Stopping,
    Unknown,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Container {
    #[serde(rename = "Id")]
    pub id: String,

    #[serde(rename = "Image")]
    pub image: String,

    #[serde(rename = "ImageID")]
    pub image_id: String,

    #[serde(rename = "Names")]
    pub names: Vec<String>,

    #[serde(rename = "Labels")]
    pub labels: ContainerLabels,

    #[serde(rename = "State")]
    pub state: ContainerState,
}

#[derive(Deserialize, Debug)]
pub struct Containers(pub Vec<Container>);

#[derive(Deserialize, Debug, Clone)]
pub struct Image {
    #[serde(rename = "Id")]
    pub id: String,

    #[serde(rename = "Names")]
    pub names: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct Images(Vec<Image>);

pub fn get_containers() -> Result<Containers> {
    let mut cmd = Command::new("podman");
    cmd.args([
        "ps",
        "--all",
        "--format",
        "json",
        "--filter",
        "label=work.litterbox.name",
    ]);
    trace_arguments(&cmd);
    let output = cmd.output().context("Failed to run 'podman' command")?;

    let stdout = extract_stdout(&output)?;
    Ok(serde_json::from_str(stdout)?)
}

pub fn get_containers_by_name(lbx_name: &str) -> Result<Containers> {
    let mut cmd = Command::new("podman");
    cmd.args([
        "ps",
        "--all",
        "--format",
        "json",
        "--filter",
        &format!("label=work.litterbox.name={lbx_name}"),
    ]);
    trace_arguments(&cmd);
    let output = cmd.output().context("Failed to run podman command")?;

    Ok(serde_json::from_str(extract_stdout(&output)?)?)
}

pub fn get_container(lbx_name: &str) -> Result<Option<Container>> {
    let mut containers = get_containers_by_name(lbx_name)?.0;

    match containers.len() {
        0 => Ok(None),
        1 => Ok(Some(containers.swap_remove(0))),
        _ => bail!("Multiple containers found for \"{lbx_name}\""),
    }
}

pub fn is_container_running(lbx_name: &str) -> Result<bool> {
    let containers = get_containers_by_name(lbx_name)?.0;

    Ok(containers
        .first()
        .is_some_and(|c| c.state == ContainerState::Running))
}

pub fn get_image(lbx_name: &str) -> Result<Option<Image>> {
    let mut cmd = Command::new("podman");
    cmd.args([
        "image",
        "ls",
        "--all",
        "--format",
        "json",
        "--filter",
        &format!("label=work.litterbox.name={lbx_name}"),
        "--filter",
        // Avoid dangling images that are left behind when an image gets
        // rebuilt.
        "dangling=false",
    ]);
    trace_arguments(&cmd);
    let output = cmd.output().context("Failed to run podman command")?;

    let stdout = extract_stdout(&output)?;
    let Images(mut images) = serde_json::from_str(stdout)?;

    match images.len() {
        0 => Ok(None),
        1 => Ok(Some(images.swap_remove(0))),
        _ => bail!("Multiple images found for \"{lbx_name}\""),
    }
}

pub fn define_litterbox(lbx_name: &str) -> anyhow::Result<()> {
    let dockerfile = dockerfile_path(lbx_name)?;

    if dockerfile.exists() {
        bail!("Dockerfile already exists at {dockerfile:?}");
    }

    let template = Template::select("Choose a template:").prompt()?;

    write_file(dockerfile.as_path(), template.contents())?;
    info!("Default Dockerfile written to {dockerfile:?}");

    Ok(())
}

pub fn build_image(lbx_name: &str) -> Result<()> {
    let image_name = match get_image(lbx_name)? {
        Some(details) => {
            assert!(!details.names.is_empty(), "All images should have a name.");
            if details.names.len() > 1 {
                warn!("Image for Litterbox had more than one name. The first one will be used.");
            }

            eprintln!("An image for this Litterbox already exists.");
            if Confirm::new("Would you like to rebuild the image?")
                .with_default(true)
                .prompt()?
            {
                eprintln!("The image will now be rebuilt!");
            } else {
                eprintln!("The existing image will be re-used!");

                // Exit the whole function since we don't need to do anything more
                return Ok(());
            }
            details.names[0].clone()
        }

        None => podman_name(lbx_name),
    };

    let dockerfile_path = files::dockerfile_path(lbx_name)?;

    if !dockerfile_path.exists() {
        info!("{dockerfile_path:?} does not exist.");
        // Ask the user right away for convenience. They can always CTRL + C
        define_litterbox(lbx_name)?;
    }

    let mut cmd = Command::new("podman");
    cmd.args([
        "build",
        "--build-arg",
        &format!("USER={}", LBX_USER),
        "--build-arg",
        &format!("UID={}", getuid().as_raw()),
        "--build-arg",
        &format!("GID={}", getgid().as_raw()),
        "--tag",
        &image_name,
        "--label",
        &format!("work.litterbox.name={lbx_name}"),
        "--file",
        dockerfile_path.to_str().expect("Invalid dockerfile_path."),
    ]);
    trace_arguments(&cmd);
    let child = cmd.spawn().context("Failed to run podman command")?;

    wait_for_podman(child)?;
    info!("Built image named {image_name}.");
    Ok(())
}

pub fn build_litterbox(lbx_name: &str) -> Result<()> {
    let image_details = get_image(lbx_name)?
        .ok_or_else(|| anyhow!("No image found for '{lbx_name}'. Run `litterbox build` first."))?;
    let image_id = image_details.id;
    let container_name = match get_container(lbx_name)? {
        Some(mut details) => {
            assert!(
                !details.names.is_empty(),
                "All containers should have a name."
            );

            if details.names.len() > 1 {
                warn!("Litterbox container has more than one name. The first one will be used.");
            }

            eprintln!("A container for this Litterbox already exists.");

            if Confirm::new("Would you like to replace this container?")
                .with_default(true)
                .prompt()?
            {
                details.names.swap_remove(0)
            } else {
                return Err(anyhow!("Cannot build without replacing container"));
            }
        }

        None => podman_name(lbx_name),
    };

    // --userns=keep-id is used, so this is fine to be used in the container.
    let uid = getuid();

    let rt_dir = PathBuf::from(&format!("/run/user/{uid}"));
    let wayland = match (
        std::env::var("WAYLAND_DISPLAY"),
        std::env::var("XDG_RUNTIME_DIR"),
    ) {
        (Ok(display), Ok(host_rt_dir)) => {
            let host_rt_dir = PathBuf::from(host_rt_dir);
            let socket_path = host_rt_dir.join(&display);

            if socket_path.exists() {
                Some((display, host_rt_dir))
            } else {
                warn!(
                    "Wayland socket '{}' does not exist; GUI forwarding will be disabled",
                    socket_path.display()
                );
                None
            }
        }
        _ => {
            warn!(
                "WAYLAND_DISPLAY/XDG_RUNTIME_DIR not set; GUI forwarding will be disabled"
            );
            None
        }
    };

    let lbx_home_path = files::lbx_home_path(lbx_name)?;
    fs::create_dir_all(&lbx_home_path).context("Failed to create litterbox home directory")?;

    let ssh_sock = SshSockFile::new(lbx_name, true)?;
    let settings = LitterboxSettings::load_or_prompt(lbx_name)?;

    let session_lock_file_path = files::session_lock_path(lbx_name)?;

    if let Some(parent) = session_lock_file_path.parent() {
        fs::create_dir_all(parent)
            .context("Failed to create session lock file parent directory")?;
    }

    fs::File::create(&session_lock_file_path).context("Failed to create session lock file")?;

    let mut cmd = Command::new("podman");
    cmd.arg("create");

    cmd.arg("--replace");
    cmd.args(["--entrypoint", "[\"/lbx-init\", \"wait\"]"]);
    cmd.args(["--env", &format!("HOME=/home/{LBX_USER}")]);
    // Allow user to specify RUST_LOG to litterbox internal commands. Useful for
    // development and for debugging.
    cmd.args(["--env", "RUST_LOG"]);
    cmd.args([
        "--env",
        &format!("SSH_AUTH_SOCK={}/ssh-agent.sock", rt_dir.to_string_lossy()),
    ]);
    cmd.args([
        "--env",
        &format!("XDG_RUNTIME_DIR={}", rt_dir.to_string_lossy()),
    ]);
    if let Some((wayland_display, _)) = &wayland {
        cmd.args(["--env", "XDG_SESSION_TYPE=wayland"]);
        cmd.args(["--env", &format!("WAYLAND_DISPLAY={wayland_display}")]);
    }
    cmd.args(["--hostname", &format!("lbx-{lbx_name}")]);
    cmd.args(["--label", &format!("work.litterbox.name={lbx_name}")]);
    cmd.args(["--name", &container_name]);
    cmd.args(["--network", settings.network_mode.podman_args()]);
    cmd.args(["--security-opt", "label=disable"]);
    cmd.args(["--userns", "keep-id"]);

    // The `wait` command uses it to know when it can exit.
    let mut session_lock_mount = session_lock_file_path.into_os_string();
    session_lock_mount.push(":/session.lock:ro");

    cmd.arg("--volume");
    cmd.arg(session_lock_mount);

    let mut entrypoint_bin_mount = env::lbx_init_binary_path().into_os_string();
    entrypoint_bin_mount.push(":/lbx-init:ro");

    cmd.arg("--volume");
    cmd.arg(entrypoint_bin_mount);

    let mut ssh_sock_mount = ssh_sock.path().as_os_str().to_owned();
    ssh_sock_mount.push(":");
    ssh_sock_mount.push(&rt_dir);
    ssh_sock_mount.push("/ssh-agent.sock");

    cmd.arg("--volume");
    cmd.arg(ssh_sock_mount);

    if let Some((wayland_display, host_rt_dir)) = &wayland {
        let mut wayland_display_mount = OsString::from(host_rt_dir);
        wayland_display_mount.push("/");
        wayland_display_mount.push(wayland_display);
        wayland_display_mount.push(":");
        wayland_display_mount.push(&rt_dir);
        wayland_display_mount.push("/");
        wayland_display_mount.push(wayland_display);

        cmd.arg("--volume");
        cmd.arg(wayland_display_mount);
    }

    let mut home_mount = lbx_home_path.into_os_string();
    home_mount.push(":/home/");
    home_mount.push(LBX_USER);

    cmd.arg("--volume");
    cmd.arg(home_mount);

    match GpuDevice::try_detect() {
        Some(dev) => {
            debug!("Appending GPU device args for '{}'", dev.device_path());
            cmd.args(["--volume", dev.volume_mount()]);
            cmd.args(["--device", dev.device_path()]);
        }

        None => {
            warn!("No GPU device found! GPU acceleration will not be available in the Litterbox.")
        }
    }

    if settings.expose_pipewire {
        match files::pipewire_socket_path() {
            Ok(path) if path.exists() => {
                let mut pipewire_mount = path.into_os_string();
                pipewire_mount.push(":");
                pipewire_mount.push(rt_dir);
                pipewire_mount.push("/pipewire-0");

                debug!("Appending PipeWire socket args");
                cmd.arg("--volume");
                cmd.arg(pipewire_mount);
            }
            Ok(path) => {
                warn!(
                    "PipeWire exposure requested but socket '{}' was not found; skipping",
                    path.display()
                );
            }
            Err(cause) => {
                warn!("PipeWire exposure requested but unavailable ({cause}); skipping");
            }
        }
    }

    if settings.support_tuntap {
        debug!("Appending TUN/TAP args");
        cmd.args(["--device", "/dev/net/tun"]);
        cmd.args(["--cap-add", "NET_ADMIN"]);
    }

    if settings.support_ping {
        debug!("Appending ping args");
        cmd.args(["--cap-add", "NET_RAW"]);
    }

    if settings.packet_forwarding {
        debug!("Appending packet forwarding args");
        cmd.args(["--sysctl", "net.ipv4.ip_forward=1"]);
        cmd.args(["--sysctl", "net.ipv6.conf.all.forwarding=1"]);
    }

    if settings.keep_groups {
        debug!("Appending keep groups args");
        cmd.args(["--group-add", "keep-groups"]);
    }

    if settings.unconfine_seccomp {
        debug!("Disabling seccomp confinement");
        cmd.args(["--security-opt", "seccomp=unconfined"]);
    }

    if settings.expose_kfd {
        debug!("Appending KFD device args");
        cmd.args(["--device", "/dev/kfd"]);
    }

    if let Some(shm_size) = settings.shm_size_gb.map(|gb| format!("{gb}G")) {
        debug!("Appending shm-size args: {shm_size}");
        cmd.args(["--shm-size", &shm_size]);
    }

    if let Some(ref custom_args) = settings.custom_podman_args {
        debug!("Appending custom podman args: {custom_args}");
        for arg in custom_args.split_whitespace() {
            cmd.arg(arg);
        }
    }

    // It's best to have the image_id as the final argument
    cmd.arg(&image_id);

    trace_arguments(&cmd);
    let child = cmd.spawn().context("Failed to run podman command")?;
    wait_for_podman(child)?;

    info!("Created container '{container_name}'.");
    Ok(())
}

pub fn start_daemon(lbx_name: &str) -> Result<(), anyhow::Error> {
    let keys = Keys::load()?;
    let password = keys.password_if_needed(lbx_name)?;
    let log_file_out = files::daemon_log_file(lbx_name)?;
    let log_file_err = log_file_out.try_clone()?;
    let mut cmd = Command::new(env::litterbox_binary_path());

    cmd.args(["daemon", lbx_name]);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::from(log_file_out));
    cmd.stderr(Stdio::from(log_file_err));

    let mut child = cmd.spawn().context("Failed to run Litterbox daemon")?;

    if let Some(pwd) = password
        && let Some(mut stdin) = child.stdin.take()
    {
        use std::io::Write;

        stdin
            .write_all(pwd.as_bytes())
            .context("Failed to write password to daemon")?;
    }

    Ok(())
}

pub fn delete_litterbox(lbx_name: &str) -> Result<()> {
    let container =
        get_container(lbx_name)?.ok_or_else(|| anyhow!("No container found for {}", lbx_name))?;
    let container_id = container.id;

    let should_delete = Confirm::new("Are you sure you want to delete this Litterbox?")
        .with_default(false)
        .with_help_message(
            "This operation cannot be undone and will delete all data/state outside the home directory.",
        )
        .prompt();

    if !should_delete.is_ok_and(|x| x) {
        eprintln!("Okay, the Litterbox won't be deleted!");
        return Ok(());
    }

    let mut cmd = Command::new("podman");
    cmd.args(["rm", &container_id]);
    trace_arguments(&cmd);
    let child = cmd.spawn().context("Failed to run podman command")?;

    wait_for_podman(child)?;
    info!("Container for Litterbox deleted!");

    let image_details =
        get_image(lbx_name)?.ok_or_else(|| anyhow!("No image found for {}", lbx_name))?;
    let mut cmd = Command::new("podman");
    cmd.args(["image", "rm", &image_details.id]);
    trace_arguments(&cmd);
    let child = cmd.spawn().context("Failed to run podman command")?;

    wait_for_podman(child)?;
    info!("Image for Litterbox deleted!");

    let home_path = files::lbx_home_path(lbx_name)?;
    if home_path.exists() {
        let should_delete_home =
            Confirm::new("Do you want to delete the home directory for this Litterbox?")
                .with_default(false)
                .with_help_message(&format!("This will delete {home_path:?}"))
                .prompt();

        match should_delete_home {
            Ok(true) => {
                fs::remove_dir_all(&home_path)?;
                info!("Home directory deleted!");
            }
            _ => {
                eprintln!("Skipping home directory deletion.");
            }
        }
    }

    let dockerfile_path = files::dockerfile_path(lbx_name)?;
    let settings_path = files::settings_path(lbx_name)?;
    if dockerfile_path.exists() || settings_path.exists() {
        let should_delete_definition =
            Confirm::new("Do you want to delete the definition files for this Litterbox?")
                .with_default(false)
                .with_help_message("This will delete the Dockerfile and settings file")
                .prompt();

        if should_delete_definition.is_ok_and(|x| x) {
            fs::remove_file(&dockerfile_path)
                .inspect(|_| info!("Dockerfile deleted!"))
                .or_else(|cause| {
                    (cause.kind() == ErrorKind::NotFound)
                        .then_some(())
                        .ok_or(cause)
                })?;

            fs::remove_file(&settings_path)
                .inspect(|_| info!("Settings file deleted!"))
                .or_else(|cause| {
                    (cause.kind() == ErrorKind::NotFound)
                        .then_some(())
                        .ok_or(cause)
                })?;
        } else {
            eprintln!("Skipping definition file deletion.");
        }
    }

    Ok(())
}

pub fn wait_for_podman(mut child: Child) -> Result<()> {
    let res = child.wait().context("Failed to run podman command")?;
    ensure!(res.success(), "Podman command failed");
    Ok(())
}

pub async fn wait_for_podman_async(child: &mut tokio::process::Child) -> Result<()> {
    let res = child.wait().await.context("Failed to run podman command")?;
    ensure!(res.success(), "Podman command failed");
    Ok(())
}
