//! The backup store of floor files (efr's auto spec, section 10.9, phase A): before
//! efrd runs an approved edit of a file that runs code later, such as `~/.zshrc`, it
//! copies the file to `$XDG_DATA_HOME/efr/file-backups/<conversation>/<turn>/<n>/`, so
//! the user can copy it back. The name `backups/` holds the copies of the database.
//!
//! Each entry `<n>` holds the copy under the file's own name and `meta.json`: the path
//! as the call named it, each symbolic link on the way, the real file, its mode, and
//! whether it was missing. A missing file has no copy; its backup is `meta.json`.
//! Directories have mode 0700 and files 0600, and the data root is masked in the
//! sandbox, so contained code never reads a backup.
//!
//! The copy follows the links of a dotfiles layout (`~/.zshrc` that links into
//! `~/dotfiles`, or `~/.config/hypr` that links to a directory) to the real file, then
//! opens the real parent with no link anywhere on its path and reads the file with
//! `O_NOFOLLOW` from that descriptor. A secret or a masked file is never copied, at its
//! path or at its real path. Every failure stops the call: efrd never runs an edit
//! whose backup it could not take. After the call, a file that did not change drops its
//! entry.
//!
//! The store follows the keys of the snapshot store: [`collect`] keeps the newest
//! `snapshot.keep_turns` turns of each conversation and drops a turn older than
//! `snapshot.max_age_days`. Phase A has no command that restores a backup; the line at
//! the end of the turn names the copy.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use efr_conversation::FileBackup;
use efr_protocol::{ConversationId, TurnId};
use efr_sandbox::{is_within, resolve};
use efr_stdx::fs::{Claim, claim_dir, write_atomic};
use rustix::fs::{FileType, Mode, OFlags};
use serde::{Deserialize, Serialize};

use crate::sandbox::fs::DaemonFs;

/// The store's directory under the data root.
pub(crate) const FILE_BACKUPS_DIR: &str = "file-backups";

/// The file of an entry that describes its copy.
pub(crate) const META_FILE: &str = "meta.json";

/// The largest file that efrd copies, as `write_file` keeps the original of a file.
pub(crate) const MAX_BACKUP_BYTES: usize = 16 * 1024 * 1024;

/// The most entries of one turn.
const MAX_ENTRIES: u32 = 10_000;

/// What `meta.json` says about one copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Meta {
    /// The file as the call named it.
    pub(crate) path: PathBuf,
    /// Each symbolic link on the way from `path` to `real`, where it lies.
    pub(crate) links: Vec<PathBuf>,
    /// The file with every link followed.
    pub(crate) real: PathBuf,
    /// Its mode before the call; absent when it was missing.
    pub(crate) mode: Option<u32>,
    /// True when the file did not exist before the call.
    pub(crate) missing: bool,
}

/// One file to copy before a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    /// The file as the exit names it.
    pub(crate) path: PathBuf,
    /// True when it is a floor.
    pub(crate) floor: bool,
    /// The same file below a dotfile link, when the exit names the real file behind
    /// the link ([`ExitNeed::link`](efr_permissions::ExitNeed::link)): the path that
    /// the user knows, which names the copy when it leads to the same real file.
    pub(crate) link: Option<PathBuf>,
}

/// The content and the mode of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Content {
    bytes: Vec<u8>,
    mode: u32,
}

/// One copy that efrd took before a call, until [`finish`] compares it with the file.
#[derive(Debug)]
pub(crate) struct Taken {
    path: PathBuf,
    real: PathBuf,
    entry: PathBuf,
    backup: PathBuf,
    before: Option<Content>,
    floor: bool,
}

/// Copies each of `targets` into the store `dir`, below the conversation and the turn.
/// A target that is a directory has no copy. `refused` holds the secrets and masks.
/// `Err` is the text that the model reads: the call does not run. It blocks.
pub(crate) fn take(
    dir: &Path,
    conversation: ConversationId,
    turn: TurnId,
    targets: &[Target],
    refused: &[PathBuf],
) -> Result<Vec<Taken>, String> {
    let mut taken: Vec<Taken> = Vec::new();
    for target in targets {
        match take_one(dir, conversation, turn, target, refused, &taken) {
            Ok(Some(one)) => taken.push(one),
            Ok(None) => {}
            Err(reason) => {
                for one in taken {
                    remove_entry(&one.entry);
                }
                return Err(reason);
            }
        }
    }
    Ok(taken)
}

fn take_one(
    dir: &Path,
    conversation: ConversationId,
    turn: TurnId,
    target: &Target,
    refused: &[PathBuf],
    taken: &[Taken],
) -> Result<Option<Taken>, String> {
    let path = &target.path;
    let failed = |reason: &dyn std::fmt::Display| {
        format!(
            "efr could not copy {} into its backup store before the change, so the call did \
             not run: {reason}",
            path.display()
        )
    };
    let resolved = resolve(&DaemonFs, path).map_err(|error| failed(&error))?;
    let real = resolved.path;
    if refused.iter().any(|mask| is_within(path, mask) || is_within(&real, mask)) {
        return Err(format!(
            "efr does not copy {}: it is a secret or a masked file. The call did not run.",
            path.display()
        ));
    }
    if taken.iter().any(|one| one.real == real) {
        return Ok(None);
    }
    let before = match read(&real) {
        Ok(Read::File(content)) => Some(content),
        Ok(Read::Missing) => None,
        // NOTE: the store keeps files; the engine never asks for a copy of a directory
        // when it knows the target's kind.
        Ok(Read::Other) => return Ok(None),
        Err(error) => return Err(failed(&error)),
    };
    // NOTE: the copy is of the file that the exit names; the link only names it, and
    // only while it still leads to that file.
    let (shown, links) = linked(target, &real).unwrap_or_else(|| (path.clone(), resolved.links));
    let Some(name) = shown.file_name() else {
        return Err(failed(&"the path names no file"));
    };
    let entry = new_entry(dir, conversation, turn).map_err(|error| failed(&error))?;
    let meta = Meta {
        path: shown.clone(),
        links,
        real: real.clone(),
        mode: before.as_ref().map(|content| content.mode),
        missing: before.is_none(),
    };
    let written = write_entry(&entry, name, &meta, before.as_ref());
    let backup = match written {
        Ok(backup) => backup,
        Err(error) => {
            remove_entry(&entry);
            return Err(failed(&error));
        }
    };
    Ok(Some(Taken { path: shown, real, entry, backup, before, floor: target.floor }))
}

/// The link of `target` with each link on its way, when it leads to `real`: a new file
/// under a dotfile link is named by the link, such as `~/.config/hypr/x.conf`, not by
/// `~/dotfiles/hypr/x.conf`. `None` without a link or when it leads elsewhere.
fn linked(target: &Target, real: &Path) -> Option<(PathBuf, Vec<PathBuf>)> {
    let link = target.link.as_ref()?;
    let resolved = resolve(&DaemonFs, link).ok()?;
    (resolved.path == real).then(|| (link.clone(), resolved.links))
}

/// Writes the copy and `meta.json` into `entry`, and returns the backup: the copy, or
/// `meta.json` for a file that was missing.
fn write_entry(
    entry: &Path,
    name: &OsStr,
    meta: &Meta,
    before: Option<&Content>,
) -> io::Result<PathBuf> {
    let bytes = serde_json::to_vec_pretty(meta).map_err(io::Error::other)?;
    let meta_path = entry.join(META_FILE);
    let backup = match before {
        Some(content) => {
            let copy = entry.join(name);
            write_atomic(&copy, &content.bytes).map_err(io::Error::other)?;
            copy
        }
        None => meta_path.clone(),
    };
    write_atomic(&meta_path, &bytes).map_err(io::Error::other)?;
    Ok(backup)
}

/// Makes `<dir>/<conversation>/<turn>/<n>` with the first free `n` from 1, each
/// directory with mode 0700.
fn new_entry(dir: &Path, conversation: ConversationId, turn: TurnId) -> io::Result<PathBuf> {
    let conversation_dir = dir.join(conversation.to_string());
    let turn_dir = conversation_dir.join(turn.to_string());
    // NOTE: another call of the turn may drop an empty turn dir meanwhile; then the
    // dirs are made once more.
    let mut tries = 2;
    loop {
        for level in [dir, &conversation_dir, &turn_dir] {
            if claim_dir(level).map_err(io::Error::other)? == Claim::Taken
                && !fs::symlink_metadata(level)?.is_dir()
            {
                return Err(io::Error::other(format!("{} is not a directory", level.display())));
            }
        }
        for n in 1..=MAX_ENTRIES {
            let entry = turn_dir.join(n.to_string());
            match claim_dir(&entry) {
                Ok(Claim::Claimed) => return Ok(entry),
                Ok(Claim::Taken) => {}
                Err(_) if tries > 1 && !turn_dir.is_dir() => break,
                Err(error) => return Err(io::Error::other(error)),
            }
            if n == MAX_ENTRIES {
                return Err(io::Error::other("the turn has too many backups"));
            }
        }
        tries -= 1;
    }
}

/// Compares each copy with its file after the call: an unchanged file drops its entry,
/// and a changed one gives a [`FileBackup`]. A file that efrd cannot read again counts
/// as changed. It blocks.
pub(crate) fn finish(taken: Vec<Taken>) -> Vec<FileBackup> {
    let mut backups = Vec::new();
    for one in taken {
        let now = resolve(&DaemonFs, &one.path).ok().map(|resolved| resolved.path);
        let after = match now.as_deref().map(read) {
            Some(Ok(Read::File(content))) => Some(Some(content)),
            Some(Ok(Read::Missing)) => Some(None),
            _ => None,
        };
        let unchanged = now.as_ref() == Some(&one.real) && after.as_ref() == Some(&one.before);
        if unchanged {
            remove_entry(&one.entry);
            continue;
        }
        let real = (one.real != one.path).then_some(one.real);
        backups.push(FileBackup { path: one.path, backup: one.backup, floor: one.floor, real });
    }
    backups
}

/// What a read of a file found.
enum Read {
    File(Content),
    Missing,
    /// A directory or another kind that the store does not keep.
    Other,
}

/// Reads the real file `real` from its parent, which opens with no link anywhere on its
/// path, with `O_NOFOLLOW`; at most [`MAX_BACKUP_BYTES`].
fn read(real: &Path) -> io::Result<Read> {
    let (Some(parent), Some(name)) = (real.parent(), real.file_name()) else {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    };
    let parent = match DaemonFs::open(parent, OFlags::RDONLY | OFlags::DIRECTORY) {
        Ok(parent) => parent,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Read::Missing),
        Err(error) => return Err(error),
    };
    // NOTE: O_NONBLOCK, so a fifo planted where the file was cannot stall efrd.
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let fd = match rustix::fs::openat(&parent, name, flags, Mode::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(Read::Missing),
        // NOTE: a socket cannot be opened.
        Err(rustix::io::Errno::NXIO) => return Ok(Read::Other),
        Err(errno) => return Err(errno.into()),
    };
    let stat = rustix::fs::fstat(&fd)?;
    let kind = FileType::from_raw_mode(stat.st_mode);
    if kind != FileType::RegularFile {
        return Ok(Read::Other);
    }
    let mut bytes = Vec::new();
    let cap = u64::try_from(MAX_BACKUP_BYTES).unwrap_or(u64::MAX).saturating_add(1);
    fs::File::from(fd).take(cap).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BACKUP_BYTES {
        return Err(io::Error::other(format!(
            "the file is larger than {} MiB",
            MAX_BACKUP_BYTES / 1024 / 1024
        )));
    }
    Ok(Read::File(Content { bytes, mode: stat.st_mode & 0o7777 }))
}

/// Removes one entry and, when they are empty now, its turn and conversation dirs.
fn remove_entry(entry: &Path) {
    if let Err(error) = fs::remove_dir_all(entry) {
        tracing::warn!(entry = %entry.display(), error = %error, "a floor file backup could not be removed");
        return;
    }
    for dir in entry.ancestors().skip(1).take(2) {
        if fs::remove_dir(dir).is_err() {
            break;
        }
    }
}

/// Removes the backups of all but the newest `keep_turns` turns of each conversation,
/// and those older than `max_age` (`snapshot.keep_turns`, `snapshot.max_age_days`).
/// The snapshot collector of `run.rs` calls it at each round, with the time of the
/// daemon's clock.
pub(crate) async fn collect(dir: &Path, keep_turns: usize, max_age: Duration, now: SystemTime) {
    let dir = dir.to_path_buf();
    let removed = tokio::task::spawn_blocking(move || collect_at(&dir, keep_turns, max_age, now));
    match removed.await {
        Ok(0) => {}
        Ok(removed) => tracing::debug!(removed, "collected old floor file backups"),
        Err(_) => tracing::warn!("collecting the floor file backups panicked"),
    }
}

/// [`collect`] at the time `now`: returns how many turn dirs it removed. A turn's time is
/// the time in its id (a version 7 UUID), else the time its dir last changed. A name
/// that is not an id of efr stays. It blocks.
pub(crate) fn collect_at(
    dir: &Path,
    keep_turns: usize,
    max_age: Duration,
    now: SystemTime,
) -> usize {
    let oldest = now.checked_sub(max_age).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0;
    let Ok(conversations) = fs::read_dir(dir) else { return 0 };
    for conversation in conversations.flatten() {
        let path = conversation.path();
        let is_id = conversation
            .file_name()
            .to_str()
            .is_some_and(|name| name.parse::<ConversationId>().is_ok());
        if !is_id || !conversation.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(turns) = fs::read_dir(&path) else { continue };
        let mut turns: Vec<(SystemTime, PathBuf)> = turns
            .flatten()
            .filter(|turn| turn.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|turn| {
                let id = turn.file_name().to_str()?.parse::<TurnId>().ok()?;
                let time = turn_time(&id).or_else(|| turn.metadata().ok()?.modified().ok())?;
                Some((time, turn.path()))
            })
            .collect();
        turns.sort_by(|left, right| right.cmp(left));
        for (n, (time, turn)) in turns.iter().enumerate() {
            if n < keep_turns && *time >= oldest {
                continue;
            }
            match fs::remove_dir_all(turn) {
                Ok(()) => removed += 1,
                Err(error) => {
                    tracing::warn!(dir = %turn.display(), error = %error, "old floor file backups could not be removed");
                }
            }
        }
        // NOTE: fails while the conversation keeps a turn, which is the point.
        let _ = fs::remove_dir(&path);
    }
    removed
}

/// The time in the version 7 UUID of `turn`, when it has one.
fn turn_time(turn: &TurnId) -> Option<SystemTime> {
    let (seconds, nanos) = turn.as_uuid().get_timestamp()?.to_unix();
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
}

#[cfg(test)]
mod tests;
