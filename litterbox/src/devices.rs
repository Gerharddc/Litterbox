use anyhow::{Context, Result, ensure};
use log::{debug, info};
use nix::sys::stat::{SFlag, major, minor, stat};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{files::lbx_home_path, utils::trace_arguments};

fn mknod(major_num: u64, minor_num: u64, dev_type: &str, path: &Path) -> Result<()> {
    eprintln!(
        "Root permissions are required to create a device node. Please enter your password if prompted."
    );

    let mut cmd = Command::new("sudo");
    cmd.args([
        "mknod",
        &path.to_string_lossy(), // TODO: maybe do something else instead?
        dev_type,
        &major_num.to_string(),
        &minor_num.to_string(),
    ]);
    trace_arguments(&cmd);
    let mut child = cmd.spawn().context("Failed to run mknod command")?;
    let res = child.wait().context("Failed to run mknod command")?;

    ensure!(res.success(), "mknod command failed");
    Ok(())
}

pub fn attach_device(lbx_name: &str, device_path: &str) -> Result<PathBuf> {
    let sub_path = device_path
        .strip_prefix("/dev/")
        .with_context(|| format!("Invalid device path: {device_path}"))?;
    debug!("sub_path: {:#?}", sub_path);

    let lbx_path = lbx_home_path(lbx_name)?;
    debug!("lbx_path: {:#?}", lbx_path);
    let dest_path = lbx_path.join("dev").join(sub_path);
    debug!("dest_path: {:#?}", dest_path);

    let metadata = stat(device_path).context("Failed to stat device")?;
    let rdev = metadata.st_rdev;
    let kind = SFlag::from_bits_truncate(metadata.st_mode);

    let major_num = major(rdev);
    let minor_num = minor(rdev);
    let dev_type = match kind {
        t if t.contains(SFlag::S_IFBLK) => "b",
        t if t.contains(SFlag::S_IFCHR) => "c",
        _ => "unknown",
    };

    debug!("Device Path: {}", device_path);
    info!(
        "Device Type: {}, Major: {}, Minor: {}",
        dev_type, major_num, minor_num
    );

    // Ensure that the path for the destination file exists
    let output_dir = dest_path
        .parent()
        .expect("Destination path should have parent.");
    fs::create_dir_all(output_dir).context("Failed to create output directory")?;
    debug!("Output dir ready!");

    mknod(major_num, minor_num, dev_type, &dest_path)?;
    // TODO: maybe we also need to set the owner and permissions
    Ok(dest_path)
}
