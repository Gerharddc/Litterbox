//! Minimal `/proc/self/mountinfo` parsing used by both the host-side
//! `litterbox` binary and the in-container `lbx-init` binary.
//!
//! Mount points in mountinfo are escaped (for example, a space is written as
//! `\040`), so comparing paths without unescaping them is unreliable. The file
//! is parsed as bytes because mount points are not required to be valid UTF-8.

use std::{
    ffi::OsString,
    fs, io,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfo {
    pub mount_point: PathBuf,
    /// The optional fields between the mount options and the `-` separator,
    /// for example `shared:1` or `master:2`.
    pub optional_fields: Vec<String>,
}

impl MountInfo {
    pub fn is_shared(&self) -> bool {
        self.optional_fields
            .iter()
            .any(|field| field.starts_with("shared:"))
    }

    pub fn is_slave(&self) -> bool {
        self.optional_fields
            .iter()
            .any(|field| field.starts_with("master:"))
    }

    /// Whether this mount can be used as the source of a slave bind mount.
    ///
    /// Podman documents that the host source mount must be shared or slave for
    /// `bind-propagation=rslave` to work.
    pub fn is_shared_or_slave(&self) -> bool {
        self.is_shared() || self.is_slave()
    }
}

/// Read and parse `/proc/self/mountinfo` for the current mount namespace.
pub fn read_mountinfo() -> io::Result<Vec<MountInfo>> {
    let contents = fs::read("/proc/self/mountinfo")?;
    parse_mountinfo(&contents)
}

/// Whether `path` is itself a mount point in the current mount namespace.
pub fn is_mount_point(path: &Path) -> io::Result<bool> {
    let path = match fs::canonicalize(path) {
        Ok(path) => path,
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(cause) => return Err(cause),
    };

    Ok(read_mountinfo()?
        .iter()
        .any(|mount| mount.mount_point == path))
}

/// Return the deepest mount whose mount point contains `path`.
pub fn deepest_mount_containing<'a>(path: &Path, mounts: &'a [MountInfo]) -> Option<&'a MountInfo> {
    mounts
        .iter()
        .filter(|mount| path.starts_with(&mount.mount_point))
        .max_by_key(|mount| mount.mount_point.as_os_str().len())
}

fn parse_mountinfo(contents: &[u8]) -> io::Result<Vec<MountInfo>> {
    let mut mounts = Vec::new();

    for line in contents.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }

        let fields: Vec<&[u8]> = line.split(|byte| *byte == b' ').collect();
        if fields.len() < 7 {
            return Err(malformed(line));
        }

        let separator = fields
            .iter()
            .position(|field| field.len() == 1 && field[0] == b'-')
            .ok_or_else(|| malformed(line))?;

        // Fields before the separator are:
        // id, parent id, major:minor, root, mount point, mount options, and
        // then zero or more optional fields.
        if separator < 6 {
            return Err(malformed(line));
        }

        mounts.push(MountInfo {
            mount_point: unescape_mount_path(fields[4]),
            optional_fields: fields[6..separator]
                .iter()
                .map(|field| String::from_utf8_lossy(field).into_owned())
                .collect(),
        });
    }

    Ok(mounts)
}

fn malformed(line: &[u8]) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "Malformed mountinfo line: {:?}",
            String::from_utf8_lossy(line)
        ),
    )
}

/// Unescape an octal-escaped mountinfo field into a path.
fn unescape_mount_path(value: &[u8]) -> PathBuf {
    let mut unescaped = Vec::with_capacity(value.len());
    let mut index = 0;

    while index < value.len() {
        if value[index] == b'\\'
            && let Some(digit_1) = value.get(index + 1).and_then(octal_digit)
            && let Some(digit_2) = value.get(index + 2).and_then(octal_digit)
            && let Some(digit_3) = value.get(index + 3).and_then(octal_digit)
        {
            let value = u16::from(digit_1) * 64 + u16::from(digit_2) * 8 + u16::from(digit_3);
            if let Ok(value) = u8::try_from(value) {
                unescaped.push(value);
                index += 4;
                continue;
            }
        }

        unescaped.push(value[index]);
        index += 1;
    }

    PathBuf::from(OsString::from_vec(unescaped))
}

fn octal_digit(value: &u8) -> Option<u8> {
    match value {
        b'0'..=b'7' => Some(*value - b'0'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn unescapes_mount_points() {
        assert_eq!(
            unescape_mount_path(br"/home/user/my\040project"),
            PathBuf::from("/home/user/my project")
        );
        assert_eq!(
            unescape_mount_path(br"/home/user/a\011b\012c\134d"),
            PathBuf::from("/home/user/a\tb\nc\\d")
        );
        assert_eq!(
            unescape_mount_path(b"/home/user/plain"),
            PathBuf::from("/home/user/plain")
        );
    }

    #[test]
    fn keeps_non_utf8_mount_points() {
        let mounts = parse_mountinfo(b"1 0 0:1 / /home/\xff rw - ext4 /dev/sda1 rw\n").unwrap();

        assert_eq!(mounts[0].mount_point.as_os_str().as_bytes(), b"/home/\xff");
    }

    #[test]
    fn parses_optional_fields() {
        let contents = concat!(
            "36 29 0:32 / / rw shared:15 - overlay overlay rw\n",
            "37 36 0:33 / /home/user\\040space rw master:7 - ext4 /dev/sda1 rw\n",
            "38 36 0:34 / /private rw - tmpfs tmpfs rw\n",
        );

        let mounts = parse_mountinfo(contents.as_bytes()).unwrap();
        assert_eq!(mounts.len(), 3);
        assert!(mounts[0].is_shared());
        assert!(!mounts[0].is_slave());
        assert!(mounts[1].is_slave());
        assert!(mounts[1].is_shared_or_slave());
        assert_eq!(mounts[1].mount_point, PathBuf::from("/home/user space"));
        assert!(!mounts[2].is_shared_or_slave());
    }

    #[test]
    fn finds_deepest_containing_mount() {
        let contents = concat!(
            "1 0 0:1 / / rw shared:1 - overlay overlay rw\n",
            "2 1 0:2 / /home rw shared:2 - ext4 /dev/sda2 rw\n",
            "3 2 0:3 / /home/user rw - ext4 /dev/sda2 rw\n",
        );
        let mounts = parse_mountinfo(contents.as_bytes()).unwrap();

        let deepest = deepest_mount_containing(Path::new("/home/user/project"), &mounts).unwrap();
        assert_eq!(deepest.mount_point, PathBuf::from("/home/user"));

        let deepest = deepest_mount_containing(Path::new("/home/other"), &mounts).unwrap();
        assert_eq!(deepest.mount_point, PathBuf::from("/home"));

        // Component-wise prefix matching: /home/user2 is not inside /home/user.
        let deepest = deepest_mount_containing(Path::new("/home/user2/project"), &mounts).unwrap();
        assert_eq!(deepest.mount_point, PathBuf::from("/home"));
    }

    #[test]
    fn rejects_malformed_lines() {
        assert!(parse_mountinfo(b"not enough fields\n").is_err());
        assert!(parse_mountinfo(b"1 2 3:4 / / rw no-separator\n").is_err());
    }

    #[test]
    fn parses_the_real_mountinfo() {
        let mounts = read_mountinfo().unwrap();
        assert!(!mounts.is_empty());
        assert!(deepest_mount_containing(Path::new("/"), &mounts).is_some());
    }
}
