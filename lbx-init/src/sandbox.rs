use anyhow::{Result, ensure};
use landlock::{
    ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, path_beneath_rules,
};
use log::{debug, error};
use std::path::PathBuf;

pub fn apply_landlock(allowed_mounts: &[PathBuf]) -> Result<()> {
    // Landlock rules are attached to inodes, not mount points. A directory
    // that was bind-mounted from the host has an inode that is not below the
    // container's home inode, so each dynamic mount needs its own rule.
    if !allowed_mounts.is_empty() {
        let mount_points = shared::mountinfo::read_mountinfo()?;

        for path in allowed_mounts {
            let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());

            ensure!(
                mount_points
                    .iter()
                    .any(|mount| mount.mount_point == canonical),
                "{path:?} is not a mount point inside the Litterbox. The host-side mount did not \
                 propagate; make sure the host mount directory is a shared mount."
            );
        }
    }

    // We avoid giving full access to the container's entire root directory so
    // that we can deny access to "internal" files that Litterbox places within
    // the root directory.
    let root_dirs = std::fs::read_dir("/")?.filter_map(|entry| {
        let path = entry.ok()?.path();

        path.is_dir().then_some(path)
    });

    let access_all = AccessFs::from_all(ABI::V6);
    let ruleset = Ruleset::default()
        .handle_access(access_all)?
        .create()?
        .add_rules(path_beneath_rules(root_dirs, access_all))?
        .add_rules(path_beneath_rules(["/"], AccessFs::ReadDir))?
        .add_rules(path_beneath_rules(
            ["/lbx-init", "/prep-home.sh"],
            AccessFs::Execute | AccessFs::ReadFile,
        ))?
        .add_rules(path_beneath_rules(["/session.lock"], AccessFs::ReadFile))?
        .add_rules(path_beneath_rules(allowed_mounts, access_all))?;

    match ruleset.restrict_self() {
        Ok(status) => debug!("Landlock sandbox applied: {status:?}"),
        Err(cause) => error!("Failed to apply Landlock sandbox: {cause:?}"),
    }

    Ok(())
}
