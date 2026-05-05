use anyhow::{Result, bail};
use log::trace;
use std::process::{Command, Output};

/// Binaries that can be used to gain temporarily root privileges.
///
/// Litterbox symlinks them to itself to inform users to use the "--root"
/// argument if one wants to gain root access.
pub const SU_BINARIES: &[&str] = &["run0", "sudo", "doas"];

pub fn trace_arguments(cmd: &Command) {
    trace!(
        "Will run: {} {}",
        cmd.get_program().to_string_lossy(),
        cmd.get_args().fold(String::new(), |mut acc, arg| {
            acc.push_str(&arg.to_string_lossy());
            acc.push(' ');
            acc
        })
    );
}

pub fn extract_stdout(output: &Output) -> Result<&str> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        bail!("Command failed: {stderr}");
    }

    Ok(str::from_utf8(&output.stdout)?)
}

pub fn podman_name(lbx_name: &str) -> String {
    format!("lbx-{lbx_name}")
}
