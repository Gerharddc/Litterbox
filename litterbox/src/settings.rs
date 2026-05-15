use anyhow::{Context, Result, anyhow};
use inquire::{Confirm, Text};
use inquire_derive::Selectable;
use log::debug;
use serde::{Deserialize, Serialize};
use std::{fmt::Display, path::Path};

use crate::files::{pipewire_socket_path, read_file, settings_path, write_file};

#[derive(Debug, Copy, Clone, Selectable, Serialize, Deserialize, PartialEq)]
pub enum NetworkMode {
    Pasta,
    PastaHostToContainer,
    PastaContainerToHost,
    PastaBidirectional,
    Host,
}

impl NetworkMode {
    fn name(&self) -> &'static str {
        match self {
            NetworkMode::Pasta => "Pasta (isolated user-mode networking stack)",
            NetworkMode::PastaHostToContainer => {
                "Pasta with automatic port forwarding (host to container)"
            }
            NetworkMode::PastaContainerToHost => {
                "Pasta with automatic port forwarding (container to host)"
            }
            NetworkMode::PastaBidirectional => {
                "Pasta with automatic port forwarding (bidirectional)"
            }
            NetworkMode::Host => "Host networking (i.e. NO ISOLATION)",
        }
    }

    pub fn podman_args(&self) -> &'static str {
        match self {
            NetworkMode::Pasta => "pasta",
            NetworkMode::PastaHostToContainer => "pasta:-t,auto,-u,auto",
            NetworkMode::PastaContainerToHost => "pasta:-T,auto,-U,auto",
            NetworkMode::PastaBidirectional => "pasta:-t,auto,-u,auto,-T,auto,-U,auto",
            NetworkMode::Host => "host",
        }
    }
}

impl Display for NetworkMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Settings for a Litterbox container, persisted to disk as RON.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LitterboxSettings {
    /// Version of the settings format stored for future migrations
    pub version: u32,

    // Original settings:
    pub support_ping: bool,
    pub support_tuntap: bool,
    pub packet_forwarding: bool,
    pub expose_pipewire: bool,

    // Settings added later which need defaults:
    #[serde(default = "default_false")]
    pub keep_groups: bool,
    #[serde(default = "default_false")]
    pub expose_kfd: bool,
    #[serde(default = "default_false")]
    pub unconfine_seccomp: bool,
    #[serde(default)]
    pub shm_size_gb: Option<u32>,
    #[serde(default = "default_pasta")]
    pub network_mode: NetworkMode,
    #[serde(default)]
    pub custom_podman_args: Option<String>,
}

fn default_false() -> bool {
    false
}

fn default_pasta() -> NetworkMode {
    NetworkMode::Pasta
}

impl LitterboxSettings {
    /// Load existing settings if available, prompt user if they want to change them,
    /// and save the final settings. This is the main entry point for getting settings
    /// during a build.
    pub fn load_or_prompt(lbx_name: &str) -> Result<Self> {
        let existing = Self::load(lbx_name)?;

        let settings = match &existing {
            Some(existing) => {
                if Confirm::new("Would you like to change the settings for this Litterbox?")
                    .with_default(false)
                    .prompt()?
                {
                    Self::prompt(Some(existing))?
                } else {
                    existing.clone()
                }
            }
            None => Self::prompt(None)?,
        };

        settings.save_to_file(lbx_name)?;
        Ok(settings)
    }

    fn load(lbx_name: &str) -> Result<Option<Self>> {
        let path = settings_path(lbx_name)?;
        if !path.exists() {
            debug!("Settings file does not exist for {}", lbx_name);
            return Ok(None);
        }

        let contents = read_file(&path)?;
        let settings: Self = ron::from_str(&contents)?;
        Ok(Some(settings))
    }

    fn save_to_file(&self, lbx_name: &str) -> Result<()> {
        use ron::ser::{PrettyConfig, to_string_pretty};

        let path = settings_path(lbx_name)?;
        let contents = to_string_pretty(self, PrettyConfig::default())
            .context("Failed to serialise settings")?;
        write_file(&path, &contents)
    }

    fn prompt(existing: Option<&Self>) -> Result<Self> {
        let network_mode = NetworkMode::select("Choose the network mode for this Litterbox:")
            .with_starting_cursor(existing.map(|s| s.network_mode as usize).unwrap_or(0))
            .prompt()?;

        let support_ping = Confirm::new("Do you want to support `ping` inside this Litterbox?")
            .with_default(existing.map(|s| s.support_ping).unwrap_or(false))
            .with_help_message("This will enable `CAP_NET_RAW`.")
            .prompt()?;

        let support_tuntap =
            Confirm::new("Do you want to support TUN/TAP creation inside this Litterbox?")
                .with_default(existing.map(|s| s.support_tuntap).unwrap_or(false))
                .with_help_message("This will enable `CAP_NET_ADMIN` and expose `/dev/net/tun`.")
                .prompt()?;

        let packet_forwarding =
            Confirm::new("Do you want to enable packet forwarding inside this Litterbox?")
                .with_default(existing.map(|s| s.packet_forwarding).unwrap_or(false))
                .prompt()?;

        let keep_groups =
            Confirm::new("Do you want to keep your user groups inside this Litterbox?")
                .with_default(existing.map(|s| s.keep_groups).unwrap_or(false))
                .with_help_message("This will preserve your host user's group memberships.")
                .prompt()?;

        let unconfine_seccomp = Confirm::new("Do you want to disable seccomp confinement?")
            .with_default(existing.map(|s| s.unconfine_seccomp).unwrap_or(false))
            .with_help_message(
                "This enables 'dangerous' syscalls required by things like the Mojo debugger.",
            )
            .prompt()?;

        let expose_kfd = if Path::new("/dev/kfd").exists() {
            Confirm::new("Do you want to expose /dev/kfd inside this Litterbox?")
                .with_default(existing.map(|s| s.expose_kfd).unwrap_or(false))
                .with_help_message("This will expose the AMD Kernel Fusion Driver for GPU compute.")
                .prompt()?
        } else {
            debug!("/dev/kfd not found on host system, user not prompted to expose it.");
            false
        };

        let expose_pipewire = if pipewire_socket_path()?.exists() {
            Confirm::new("Do you want to expose PipeWire inside this Litterbox?")
                .with_default(existing.map(|s| s.expose_pipewire).unwrap_or(false))
                .with_help_message(
                    "This will allow audio applications to work inside the Litterbox.",
                )
                .prompt()?
        } else {
            debug!("PipeWire socket not found on host system, user not prompted to expose it.");
            false
        };

        let shm_size_default = existing.and_then(|s| s.shm_size_gb);
        let shm_size_input = Text::new("Shared memory size in GB (leave empty for default):")
            .with_default(&shm_size_default.map(|v| v.to_string()).unwrap_or_default())
            .with_help_message("Sets --shm-size for the container (e.g., 8 for 8G).")
            .prompt()?;
        let shm_size_gb: Option<u32> = if shm_size_input.trim().is_empty() {
            None
        } else {
            Some(
                shm_size_input
                    .trim()
                    .parse()
                    .map_err(|_| anyhow!("shm_size_gb must be a valid integer"))?,
            )
        };

        let custom_podman_args_input =
            Text::new("Custom podman arguments (space-separated, leave empty for none):")
                .with_default(
                    &existing
                        .and_then(|s| s.custom_podman_args.clone())
                        .unwrap_or_default(),
                )
                .with_help_message("Example: --device /dev/foo --volume /bar")
                .prompt()?;
        let custom_podman_args: Option<String> = if custom_podman_args_input.trim().is_empty() {
            None
        } else {
            Some(custom_podman_args_input.trim().to_string())
        };

        Ok(Self {
            version: 1,
            network_mode,
            support_ping,
            support_tuntap,
            packet_forwarding,
            unconfine_seccomp,
            expose_pipewire,
            keep_groups,
            expose_kfd,
            shm_size_gb,
            custom_podman_args,
        })
    }
}
