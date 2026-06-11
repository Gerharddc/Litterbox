use anyhow::{Result, bail};
use clap::Args;
use log::{debug, info, warn};
use nix::sys::{
    inotify::{AddWatchFlags, InitFlags, Inotify},
    wait::{WaitStatus, waitpid},
};
use std::path::Path;

/// Wait for the Litterbox to finish (for internal use)
#[derive(Args, Debug)]
pub struct Command {}

impl Command {
    pub fn run(self) -> Result<()> {
        crate::sandbox::apply_landlock()?;

        let session_lock_path = Path::new("/session.lock");
        let inotify = Inotify::init(InitFlags::empty())?;
        inotify.add_watch(session_lock_path, AddWatchFlags::IN_MODIFY)?;

        info!("Litterbox has started");

        start_xwayland_satellite();

        debug!("Waiting for sessions to end");

        loop {
            match std::fs::read_to_string(session_lock_path) {
                Ok(content) => {
                    if content.trim().is_empty() {
                        break;
                    }
                }

                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,

                Err(e) => {
                    log::error!("Failed to read session lock file: {e}");
                    break;
                }
            }

            let _ = inotify.read_events()?;
        }

        debug!("Waiting on unwaited-for child processes");

        // `lbx-init entrypoint` sends them over here.
        loop {
            match waitpid(None, None) {
                Ok(WaitStatus::Exited(pid, status)) => {
                    debug!("Child process {pid} exited: {status}");
                }

                Ok(WaitStatus::Signaled(pid, signal, _)) => {
                    debug!("Child process {pid} was killed with signal {signal}");
                }

                Ok(status) => {
                    warn!("Child signaled with unhandled status: {status:?}");
                }

                Err(nix::errno::Errno::ECHILD) => {
                    debug!("Received ECHILD: No remaining unwaited-for child processes");

                    break;
                }

                Err(cause) => bail!(cause),
            }
        }

        info!("Litterbox has finished");

        Ok(())
    }
}

fn start_xwayland_satellite() {
    use std::io::ErrorKind;
    use std::process::Command as ProcessCommand;

    let mut cmd = ProcessCommand::new("xwayland-satellite");
    if let Ok(display) = std::env::var("DISPLAY") {
        cmd.arg(display);
    }

    match cmd.spawn() {
        Ok(_) => info!("Started xwayland-satellite"),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            debug!("xwayland-satellite not found in PATH");
        }
        Err(e) => warn!("Failed to start xwayland-satellite: {e}"),
    }
}
