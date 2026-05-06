use crate::files;
use crate::sandbox;
use crate::utils::SU_BINARIES;

use anyhow::{Context as _, Result, bail};
use clap::Args;
use log::{debug, info, warn};
use nix::{
    sys::{
        prctl::set_child_subreaper,
        signal::{Signal, kill},
        wait::{WaitPidFlag, WaitStatus, waitpid},
    },
    unistd::{Gid, Pid, Uid, chown, setgid, setuid},
};
use shared::entrypoint::{CommonEntrypointOptions, WaitBehaviour};
use shared::env;
use std::{
    fs,
    os::unix::{fs::symlink, prelude::ExitStatusExt},
    process::{ExitStatus, Stdio},
    sync::mpsc::RecvTimeoutError,
    time::Duration,
};

/// Container entrypoint (for internal use)
#[derive(Args, Debug)]
pub struct Command {
    /// The UID to drop to if dropping privileges
    #[arg(long, value_parser = |x: &str| x.parse().map(Uid::from_raw))]
    uid: Uid,

    /// The GID to drop to if dropping privileges
    #[arg(long, value_parser = |x: &str| x.parse().map(Gid::from_raw))]
    gid: Gid,

    #[clap(flatten)]
    opts: CommonEntrypointOptions,
}

impl Command {
    pub fn run(self) -> Result<()> {
        use std::process::Command;

        match env::xdg_runtime_dir() {
            Ok(xdg_runtime_dir) => {
                if !xdg_runtime_dir.exists()
                    && let Err(cause) = fs::create_dir_all(&xdg_runtime_dir)
                {
                    warn!("Failed to create $XDG_RUNTIME_DIR: {cause}");
                }

                if let Err(cause) = chown(&xdg_runtime_dir, Some(self.uid), Some(self.gid)) {
                    warn!("Failed to set owner of $XDG_RUNTIME_DIR: {cause}");
                }
            }
            Err(cause) => {
                warn!("$XDG_RUNTIME_DIR is unavailable: {cause}");
            }
        }

        for su_bin in SU_BINARIES {
            let _ = symlink("/lbx-init", format!("/usr/bin/{su_bin}"));
        }

        if !self.opts.root {
            setgid(self.gid)?;
            setuid(self.uid)?;
            debug!("Dropped from root to {}:{}", self.uid, self.gid);
        } else {
            debug!("Will keep root privileges!");
        }

        sandbox::apply_landlock()?;
        files::setup_home()?;

        let mut cmd = Command::new(&env::shell()?);
        cmd.stderr(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
        cmd.stdin(Stdio::inherit());
        // $RUST_LOG can be passed from `litterbox build` for development and
        // debugging purposes. We don't want child processes to inherit it.
        cmd.env_remove("RUST_LOG");

        // Have the shell assume it's a login shell.
        cmd.arg("-l");

        if let Some(mut exec_args) = self.opts.command {
            // We can't use Command::args for "command" because shells generally
            // expect a single argument for the "-c" option.
            for arg in self.opts.args {
                exec_args.push(" ");
                exec_args.push(arg);
            }

            // Have the shell execute just `exec_args` and then exit.
            cmd.arg("-c");
            cmd.arg(exec_args);
        }

        let shell_child = cmd.spawn().context("Failed to launch shell")?;
        let shell_pid = Pid::from_raw(shell_child.id() as i32);
        let mut waitpid_flags = WaitPidFlag::empty();

        set_child_subreaper(true).context("failed to make process child subreaper")?;

        loop {
            match waitpid(None, Some(waitpid_flags)) {
                Ok(WaitStatus::Exited(pid, status)) => {
                    let status = ExitStatus::from_raw(status);

                    if pid == shell_pid {
                        debug!(
                            "Login shell {:?} (PID: {pid}) exited: {status}",
                            cmd.get_program()
                        );

                        if !status.success() {
                            bail!("Failed to execute {:?}", cmd.get_program());
                        }

                        // Activate the `WaitStatus::StillAlive` arm.
                        waitpid_flags |= WaitPidFlag::WNOHANG;
                    } else {
                        debug!("Child process {pid} exited: {status}");
                    }
                }

                Ok(WaitStatus::Signaled(pid, signal, _)) => {
                    if pid == shell_pid {
                        warn!(
                            "Login shell {:?} (PID: {pid}) was killed with signal {signal}",
                            cmd.get_program()
                        );

                        // Activate the `WaitStatus::StillAlive` arm.
                        waitpid_flags |= WaitPidFlag::WNOHANG;
                    } else {
                        debug!("Child process {pid} was killed with signal {signal}");
                    }
                }

                Ok(WaitStatus::StillAlive) => {
                    // Disable this arm.
                    waitpid_flags -= WaitPidFlag::WNOHANG;

                    debug!("Wait behavior has been set to \"{}\"", self.opts.wait);

                    match self.opts.wait {
                        WaitBehaviour::Foreground => {
                            // The default SIGINT behavior is to terminate the
                            // process.
                            info!("Press CTRL+C to exit.");
                        }

                        WaitBehaviour::Background => {
                            // Exit just this process to pass its descendants to
                            // the next child subreaper, `lbx-init wait` (the entrypoint).
                            info!("Continuing background processes in the background.");

                            break;
                        }

                        WaitBehaviour::Kill => {
                            // Explicitly kill the process and its children. Pid
                            // of 0 wouldn't work because orphaned processes
                            // have different group IDs.

                            kill(Pid::from_raw(-1), Signal::SIGTERM)
                                .context("Kill child processes")?;

                            let (tx, rx) = std::sync::mpsc::sync_channel(1);

                            std::thread::spawn(move || {
                                loop {
                                    if let Err(nix::errno::Errno::ECHILD) = waitpid(None, None) {
                                        debug!("All orphaned processes exited within 10 seconds");

                                        break;
                                    }
                                }

                                let _ = tx.try_send(());
                            });

                            if let Err(RecvTimeoutError::Timeout) =
                                rx.recv_timeout(Duration::from_secs(10))
                            {
                                warn!(
                                    "SIGTERM failed to kill background processes in 10 seconds, resorting to SIGKILL"
                                );

                                kill(Pid::from_raw(-1), Signal::SIGKILL)
                                    .context("Kill child processes")?;
                            }

                            break;
                        }
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

        Ok(())
    }
}
