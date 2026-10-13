//! The `findmnt --df` and `findmnt --dfi` tables of a snapshot, in util-linux's format.

use super::human::util_linux_size;
use super::{MountEntry, MountSizes, MountSnapshot};

/// The types that `findmnt --df` leaves out: libmount's pseudo file systems, but every
/// type with `tmpfs` in its name stays.
const PSEUDO_TYPES: &[&str] = &[
    "anon_inodefs",
    "apparmorfs",
    "autofs",
    "bdev",
    "binder",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "cpuset",
    "debugfs",
    "devfs",
    "devpts",
    "dlmfs",
    "dmabuf",
    "drm",
    "efivarfs",
    "fuse",
    "fuse.archivemount",
    "fuse.avfsd",
    "fuse.dumpfs",
    "fuse.encfs",
    "fuse.gvfs-fuse-daemon",
    "fuse.gvfsd-fuse",
    "fuse.lxcfs",
    "fuse.portal",
    "fuse.rofiles-fuse",
    "fuse.vmware-vmblock",
    "fuse.xwmfs",
    "fusectl",
    "hugetlbfs",
    "ipathfs",
    "mqueue",
    "nfsd",
    "none",
    "nsfs",
    "overlay",
    "pipefs",
    "proc",
    "pstore",
    "ramfs",
    "resctrl",
    "rootfs",
    "rpc_pipefs",
    "securityfs",
    "selinuxfs",
    "smackfs",
    "sockfs",
    "spufs",
    "sysfs",
    "tracefs",
];

/// The `findmnt --df` table of `entries` (with `inodes`, the `--dfi` table): every
/// mount that is not a pseudo file system, in the order of the table, with the sizes
/// of `snapshot`. A mount with no sizes shows `-`.
pub fn render_findmnt_df(entries: &[MountEntry], snapshot: &MountSnapshot, inodes: bool) -> String {
    let headers: [&str; 7] = if inodes {
        ["SOURCE", "FSTYPE", "INO.TOTAL", "INO.USED", "INO.AVAIL", "INO.USE%", "TARGET"]
    } else {
        ["SOURCE", "FSTYPE", "SIZE", "USED", "AVAIL", "USE%", "TARGET"]
    };
    let mut table: Vec<[String; 7]> = vec![headers.map(str::to_owned)];
    for entry in entries {
        if !entry.fstype.contains("tmpfs") && PSEUDO_TYPES.contains(&entry.fstype.as_str()) {
            continue;
        }
        let source = if entry.root.as_os_str() == "/" {
            entry.source.clone()
        } else {
            format!("{}[{}]", entry.source, entry.root.to_string_lossy())
        };
        let numbers = match snapshot.sizes(entry.id) {
            Some(MountSizes::Measured { blocks, bfree, bavail, frsize, files, ffree }) => {
                if inodes {
                    let used = files.saturating_sub(*ffree);
                    [count(*files), count(used), count(*ffree), use_percent(used, *files)]
                } else {
                    let bytes = |count: u64| size(count.saturating_mul(*frsize));
                    let used = blocks.saturating_sub(*bfree);
                    [bytes(*blocks), bytes(used), bytes(*bavail), use_percent(used, *blocks)]
                }
            }
            Some(MountSizes::Dummy) => ["0", "0", "0", "-"].map(str::to_owned),
            _ => ["-", "-", "-", "-"].map(str::to_owned),
        };
        let [first, second, third, fourth] = numbers;
        table.push([
            escape(&source),
            escape(&entry.fstype),
            first,
            second,
            third,
            fourth,
            escape(&entry.target.to_string_lossy()),
        ]);
    }
    let mut widths = [0_usize; 7];
    for row in &table {
        for (width, text) in widths.iter_mut().zip(row) {
            *width = (*width).max(text.chars().count());
        }
    }
    let mut out = String::new();
    for row in &table {
        let mut line = String::new();
        for (at, text) in row.iter().enumerate() {
            let pad = widths[at].saturating_sub(text.chars().count());
            match at {
                0 | 1 => {
                    line.push_str(text);
                    line.extend(std::iter::repeat_n(' ', pad + 1));
                }
                6 => line.push_str(text),
                _ => {
                    line.extend(std::iter::repeat_n(' ', pad));
                    line.push_str(text);
                    line.push(' ');
                }
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn size(bytes: u64) -> String {
    if bytes == 0 { "0".to_owned() } else { util_linux_size(bytes) }
}

fn count(value: u64) -> String {
    value.to_string()
}

/// `used` as a percent of `total`, rounded to nearest; `-` for a total of 0.
fn use_percent(used: u64, total: u64) -> String {
    if total == 0 {
        return "-".to_owned();
    }
    #[expect(clippy::cast_precision_loss, reason = "a percent needs no more than a double")]
    let pct = used as f64 / total as f64 * 100.0;
    format!("{pct:.0}%")
}

/// A name as `findmnt` prints it: control characters as `\xHH`.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.push_str(&format!("\\x{:02x}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}
