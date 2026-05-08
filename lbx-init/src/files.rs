use anyhow::{Context, Result};
use log::{debug, info};
use std::env;
use std::fs::File;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;

pub fn setup_home() -> Result<()> {
    let home = env::var("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")?;
    let marker = home.join(".home-built");

    if marker.exists() {
        debug!("Home already built; skipping.");
    } else {
        info!("Building home for the first time");

        Command::new("/prep-home.sh")
            .status()
            .or_else(|cause| {
                // The script is optional.
                (cause.kind() == ErrorKind::NotFound)
                    .then(Default::default)
                    .ok_or(cause)
            })
            .context("Running /prep-home.sh")?;

        File::create(&marker).context("Creating .home-built marker")?;
        info!("Home built!");
    }

    Ok(())
}
