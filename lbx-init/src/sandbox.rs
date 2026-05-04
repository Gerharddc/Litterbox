use anyhow::Result;
#[cfg(target_os = "linux")]
use landlock::{
    ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, path_beneath_rules,
};
use log::debug;
#[cfg(target_os = "linux")]
use log::error;

#[cfg(target_os = "linux")]
pub fn apply_landlock() -> Result<()> {
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
        ))?;

    match ruleset.restrict_self() {
        Ok(status) => debug!("Landlock sandbox applied: {status:?}"),
        Err(cause) => error!("Failed to apply Landlock sandbox: {cause:?}"),
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn apply_landlock() -> Result<()> {
    debug!("Landlock is not available on this platform; skipping sandbox setup");
    Ok(())
}
