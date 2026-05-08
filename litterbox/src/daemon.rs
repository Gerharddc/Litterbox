use anyhow::{Context, Result};
use log::info;
use nix::sys::signal::kill;
use nix::unistd::Pid;

use crate::files;
use crate::keys::Keys;
use crate::podman::is_container_running;

pub async fn run(lbx_name: &str, password: &str) -> Result<()> {
    let daemon_lock = files::daemon_lock_path(lbx_name)?;

    if daemon_lock.exists() {
        let pid_str =
            std::fs::read_to_string(&daemon_lock).context("Failed to read daemon lock file")?;

        if let Ok(pid) = pid_str.trim().parse().map(Pid::from_raw)
            && kill(pid, None).is_ok()
        {
            info!("Daemon already running for {}", lbx_name);
            return Ok(());
        }

        info!("Stale daemon lock file found, removing");
        std::fs::remove_file(&daemon_lock).context("Failed to remove stale daemon lock file")?;
    }

    let my_pid = std::process::id();
    std::fs::write(&daemon_lock, my_pid.to_string()).context("Failed to write daemon lock file")?;

    let keys = Keys::load()?;
    keys.start_ssh_server(lbx_name, password).await?;

    let session_path = files::session_lock_path(lbx_name)?;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        files::cleanup_dead_pids_from_session_lockfile(&session_path)?;

        if !is_container_running(lbx_name)? {
            info!("Container no longer running, daemon will stop.");
            break;
        }
    }

    if session_path.exists() {
        info!("Cleaning up session lockfile.");
        std::fs::remove_file(&session_path).context("Failed to remove session lock file")?;
    }

    std::fs::remove_file(&daemon_lock).context("Failed to remove daemon lock file")?;
    info!("Daemon exiting for {}", lbx_name);
    Ok(())
}

pub fn is_running(lbx_name: &str) -> Result<bool> {
    let daemon_lock = files::daemon_lock_path(lbx_name)?;

    std::fs::read_to_string(&daemon_lock)
        .ok()
        .and_then(|pid| pid.trim().parse().map(Pid::from_raw).ok())
        .map(|pid| Ok(kill(pid, None).is_ok()))
        .unwrap_or(Ok(false))
}
