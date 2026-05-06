use anyhow::{Context, Result, ensure};
use shared::env::get_env;
use std::io::Read;
use std::path::PathBuf;

pub use shared::env::xdg_runtime_dir;

pub fn home_dir() -> Result<PathBuf> {
    get_env("HOME").map(PathBuf::from)
}

pub fn litterbox_binary_path() -> PathBuf {
    std::env::current_exe().expect("Binary path should be defined.")
}

pub fn lbx_init_binary_path() -> PathBuf {
    litterbox_binary_path()
        .parent()
        .expect("Litterbox binary path should have a parent")
        .join("lbx-init")
}

pub fn checked_lbx_init_binary_path() -> Result<PathBuf> {
    let path = lbx_init_binary_path();
    let mut file = std::fs::File::open(&path)
        .with_context(|| format!("Could not open lbx-init binary at '{}'", path.display()))?;
    let mut magic = [0_u8; 4];

    file.read_exact(&mut magic)
        .with_context(|| format!("Could not read lbx-init binary at '{}'", path.display()))?;

    ensure!(
        magic == [0x7f, b'E', b'L', b'F'],
        "lbx-init at '{}' is not a Linux ELF binary. Rebuild with './compile.sh' to cross-compile lbx-init for Linux.",
        path.display()
    );

    Ok(path)
}
