use anyhow::{Result, bail};
use clap::Args;
use log::{debug, info, warn};
use nix::sys::{
    inotify::{AddWatchFlags, InitFlags, Inotify},
    wait::{WaitPidFlag, WaitStatus, waitpid},
};
use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;
use std::{fs, thread};

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

        let satellite_pid = start_xwayland_satellite();

        debug!("Waiting for sessions to end");

        loop {
            match std::fs::read_to_string(session_lock_path) {
                Ok(content) => {
                    if content.trim().is_empty() {
                        break;
                    }
                }

                Err(e) if e.kind() == ErrorKind::NotFound => break,

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
            match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::Exited(pid, status)) => {
                    debug!("Child process {pid} exited: {status}");
                }

                Ok(WaitStatus::Signaled(pid, signal, _)) => {
                    debug!("Child process {pid} was killed with signal {signal}");
                }

                Ok(WaitStatus::StillAlive) => {
                    // No more zombies ready to reap. Check if any non-xwayland children remain.
                    let has_other_children = fs::read_to_string("/proc/self/task/self/children")
                        .ok()
                        .map(|content| {
                            content
                                .split_whitespace()
                                .filter_map(|s| s.parse::<i32>().ok())
                                .any(|pid| satellite_pid != Some(pid))
                        })
                        .unwrap_or(false);

                    if has_other_children {
                        thread::sleep(Duration::from_secs(1));
                    } else {
                        debug!("Only xwayland daemon left, quitting");
                        break;
                    }
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

fn start_xwayland_satellite() -> Option<i32> {
    use std::process::Command as ProcessCommand;

    let mut cmd = ProcessCommand::new("xwayland-satellite");
    if let Ok(display) = std::env::var("DISPLAY") {
        cmd.arg(display);
    }

    match cmd.spawn() {
        Ok(child) => {
            info!("Started xwayland-satellite");
            Some(child.id() as i32)
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            debug!("xwayland-satellite not found in PATH");
            None
        }
        Err(e) => {
            warn!("Failed to start xwayland-satellite: {e}");
            None
        }
    }
}
