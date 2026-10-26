//! Dynamic project mounts.
//!
//! A Litterbox container bind-mounts a per-Litterbox host directory at
//! [`CONTAINER_SHARED_DIR`] with `rslave` propagation. Individual project
//! directories are bind-mounted into that host directory, which makes them
//! appear in the running container without recreating it.
//!
//! The host-side mounts require root privileges (like `litterbox device`), so
//! the relevant operations are performed through `sudo`.

use anyhow::{Context, Result, anyhow, bail, ensure};
use log::{debug, info};
use nix::fcntl::{Flock, FlockArg};
use shared::mountinfo;
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{files, utils::trace_arguments};

/// Directory inside the container where dynamically mounted projects appear.
pub const CONTAINER_SHARED_DIR: &str = "/home/user/shared";

/// Value of the `work.litterbox.shared` label for containers that were
/// created with the shared mount and slave propagation enabled.
pub const SHARED_MOUNT_LABEL: &str = "ready";

/// Value used when the shared mount exists but its source did not have the
/// propagation properties needed for dynamic mounts at creation time.
pub const SHARED_MOUNT_CONFIGURED: &str = "configured";

#[derive(Debug, Clone)]
pub struct ResolvedMount {
    pub source: PathBuf,
    pub name: OsString,
    pub container_path: PathBuf,
}

/// Serialises mount changes for one Litterbox across concurrent `enter`
/// invocations.
pub struct MountLock(#[allow(dead_code)] Flock<fs::File>);

pub fn lock(lbx_name: &str) -> Result<MountLock> {
    let path = files::mount_lock_path(lbx_name)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create mount lock directory {parent:?}"))?;
    }

    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("Failed to open mount lock file {path:?}"))?;

    let guard = Flock::lock(file, FlockArg::LockExclusive)
        .map_err(|(_, cause)| anyhow!("Failed to lock {path:?}: {cause}"))?;

    Ok(MountLock(guard))
}

/// Resolve the user-supplied mount sources to stable host paths and container
/// paths.
pub fn resolve(sources: &[PathBuf]) -> Result<Vec<ResolvedMount>> {
    let mut resolved = Vec::new();
    let mut by_name: HashMap<OsString, PathBuf> = HashMap::new();

    for source in sources {
        let canonical = fs::canonicalize(source)
            .with_context(|| format!("Failed to resolve mount source {source:?}"))?;
        ensure!(
            canonical.is_dir(),
            "Mount source {source:?} is not a directory"
        );

        let name = canonical
            .file_name()
            .ok_or_else(|| anyhow!("Cannot mount {source:?}: it has no file name"))?
            .to_owned();

        if let Some(existing) = by_name.insert(name.clone(), canonical.clone()) {
            if existing != canonical {
                bail!(
                    "Cannot mount both {existing:?} and {canonical:?}: both would appear as {:?} \
                     inside the Litterbox",
                    Path::new(CONTAINER_SHARED_DIR).join(&name)
                );
            }

            // The same source was specified more than once.
            continue;
        }

        resolved.push(ResolvedMount {
            source: canonical,
            container_path: Path::new(CONTAINER_SHARED_DIR).join(&name),
            name,
        });
    }

    Ok(resolved)
}

/// Ensure the host directory exists and return its canonical path.
pub fn shared_dir(lbx_name: &str) -> Result<PathBuf> {
    let path = files::mounts_dir_path(lbx_name)?;
    fs::create_dir_all(&path)
        .with_context(|| format!("Failed to create mount directory {path:?}"))?;

    fs::canonicalize(&path).with_context(|| format!("Failed to resolve mount directory {path:?}"))
}

/// Whether the mount containing `shared_dir` can receive host-side mounts.
///
/// The source needs to be *shared* (not merely a slave) so that mounts created
/// below it propagate to the container's slave mount.
pub fn propagation_ready(shared_dir: &Path) -> Result<bool> {
    let mounts = mountinfo::read_mountinfo().context("Failed to read /proc/self/mountinfo")?;
    let mount = mountinfo::deepest_mount_containing(shared_dir, &mounts)
        .ok_or_else(|| anyhow!("Could not find the mount containing {shared_dir:?}"))?;

    Ok(mount.is_shared())
}

/// Make sure host-side mounts below `shared_dir` propagate into containers.
///
/// This may create a bind mount for the directory and mark it as shared. Both
/// operations require root privileges.
pub fn ensure_propagation(shared_dir: &Path) -> Result<()> {
    if propagation_ready(shared_dir)? {
        return Ok(());
    }

    eprintln!(
        "Root permissions are required to enable dynamic mounts for this Litterbox. \
         Please enter your password if prompted."
    );

    if !mountinfo::is_mount_point(shared_dir)? {
        mount_bind(shared_dir, shared_dir)?;
    }

    make_rshared(shared_dir)?;

    ensure!(
        propagation_ready(shared_dir)?,
        "Failed to make {shared_dir:?} a shared mount"
    );

    info!("Configured shared mount propagation for {:?}", shared_dir);
    Ok(())
}

/// Remove stale project mounts before a stopped container is started.
///
/// Mounts are kept for the lifetime of a running container, so this is only
/// called when the container is stopped and no other sessions are active.
pub fn cleanup_stale(lbx_name: &str) -> Result<()> {
    let shared = shared_dir(lbx_name)?;
    if descendant_mounts(&shared)?.is_empty() {
        return Ok(());
    }

    eprintln!(
        "Root permissions are required to remove stale project mounts from this Litterbox. \
         Please enter your password if prompted."
    );

    unmount_descendants(&shared)
}

/// Mount the requested directories and return the container paths that should
/// be made accessible to the new session.
///
/// This is called after the container has been started so that the new mounts
/// propagate into its mount namespace. Mounts added by other concurrent
/// sessions are left in place.
pub fn apply(lbx_name: &str, requested: &[ResolvedMount]) -> Result<Vec<PathBuf>> {
    let shared = shared_dir(lbx_name)?;

    for mount in requested {
        ensure!(
            !shared.starts_with(&mount.source),
            "Cannot mount {:?}: it contains the Litterbox mount directory {shared:?}",
            mount.source
        );
        ensure!(
            !mount.source.starts_with(&shared),
            "Cannot mount {:?}: it is inside the Litterbox mount directory {shared:?}",
            mount.source
        );
    }

    let mut visible = direct_child_mounts(&shared)?;
    let needs_mount = requested
        .iter()
        .any(|mount| !visible.contains(&shared.join(&mount.name)));

    if needs_mount {
        eprintln!(
            "Root permissions are required to change the project mounts for this Litterbox. \
             Please enter your password if prompted."
        );
    }

    for mount in requested {
        let target = shared.join(&mount.name);

        if visible.iter().any(|mount_point| mount_point == &target) {
            ensure_same_inode(&mount.source, &target)?;
            debug!("{:?} is already mounted at {:?}", mount.source, target);
            continue;
        }

        ensure_target_dir(&target)?;
        mount_bind(&mount.source, &target)?;
        ensure_same_inode(&mount.source, &target)?;

        info!(
            "Mounted {:?} at {:?} inside the Litterbox",
            mount.source, mount.container_path
        );
        visible.push(target);
    }

    visible.sort();
    visible.dedup();

    visible
        .iter()
        .map(|mount_point| {
            let name = mount_point
                .file_name()
                .ok_or_else(|| anyhow!("Mount point {mount_point:?} does not have a file name"))?;

            Ok(Path::new(CONTAINER_SHARED_DIR).join(name))
        })
        .collect()
}

/// Remove all project mounts and the shared directory for a Litterbox.
///
/// Used when deleting a Litterbox.
pub fn cleanup_all(lbx_name: &str) -> Result<()> {
    let path = files::mounts_dir_path(lbx_name)?;
    if !path.exists() {
        return Ok(());
    }

    let shared = fs::canonicalize(&path)
        .with_context(|| format!("Failed to resolve mount directory {path:?}"))?;

    if !descendant_mounts(&shared)?.is_empty() {
        eprintln!(
            "Root permissions are required to remove the project mounts for this Litterbox. \
             Please enter your password if prompted."
        );
    }

    unmount_descendants(&shared)?;

    if mountinfo::is_mount_point(&shared)? {
        eprintln!(
            "Root permissions are required to remove the Litterbox mount directory. \
             Please enter your password if prompted."
        );
        umount(&shared)?;
    }

    if path.exists() {
        fs::remove_dir_all(&path)
            .with_context(|| format!("Failed to remove mount directory {path:?}"))?;
    }

    Ok(())
}

fn direct_child_mounts(shared: &Path) -> Result<Vec<PathBuf>> {
    let mounts = mountinfo::read_mountinfo().context("Failed to read /proc/self/mountinfo")?;
    let mut result: Vec<PathBuf> = mounts
        .into_iter()
        .filter(|mount| mount.mount_point != shared && mount.mount_point.parent() == Some(shared))
        .map(|mount| mount.mount_point)
        .collect();

    result.sort();
    result.dedup();
    Ok(result)
}

fn descendant_mounts(shared: &Path) -> Result<Vec<PathBuf>> {
    let mounts = mountinfo::read_mountinfo().context("Failed to read /proc/self/mountinfo")?;
    let mut result: Vec<PathBuf> = mounts
        .into_iter()
        .filter(|mount| mount.mount_point != shared && mount.mount_point.starts_with(shared))
        .map(|mount| mount.mount_point)
        .collect();

    // Unmount children before their parents.
    result.sort_by_key(|mount_point| std::cmp::Reverse(mount_point.components().count()));
    Ok(result)
}

fn unmount_descendants(shared: &Path) -> Result<()> {
    let mounts = descendant_mounts(shared)?;
    if mounts.is_empty() {
        return Ok(());
    }

    for mount_point in mounts {
        info!("Unmounting stale project mount {mount_point:?}");
        umount(&mount_point).with_context(|| format!("Failed to unmount {mount_point:?}"))?;
    }

    let remaining = descendant_mounts(shared)?;
    ensure!(
        remaining.is_empty(),
        "Some project mounts could not be unmounted: {remaining:?}"
    );

    Ok(())
}

fn ensure_target_dir(target: &Path) -> Result<()> {
    match fs::symlink_metadata(target) {
        Ok(metadata) => {
            ensure!(
                !metadata.file_type().is_symlink(),
                "Refusing to mount over the symlink {target:?}"
            );
            ensure!(
                metadata.is_dir(),
                "Mount target {target:?} is not a directory"
            );
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(target)
                .with_context(|| format!("Failed to create mount target {target:?}"))?;
        }
        Err(cause) => {
            return Err(cause)
                .with_context(|| format!("Failed to inspect mount target {target:?}"));
        }
    }

    Ok(())
}

fn ensure_same_inode(source: &Path, target: &Path) -> Result<()> {
    let source_metadata =
        fs::metadata(source).with_context(|| format!("Failed to stat mount source {source:?}"))?;
    let target_metadata =
        fs::metadata(target).with_context(|| format!("Failed to stat mount target {target:?}"))?;

    ensure!(
        source_metadata.dev() == target_metadata.dev()
            && source_metadata.ino() == target_metadata.ino(),
        "The Litterbox already has something else mounted at {target:?}. Stop the Litterbox \
         before replacing it with {source:?}."
    );

    Ok(())
}

fn mount_bind(source: &Path, target: &Path) -> Result<()> {
    let mut cmd = Command::new("sudo");
    cmd.arg("mount").arg("--bind").arg(source).arg(target);
    trace_arguments(&cmd);

    let status = cmd.status().context("Failed to run `sudo mount --bind`")?;
    ensure!(
        status.success(),
        "`sudo mount --bind {source:?} {target:?}` failed"
    );

    Ok(())
}

fn make_rshared(path: &Path) -> Result<()> {
    let mut cmd = Command::new("sudo");
    cmd.arg("mount").arg("--make-rshared").arg(path);
    trace_arguments(&cmd);

    let status = cmd
        .status()
        .context("Failed to run `sudo mount --make-rshared`")?;
    ensure!(
        status.success(),
        "`sudo mount --make-rshared {path:?}` failed"
    );

    Ok(())
}

fn umount(path: &Path) -> Result<()> {
    let mut cmd = Command::new("sudo");
    cmd.arg("umount").arg("--").arg(path);
    trace_arguments(&cmd);

    let status = cmd.status().context("Failed to run `sudo umount`")?;
    ensure!(status.success(), "`sudo umount {path:?}` failed");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "litterbox-mounts-test-{}-{test_name}",
            std::process::id()
        ));

        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn resolves_container_path_from_source_name() {
        let root = test_root("resolve");
        let source = root.join("project");
        fs::create_dir_all(&source).unwrap();

        let resolved = resolve(std::slice::from_ref(&source)).unwrap();

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].source, fs::canonicalize(&source).unwrap());
        assert_eq!(
            resolved[0].container_path,
            PathBuf::from("/home/user/shared/project")
        );

        let resolved = resolve(&[source.clone(), source]).unwrap();
        assert_eq!(
            resolved.len(),
            1,
            "duplicate sources should be deduplicated"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_sources_with_the_same_name() {
        let root = test_root("collision");
        let first = root.join("first/project");
        let second = root.join("second/project");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();

        let cause = resolve(&[first, second]).unwrap_err();
        assert!(
            cause.to_string().contains("both would appear as"),
            "unexpected error: {cause:#}"
        );

        fs::remove_dir_all(root).unwrap();
    }
}
