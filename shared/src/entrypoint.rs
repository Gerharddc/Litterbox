//! Common entrypoint types shared between `litterbox` and `lbx-init`.

use clap::{Args, ValueEnum};
use std::{ffi::OsString, fmt::Display};

// If you add a new field, make sure to pass it inside the container in
// `container_exec_entrypoint`.
#[derive(Args, Debug)]
pub struct CommonEntrypointOptions {
    /// Run as root instead of dropping privileges.
    #[arg(long, default_value_t = false)]
    pub root: bool,

    /// Specify what to do with background processes.
    #[arg(long, value_enum, default_value_t = Default::default())]
    pub wait: WaitBehaviour,

    /// The command to execute with the login shell.
    pub command: Option<OsString>,

    /// Additional arguments to pass to COMMAND.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<OsString>,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum WaitBehaviour {
    /// Wait for background processes to exit.
    #[default]
    Foreground,
    /// Background processes will continue in the background.
    Background,
    /// Request background processes to exit within 10 seconds, after which kill
    /// them.
    Kill,
}

impl Display for WaitBehaviour {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.to_possible_value()
            .expect("there are no skipped variants")
            .get_name()
            .fmt(f)
    }
}
