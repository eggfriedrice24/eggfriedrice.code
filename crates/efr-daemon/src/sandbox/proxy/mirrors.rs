//! The pacman mirror hosts of `@pacman-mirrors` (DP4): the `Server =` lines of the
//! mirror list and of every file that an `Include =` line of `pacman.conf` names, at
//! most [`PacmanFile::MAX_FILES`] files. The parser is pure (`efr-permissions`); this
//! module reads the files, off the async workers.
//!
//! efrd reads them at start and at each reload, and at a call's prepare when a file
//! that the last read used changed (its modification or change time, its inode or its
//! size), at most once a minute. Off Arch the files do not exist: the preset is empty,
//! with no warning.

use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;
use std::time::Duration;

use efr_permissions::network::parse_pacman;
use efr_permissions::{MirrorHost, PacmanFile};
use jiff::Timestamp;

/// How often a prepare may look at the files again.
pub(crate) const RECHECK: Duration = Duration::from_secs(60);

/// The most bytes of one pacman file that efrd reads.
const MAX_BYTES: u64 = 256 * 1024;

/// What tells that a file changed: its modification and change times, its inode and
/// its size; `None` for a file that does not exist.
type Stamp = Option<(i64, i64, i64, i64, u64, u64)>;

/// The pacman files and what their last read found.
#[derive(Debug)]
pub(crate) struct Mirrors {
    conf: PathBuf,
    list: PathBuf,
    read: Option<Read>,
}

/// One read: the files with their stamps, and when the files were last looked at.
#[derive(Debug, Clone)]
struct Read {
    files: Vec<(PathBuf, Stamp)>,
    checked: Timestamp,
}

impl Mirrors {
    /// `conf` is `pacman.conf` and `list` the mirror list; nothing is read yet.
    pub(crate) fn new(conf: PathBuf, list: PathBuf) -> Mirrors {
        Mirrors { conf, list, read: None }
    }

    /// Reads the files again when `force`, when nothing was read yet, or when a file of
    /// the last read changed and the last look is at least [`RECHECK`] old at `now`.
    /// `Some` holds the hosts of a new read; `None` means the last read stands. It
    /// blocks.
    pub(crate) fn refresh(&mut self, now: Timestamp, force: bool) -> Option<Vec<MirrorHost>> {
        if !force && let Some(read) = &mut self.read {
            let due = read.checked.checked_add(RECHECK).map_or(true, |due| now >= due);
            if !due {
                return None;
            }
            read.checked = now;
            if read.files.iter().all(|(path, stamp)| stamp_of(path) == *stamp) {
                return None;
            }
        }
        let (files, hosts) = read_files(self.conf.clone(), self.list.clone());
        self.read = Some(Read { files, checked: now });
        Some(hosts)
    }
}

/// The mirror hosts of `conf`, `list` and the files that their `Include =` lines name,
/// with the stamp of each file read. It blocks.
fn read_files(conf: PathBuf, list: PathBuf) -> (Vec<(PathBuf, Stamp)>, Vec<MirrorHost>) {
    let mut todo = vec![conf, list];
    let mut files: Vec<(PathBuf, Stamp)> = Vec::new();
    let mut hosts: Vec<MirrorHost> = Vec::new();
    let mut at = 0;
    while at < todo.len() && files.len() < PacmanFile::MAX_FILES {
        let path = todo[at].clone();
        at += 1;
        if files.iter().any(|(known, _)| *known == path) {
            continue;
        }
        let stamp = stamp_of(&path);
        let text = read_text(&path);
        files.push((path, stamp));
        let Some(text) = text else { continue };
        let parsed = parse_pacman(&text);
        for mirror in parsed.mirrors {
            if !hosts.contains(&mirror) {
                hosts.push(mirror);
            }
        }
        for include in parsed.includes {
            if !todo.contains(&include) {
                todo.push(include);
            }
        }
    }
    (files, hosts)
}

/// The text of `path`, at most [`MAX_BYTES`] of it; `None` when it cannot be read.
fn read_text(path: &std::path::Path) -> Option<String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn stamp_of(path: &std::path::Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.mtime(), meta.mtime_nsec(), meta.ctime(), meta.ctime_nsec(), meta.ino(), meta.len()))
}
