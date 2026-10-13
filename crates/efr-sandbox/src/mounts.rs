//! The mount view of a contained call (efr's auto spec, section 8.8): the host's mount
//! table that efrd reads outside the sandbox, the sizes that it measured there, and the
//! `df` and `findmnt --df` tables that `efr-sbx mounts` prints from them inside a call.
//!
//! In the sandbox, `df` and `findmnt` read the call's own mount table: it holds every
//! mask, bind and cache overlay of the plan, and the sizes of `/tmp`, `/dev/shm` and the
//! runtime dir are those of the call's own mounts. efrd writes [`MOUNTS_FILE`] (the
//! host's `mountinfo` lines, verbatim) and [`SIZES_FILE`] (a [`MountSnapshot`]) into the
//! call dir, and the child shell's `df` and `findmnt` functions print them.
//!
//! The rules here are pure: which types are never measured ([`is_dummy`]), which
//! mounts belong to another user ([`is_other_users`]) and which lie below a mask of the
//! call ([`is_masked`]). efrd applies them before it writes the sizes.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::SandboxError;

mod df;
mod findmnt;
mod human;

pub use df::render_df;
pub use findmnt::render_findmnt_df;

/// The host's `mountinfo` lines in the call dir, and next to the launcher inside.
pub const MOUNTS_FILE: &str = "mounts";

/// The sizes of the host's mounts in the call dir, and next to the launcher inside.
pub const SIZES_FILE: &str = "mounts.json";

/// The most bytes of [`MOUNTS_FILE`] or [`SIZES_FILE`] that a reader takes.
pub const MAX_MOUNTS_BYTES: usize = 4 * 1024 * 1024;

/// The types whose sizes efrd never measures: a `statvfs` of `autofs` can mount a
/// network share or wait for one, and the others are views of the kernel with no
/// blocks. `df` hides them without `-a` (GNU's dummy list is part of this one).
const DUMMY_TYPES: &[&str] = &[
    "autofs",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "debugfs",
    "devfs",
    "devpts",
    "fuse.portal",
    "fusectl",
    "hugetlbfs",
    "ignore",
    "kernfs",
    "mqueue",
    "none",
    "nsfs",
    "proc",
    "pstore",
    "rpc_pipefs",
    "securityfs",
    "subfs",
    "sysfs",
    "tracefs",
];

/// One line of a `mountinfo` file (`proc_pid_mountinfo(5)`), with its octal escapes
/// (`\040`, `\011`, `\012`, `\134`) undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// The mount id, unique on the machine.
    pub id: u32,
    /// The id of the parent mount.
    pub parent: u32,
    /// The device's major number.
    pub major: u32,
    /// The device's minor number.
    pub minor: u32,
    /// The dir of the file system that is mounted here (`/` for a whole file system).
    pub root: PathBuf,
    /// Where it is mounted.
    pub target: PathBuf,
    /// The file system type, such as `ext4` or `fuse.sshfs`.
    pub fstype: String,
    /// The source, such as `/dev/nvme0n1p2` or `tmpfs`.
    pub source: String,
    /// The options of the mount.
    pub options: String,
    /// The options of the file system, such as `user_id=1000` of a FUSE mount.
    pub super_options: String,
}

impl MountEntry {
    /// The device as `major:minor`, the key of `df`'s one row per device.
    pub fn device(&self) -> (u32, u32) {
        (self.major, self.minor)
    }

    /// True for a FUSE mount (`fuse` or `fuse.<name>`), whose server can answer
    /// slowly or never.
    fn is_fuse(&self) -> bool {
        self.fstype == "fuse" || self.fstype.starts_with("fuse.")
    }

    /// The `user_id=` of a FUSE mount's options: the user who mounted it.
    fn fuse_user(&self) -> Option<u32> {
        self.super_options
            .split(',')
            .find_map(|option| option.strip_prefix("user_id="))
            .and_then(|uid| uid.parse().ok())
    }
}

/// Reads a `mountinfo` file. A line that does not parse is left out.
pub fn parse_mountinfo(text: &[u8]) -> Vec<MountEntry> {
    text.split(|byte| *byte == b'\n').filter_map(parse_line).collect()
}

fn parse_line(line: &[u8]) -> Option<MountEntry> {
    let fields: Vec<&[u8]> = line.split(|byte| *byte == b' ').collect();
    let dash = fields.iter().position(|field| *field == b"-")?;
    if dash < 6 || fields.len() < dash + 3 {
        return None;
    }
    let number = |field: &[u8]| std::str::from_utf8(field).ok()?.parse::<u32>().ok();
    let (major, minor) = {
        let text = std::str::from_utf8(fields[2]).ok()?;
        let (major, minor) = text.split_once(':')?;
        (major.parse().ok()?, minor.parse().ok()?)
    };
    let text = |field: &[u8]| String::from_utf8_lossy(&unescape(field)).into_owned();
    Some(MountEntry {
        id: number(fields[0])?,
        parent: number(fields[1])?,
        major,
        minor,
        root: path(fields[3]),
        target: path(fields[4]),
        options: text(fields[5]),
        fstype: text(fields[dash + 1]),
        source: text(fields[dash + 2]),
        super_options: fields.get(dash + 3).map(|field| text(field)).unwrap_or_default(),
    })
}

fn path(field: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(unescape(field)))
}

/// Undoes the kernel's octal escapes: a backslash and three octal digits stand for one
/// byte. Any other backslash stays as it is.
fn unescape(field: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(field.len());
    let mut at = 0;
    while at < field.len() {
        let octal = field.get(at + 1..at + 4).filter(|digits| {
            field[at] == b'\\' && digits.iter().all(|digit| (b'0'..=b'7').contains(digit))
        });
        match octal {
            Some(digits) => {
                let value =
                    digits.iter().fold(0_u32, |value, digit| value * 8 + u32::from(digit - b'0'));
                match u8::try_from(value) {
                    Ok(byte) => {
                        out.push(byte);
                        at += 4;
                    }
                    Err(_) => {
                        out.push(field[at]);
                        at += 1;
                    }
                }
            }
            None => {
                out.push(field[at]);
                at += 1;
            }
        }
    }
    out
}

/// The sizes of one mount, or why it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum MountSizes {
    /// What `statvfs` gave, outside the sandbox.
    Measured {
        /// The size in fragments.
        blocks: u64,
        /// The free fragments.
        bfree: u64,
        /// The fragments free for a user that is not root.
        bavail: u64,
        /// The fragment size in bytes.
        frsize: u64,
        /// The inodes.
        files: u64,
        /// The free inodes.
        ffree: u64,
    },
    /// A type that is never measured ([`is_dummy`]); `df` shows zeros for it.
    Dummy,
    /// A mount of another user ([`is_other_users`]).
    OtherUser,
    /// A mount at or below a mask of the call ([`is_masked`]).
    Masked,
    /// The mount did not answer by the deadline, or did not answer within the last
    /// minute.
    Timeout,
    /// `statvfs` failed with `errno`; 0 when the measuring process ended without an
    /// answer for this mount.
    Failed {
        /// The error number.
        errno: i32,
    },
}

/// The most paths that one `efr-sbx statvfs` measures.
pub const MAX_STATVFS_PATHS: usize = 4096;

/// The most bytes of one path that `efr-sbx statvfs` takes.
pub const MAX_STATVFS_PATH_BYTES: usize = 4096;

/// What `statvfs` gave for one path: the fields of [`MountSizes::Measured`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatvfsValues {
    /// The size in fragments.
    pub blocks: u64,
    /// The free fragments.
    pub bfree: u64,
    /// The fragments free for a user that is not root.
    pub bavail: u64,
    /// The fragment size in bytes.
    pub frsize: u64,
    /// The inodes.
    pub files: u64,
    /// The free inodes.
    pub ffree: u64,
}

/// One line of `efr-sbx statvfs`: `{"path": P, "ok": {...}}` or
/// `{"path": P, "errno": N}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatvfsLine {
    /// The path as stdin named it; efrd asks only for paths in UTF-8.
    pub path: String,
    /// The values, when `statvfs` succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<StatvfsValues>,
    /// The error number, when it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errno: Option<i32>,
    /// The device (`major`, `minor`) that `stat` gives for the path. GNU `df` keeps one
    /// row per such device; on btrfs it differs for each subvolume, while `mountinfo`
    /// gives every subvolume the device of the file system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<(u32, u32)>,
}

impl StatvfsLine {
    /// The sizes that the line gives: measured, or failed with its error number.
    pub fn sizes(&self) -> MountSizes {
        match (self.ok, self.errno) {
            (Some(values), _) => MountSizes::Measured {
                blocks: values.blocks,
                bfree: values.bfree,
                bavail: values.bavail,
                frsize: values.frsize,
                files: values.files,
                ffree: values.ffree,
            },
            (None, errno) => MountSizes::Failed { errno: errno.unwrap_or(0) },
        }
    }
}

/// What efrd measured for one call: the sizes of each mount, by mount id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct MountSnapshot {
    /// When efrd read the mount table, in seconds since the epoch.
    pub taken_at: u64,
    /// The sizes, by mount id. A mount with no entry has no sizes.
    pub sizes: BTreeMap<u32, MountSizes>,
    /// The device that `stat` gave for a mount's target, by mount id, where it differs
    /// from the device of its `mountinfo` line ([`StatvfsLine::device`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub devices: BTreeMap<u32, (u32, u32)>,
}

impl MountSnapshot {
    /// A snapshot taken at `taken_at` with `sizes`.
    pub fn new(taken_at: u64, sizes: BTreeMap<u32, MountSizes>) -> MountSnapshot {
        MountSnapshot { taken_at, sizes, devices: BTreeMap::new() }
    }

    /// The snapshot with the devices that `stat` gave, by mount id.
    #[must_use]
    pub fn with_devices(mut self, devices: BTreeMap<u32, (u32, u32)>) -> MountSnapshot {
        self.devices = devices;
        self
    }

    /// The device of `entry` as `df` counts it: what `stat` gave, else its line's.
    pub fn device(&self, entry: &MountEntry) -> (u32, u32) {
        self.devices.get(&entry.id).copied().unwrap_or(entry.device())
    }

    /// The sizes of the mount `id`, if efrd measured it.
    pub fn sizes(&self, id: u32) -> Option<&MountSizes> {
        self.sizes.get(&id)
    }

    /// The snapshot as the bytes of [`SIZES_FILE`].
    pub fn to_json(&self) -> Result<Vec<u8>, SandboxError> {
        serde_json::to_vec(self)
            .map_err(|source| SandboxError::Json { what: "mount sizes", source })
    }

    /// Reads [`SIZES_FILE`].
    pub fn from_json(bytes: &[u8]) -> Result<MountSnapshot, SandboxError> {
        if bytes.len() > MAX_MOUNTS_BYTES {
            return Err(SandboxError::TooLarge {
                what: "mount sizes",
                len: bytes.len(),
                max: MAX_MOUNTS_BYTES,
            });
        }
        serde_json::from_slice(bytes)
            .map_err(|source| SandboxError::Json { what: "mount sizes", source })
    }
}

/// True for a type whose sizes efrd never measures, and that `df` hides without `-a`.
pub fn is_dummy(fstype: &str) -> bool {
    DUMMY_TYPES.contains(&fstype)
}

/// True for a mount of another user than `uid`: one at or below `/run/user/<n>` with
/// another `n`, or a FUSE mount whose `user_id=` is another user. efrd gives them no
/// sizes: another user's FUSE server decides what `statvfs` sees, and the use of their
/// runtime dir is theirs.
pub fn is_other_users(entry: &MountEntry, uid: u32) -> bool {
    let runtime_owner = entry
        .target
        .strip_prefix("/run/user")
        .ok()
        .and_then(|rest| rest.components().next())
        .and_then(|first| first.as_os_str().to_str()?.parse::<u32>().ok());
    if runtime_owner.is_some_and(|owner| owner != uid) {
        return true;
    }
    entry.is_fuse() && entry.fuse_user().is_some_and(|owner| owner != uid)
}

/// True when `target` is at or below one of `masks`, or strictly below one of `below`.
///
/// `masks` are the read masks of the call (its secrets, the user's masks): a mount
/// there, such as an encrypted store, shows no sizes. `below` are the dirs that the
/// sandbox replaces with its own (the runtime dir, efr's roots): their own row keeps
/// its sizes, and the mounts in them (`doc`, `gvfs`) show none.
pub fn is_masked(target: &Path, masks: &[PathBuf], below: &[PathBuf]) -> bool {
    masks.iter().any(|mask| target.starts_with(mask))
        || below.iter().any(|dir| target != dir.as_path() && target.starts_with(dir))
}

#[cfg(test)]
mod tests;
