use anyhow::{Result, bail};
use clap::Args;
use log::{debug, info, warn};
#[cfg(target_os = "linux")]
use nix::sys::{
    inotify::{AddWatchFlags, InitFlags, Inotify},
    wait::{WaitStatus, waitpid},
};
#[cfg(not(target_os = "linux"))]
use nix::sys::wait::{WaitStatus, waitpid};
use std::path::Path;

/// Wait for the Litterbox to finish (for internal use)
#[derive(Args, Debug)]
pub struct Command {}

impl Command {
    pub fn run(self) -> Result<()> {
        let session_lock_path = Path::new("/session.lock");
        #[cfg(target_os = "linux")]
        let inotify = Inotify::init(InitFlags::empty())?;
        #[cfg(target_os = "linux")]
        inotify.add_watch(session_lock_path, AddWatchFlags::IN_MODIFY)?;

        info!("Litterbox has started");
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

            #[cfg(target_os = "linux")]
            {
                let _ = inotify.read_events()?;
            }

            #[cfg(not(target_os = "linux"))]
            {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
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
