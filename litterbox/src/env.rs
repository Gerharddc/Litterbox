use anyhow::Result;
use shared::env::get_env;
use std::path::PathBuf;

pub use shared::env::xdg_runtime_dir;

pub fn home_dir() -> Result<PathBuf> {
    get_env("HOME").map(PathBuf::from)
}

pub fn wayland_display() -> Result<String> {
    get_env("WAYLAND_DISPLAY")
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
