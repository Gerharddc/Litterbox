use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn get_env(var: &'static str) -> Result<String> {
    std::env::var(var).with_context(|| format!("Environment variable {var} is not defined"))
}

pub fn xdg_runtime_dir() -> Result<PathBuf> {
    get_env("XDG_RUNTIME_DIR").map(PathBuf::from)
}

pub fn shell() -> Result<String> {
    get_env("SHELL")
}
