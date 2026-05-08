/// Binaries that can be used to gain temporarily root privileges.
///
/// `lbx-init` symlinks them to itself inside the container to inform users to
/// use the "--root" argument if one wants to gain root access.
pub const SU_BINARIES: &[&str] = &["run0", "sudo", "doas"];
