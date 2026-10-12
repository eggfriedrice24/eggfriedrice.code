//! The home copies (efr's auto spec 10.8, phase B): before efrd runs a call whose
//! approved exits write outside the snapshot roots, such as `~/.zshrc` or
//! `~/Documents/notes.md`, it copies each target into the home store of `efr-snapshot`
//! (`$XDG_DATA_HOME/efr/snapshots/home.git`), so `efr undo` can bring it back.
//!
//! What it copies comes from [`ExitNeed::copies`](efr_permissions::ExitNeed::copies):
//! a file ([`CoverAsk::Home`]) with the links on its way, or a directory with what it
//! holds ([`CoverAsk::HomeDir`]), within `snapshot.home_max_files` files and
//! `snapshot.home_max_mib` MiB. efrd reads every path itself, never through a link
//! (`DaemonFs`), and hands the entries to the store one at a time, so at most one file
//! is in memory. After the call it reads the same paths again, lists each directory
//! that it walked again (a new file there was missing before, so undo moves it to the
//! undo trash), and the store says what changed.
//!
//! Failure rule (plan PB14): a read error stops the call. A floor or persistence file
//! that efrd cannot copy (masked, larger than 16 MiB) also stops it, because the
//! reviewer allowed it only with a copy. Any other path over a limit, and a masked
//! path, runs with no copy: the record and the end of the turn say "not covered". A
//! secret or a mask is never read.
//!
//! [`coverage`] answers the same questions before the decision, for the facts of a
//! call ([`CallFacts::covered`](efr_permissions::CallFacts::covered)).
//!
//! A named change of the crontab or of the linger setting changes no file, so its copy
//! holds the state that the program that sets it reads (`home/state.rs`): the entries
//! `@crontab` and `@linger`, whose bytes come with the copy, not from a file.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use efr_config::Settings;
use efr_conversation::FileBackup;
use efr_permissions::{CoverAsk, Coverage, ExitNeed};
use efr_protocol::{ExitKind, UndoReason};
use efr_sandbox::{FsView as _, is_within, resolve};
use efr_snapshot::{
    HomeDelta, HomeEntry, HomeReader, HomeState, MAX_HOME_FILE_BYTES, SnapshotError,
};
use rustix::fs::{FileType, OFlags};

use crate::sandbox::fs::DaemonFs;

pub(crate) mod state;

/// The undo trash below efr's data root: `undo-trash/<undo-id>/`, where `efr undo`
/// puts each file that it overwrites or removes.
pub(crate) const UNDO_TRASH_DIR: &str = "undo-trash";

/// One MiB.
const MIB: u64 = 1024 * 1024;

/// How deep a walk of a directory goes.
const MAX_WALK_DEPTH: usize = 64;

/// The limits of the copy of one directory: `snapshot.home_max_files` and
/// `snapshot.home_max_mib`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HomeLimits {
    pub(crate) max_files: u64,
    pub(crate) max_bytes: u64,
}

impl HomeLimits {
    /// The limits of `settings`.
    pub(crate) fn of(settings: &Settings) -> HomeLimits {
        HomeLimits {
            max_files: u64::from(settings.snapshot.home_max_files),
            max_bytes: u64::from(settings.snapshot.home_max_mib).saturating_mul(MIB),
        }
    }
}

/// One path that an approved exit asks efrd to copy before the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    /// [`CoverAsk::Home`] or [`CoverAsk::HomeDir`].
    pub(crate) ask: CoverAsk,
    /// True for a floor or persistence file.
    pub(crate) floor: bool,
    /// The path that the user knows, when the exit names the real file behind a
    /// dotfile link ([`ExitNeed::link`]).
    pub(crate) link: Option<PathBuf>,
}

/// What the approved exits `exits` ask efrd to copy, each ask once. Asks of other kinds
/// (units, cron, linger) are not copies of files.
pub(crate) fn targets(exits: &[ExitNeed]) -> Vec<Target> {
    let mut targets: Vec<Target> = Vec::new();
    for need in exits {
        let floor = need.kind == ExitKind::Persistence;
        for ask in &need.copies {
            let link = match ask {
                CoverAsk::Home(path) if Some(path) == need.target.as_ref() => need.link.clone(),
                CoverAsk::Home(_) | CoverAsk::HomeDir(_) => None,
                _ => continue,
            };
            match targets.iter_mut().find(|known| known.ask == *ask) {
                Some(known) => {
                    known.floor |= floor;
                    known.link = known.link.take().or(link);
                }
                None => targets.push(Target { ask: ask.clone(), floor, link }),
            }
        }
    }
    targets
}

/// True when the home ask `ask` names a masked path strictly inside one of `roots`:
/// the home store never copies it, and the snapshot of that root holds it.
pub(crate) fn root_holds(ask: &CoverAsk, roots: &[PathBuf], masks: &[PathBuf]) -> bool {
    match ask {
        CoverAsk::Home(path) | CoverAsk::HomeDir(path) => {
            masked(path, masks) && roots.iter().any(|root| path.starts_with(root) && path != root)
        }
        _ => false,
    }
}

/// The entries of one copy, before efrd reads the bytes of their files.
#[derive(Debug, Clone, Default)]
pub(crate) struct Prepared {
    /// The entries, in the order the store gets them.
    pub(crate) entries: Vec<HomeEntry>,
    /// The directories that the copy walked whole, which the copy after the call lists
    /// again.
    pub(crate) walked: Vec<PathBuf>,
    /// Each real file that the user knows by a link: `(real, link)`.
    pub(crate) shown: Vec<(PathBuf, PathBuf)>,
    /// Each target that no copy covers, with why, for the end of the turn.
    pub(crate) not_covered: Vec<(PathBuf, UndoReason)>,
    /// The bytes of each state entry (`@crontab`, `@linger`), which no file holds.
    pub(crate) states: Vec<(PathBuf, Vec<u8>)>,
    /// What `lstat` said of each path of a walked directory that no copy covers
    /// (masked, over the size cap, special), so a copy after the call names it only
    /// when it changed.
    pub(crate) stamps: Vec<(PathBuf, Stamp)>,
}

/// What `lstat` says of a path that no copy holds: its inode, size, change and status
/// times. Its bytes are never read.
pub(crate) type Stamp = (u64, u64, i64, i64, i64, i64);

/// The [`Stamp`] of `meta`.
fn stamp_of(meta: &fs::Metadata) -> Stamp {
    use std::os::unix::fs::MetadataExt as _;
    (meta.ino(), meta.size(), meta.mtime(), meta.mtime_nsec(), meta.ctime(), meta.ctime_nsec())
}

impl Prepared {
    fn push(&mut self, entry: HomeEntry) {
        if let HomeState::NotCovered(reason) = entry.state
            && !self.not_covered.iter().any(|(path, _)| *path == entry.path)
        {
            self.not_covered.push((entry.path.clone(), reason));
        }
        if !self.entries.iter().any(|known| known.path == entry.path) {
            self.entries.push(entry);
        }
    }

    /// Adds the state entry `entry` with its bytes, which a command read.
    pub(crate) fn push_state(&mut self, entry: HomeEntry, bytes: Option<Vec<u8>>) {
        if let Some(bytes) = bytes {
            self.states.retain(|(path, _)| *path != entry.path);
            self.states.push((entry.path.clone(), bytes));
        }
        self.entries.retain(|known| known.path != entry.path);
        self.push(entry);
    }

    /// True when the copy covers the target `ask`: no entry for it says "not covered".
    pub(crate) fn covers(&self, ask: &CoverAsk) -> bool {
        let path = match ask {
            CoverAsk::Home(path) | CoverAsk::HomeDir(path) => path,
            _ => return false,
        };
        !self.not_covered.iter().any(|(known, _)| {
            known == path || (matches!(ask, CoverAsk::HomeDir(_)) && known.starts_with(path))
        })
    }
}

/// Reads what the copy of `targets` holds now, without the bytes. `masks` are the
/// secrets and masks that efrd never copies. `Err` is the text that the model reads:
/// the call does not run. It blocks.
pub(crate) fn prepare(
    targets: &[Target],
    masks: &[PathBuf],
    limits: HomeLimits,
) -> Result<Prepared, String> {
    let mut prepared = Prepared::default();
    for target in targets {
        match &target.ask {
            CoverAsk::Home(path) => file_entries(&mut prepared, target, path, masks)?,
            CoverAsk::HomeDir(dir) => {
                dir_entries(&mut prepared, dir, target.floor, masks, limits, true)?;
            }
            _ => {}
        }
    }
    Ok(prepared)
}

/// The entries of the copy after the call: the files of `before` again, and each
/// directory that it walked, listed again. It blocks.
pub(crate) fn after(before: &Prepared, masks: &[PathBuf], limits: HomeLimits) -> Prepared {
    let mut prepared = Prepared { shown: before.shown.clone(), ..Prepared::default() };
    for entry in &before.entries {
        // NOTE: a state entry (`@crontab`) names no file; the caller reads it again.
        if !entry.path.is_absolute() || before.walked.iter().any(|dir| entry.path.starts_with(dir))
        {
            continue;
        }
        let state = match entry.state {
            HomeState::NotCovered(reason) => HomeState::NotCovered(reason),
            _ => state_of(&entry.path, masks).unwrap_or(HomeState::NotCovered(UndoReason::Unknown)),
        };
        prepared.push(HomeEntry { state, ..entry.clone() });
    }
    // NOTE: a file of a walked dir that the call removed is missing now; the store must
    // hear so, or it could not tell a removal from a path that no copy covers.
    for entry in &before.entries {
        if before.walked.iter().any(|dir| entry.path.starts_with(dir))
            && !matches!(entry.state, HomeState::NotCovered(_))
            && fs::symlink_metadata(&entry.path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            prepared.push(HomeEntry { state: HomeState::Missing, ..entry.clone() });
        }
    }
    for dir in &before.walked {
        let floor = before.entries.iter().any(|entry| entry.floor && entry.path.starts_with(dir));
        if dir_entries(&mut prepared, dir, floor, masks, limits, false).is_err() {
            prepared.push(entry(dir, HomeState::NotCovered(UndoReason::Unknown), floor));
        }
    }
    prepared
}

/// The entries of the file target `path`: each link on the way from the path that the
/// user knows, then the real file.
fn file_entries(
    prepared: &mut Prepared,
    target: &Target,
    path: &Path,
    masks: &[PathBuf],
) -> Result<(), String> {
    let failed = |reason: &dyn std::fmt::Display| {
        format!(
            "efr could not copy {} into its snapshot store before the change, so the call did \
             not run: {reason}",
            path.display()
        )
    };
    let known = target.link.as_deref().unwrap_or(path);
    let resolved = resolve(&DaemonFs, known).map_err(|error| failed(&error))?;
    // NOTE: the link only names the file, and only while it still leads to it.
    let (resolved, named) = if target.link.is_some() && resolved.path != path {
        (resolve(&DaemonFs, path).map_err(|error| failed(&error))?, path)
    } else {
        (resolved, known)
    };
    let real = resolved.path;
    if masked(named, masks) || masked(&real, masks) {
        if target.floor {
            return Err(format!(
                "efr does not copy {}: it is a secret or a masked file. The call did not run.",
                named.display()
            ));
        }
        prepared.push(entry(&real, HomeState::NotCovered(UndoReason::Masked), target.floor));
        return Ok(());
    }
    for link in &resolved.links {
        let state = match fs::read_link(link) {
            Ok(to) => HomeState::Link { target: os_bytes(to.into_os_string()) },
            Err(error) => return Err(failed(&error)),
        };
        prepared.push(entry(link, state, target.floor));
    }
    if named != real {
        prepared.shown.push((real.clone(), named.to_path_buf()));
    }
    let state = match state_of(&real, masks) {
        Ok(HomeState::NotCovered(UndoReason::OverLimit)) if target.floor => {
            return Err(format!(
                "efr could not copy {}: it is larger than {} MiB. The call did not run.",
                named.display(),
                MAX_HOME_FILE_BYTES / MIB
            ));
        }
        Ok(state) => state,
        Err(error) => return Err(failed(&error)),
    };
    let mut real_entry = entry(&real, state, target.floor);
    real_entry.chain = resolved.links;
    prepared.push(real_entry);
    Ok(())
}

/// The state of `path` for a copy, without following a link at its end: a file within
/// [`MAX_HOME_FILE_BYTES`], a link, missing, or not covered.
fn state_of(path: &Path, masks: &[PathBuf]) -> std::io::Result<HomeState> {
    if masked(path, masks) {
        return Ok(HomeState::NotCovered(UndoReason::Masked));
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HomeState::Missing);
        }
        Err(error) => return Err(error),
    };
    let kind = meta.file_type();
    Ok(if kind.is_symlink() {
        HomeState::Link { target: os_bytes(fs::read_link(path)?.into_os_string()) }
    } else if kind.is_file() && meta.len() > MAX_HOME_FILE_BYTES {
        HomeState::NotCovered(UndoReason::OverLimit)
    } else if kind.is_file() {
        HomeState::File {
            mode: std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o7777,
        }
    } else {
        HomeState::NotCovered(UndoReason::Unknown)
    })
}

/// The entries of the directory `dir` and of what it holds, within `limits`. Over the
/// limits, the directory itself is "not covered" and none of its files is read. With
/// `record`, a directory that it walks joins [`Prepared::walked`].
fn dir_entries(
    prepared: &mut Prepared,
    dir: &Path,
    floor: bool,
    masks: &[PathBuf],
    limits: HomeLimits,
    record: bool,
) -> Result<(), String> {
    if masked(dir, masks) {
        prepared.push(entry(dir, HomeState::NotCovered(UndoReason::Masked), floor));
        return Ok(());
    }
    match fs::symlink_metadata(dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if record && !prepared.walked.contains(&dir.to_path_buf()) {
                prepared.walked.push(dir.to_path_buf());
            }
            return Ok(());
        }
        Err(error) => {
            return Err(format!(
                "efr could not read {} before the change, so the call did not run: {error}",
                dir.display()
            ));
        }
        Ok(meta) if !meta.is_dir() => {
            let state = state_of(dir, masks).map_err(|error| error.to_string())?;
            prepared.push(entry(dir, state, floor));
            return Ok(());
        }
        Ok(_) => {}
    }
    match walk(dir, masks, limits) {
        Walk::Over => {
            prepared.push(entry(dir, HomeState::NotCovered(UndoReason::OverLimit), floor));
        }
        Walk::Failed(error) => {
            return Err(format!(
                "efr could not read {} before the change, so the call did not run: {error}",
                dir.display()
            ));
        }
        Walk::Within(found) => {
            for (path, state, stamp) in found {
                if let Some(stamp) = stamp
                    && !prepared.stamps.iter().any(|(known, _)| *known == path)
                {
                    prepared.stamps.push((path.clone(), stamp));
                }
                prepared.push(entry(&path, state, floor));
            }
            if record && !prepared.walked.contains(&dir.to_path_buf()) {
                prepared.walked.push(dir.to_path_buf());
            }
        }
    }
    Ok(())
}

/// What a bounded walk of a directory found.
enum Walk {
    /// Each file and link below it, with its state; a masked path is "not covered".
    /// A path that no copy covers has the [`Stamp`] of its `lstat`.
    Within(Vec<(PathBuf, HomeState, Option<Stamp>)>),
    /// More files or bytes than the limits.
    Over,
    /// A directory could not be read.
    Failed(std::io::Error),
}

/// Walks `dir` with no link followed, and stops at more than `limits` allow.
fn walk(dir: &Path, masks: &[PathBuf], limits: HomeLimits) -> Walk {
    let mut found = Vec::new();
    let (mut files, mut bytes) = (0_u64, 0_u64);
    let mut todo = vec![(dir.to_path_buf(), 0_usize)];
    while let Some((at, depth)) = todo.pop() {
        let names = match DaemonFs.read_dir(&at) {
            Ok(names) => names,
            Err(error) => return Walk::Failed(error),
        };
        for name in names {
            let path = at.join(name);
            if masked(&path, masks) {
                // NOTE: `lstat` only, so a change shows; a mask's bytes are never read.
                let stamp = fs::symlink_metadata(&path).ok().map(|meta| stamp_of(&meta));
                found.push((path, HomeState::NotCovered(UndoReason::Masked), stamp));
                continue;
            }
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Walk::Failed(error),
            };
            let kind = meta.file_type();
            if kind.is_dir() {
                if depth + 1 >= MAX_WALK_DEPTH {
                    return Walk::Over;
                }
                todo.push((path, depth + 1));
                continue;
            }
            files += 1;
            if kind.is_file() {
                bytes = bytes.saturating_add(meta.len());
            }
            if files > limits.max_files || bytes > limits.max_bytes {
                return Walk::Over;
            }
            let state = if kind.is_symlink() {
                match fs::read_link(&path) {
                    Ok(to) => HomeState::Link { target: os_bytes(to.into_os_string()) },
                    Err(error) => return Walk::Failed(error),
                }
            } else if kind.is_file() && meta.len() > MAX_HOME_FILE_BYTES {
                HomeState::NotCovered(UndoReason::OverLimit)
            } else if kind.is_file() {
                HomeState::File {
                    mode: std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o7777,
                }
            } else {
                HomeState::NotCovered(UndoReason::Unknown)
            };
            let stamp = matches!(state, HomeState::NotCovered(_)).then(|| stamp_of(&meta));
            found.push((path, state, stamp));
        }
    }
    Walk::Within(found)
}

/// Whether the home store covers `ask` now, for the facts of a call: a file that is
/// regular or missing, at most 16 MiB and not masked; a directory within `limits`
/// with no mask in it. It blocks.
pub(crate) fn coverage(ask: &CoverAsk, masks: &[PathBuf], limits: HomeLimits) -> Coverage {
    let target = Target { ask: ask.clone(), floor: false, link: None };
    let prepared = match ask {
        CoverAsk::Home(_) | CoverAsk::HomeDir(_) => prepare(&[target], masks, limits),
        _ => return Coverage::Unknown,
    };
    let Ok(prepared) = prepared else { return Coverage::Unknown };
    match prepared.not_covered.iter().map(|(_, reason)| *reason).next() {
        None => Coverage::Covered,
        Some(UndoReason::Masked) => Coverage::Masked,
        Some(UndoReason::OverLimit) => Coverage::OverLimit,
        Some(_) => Coverage::NotAllowed,
    }
}

/// The entries of a copy as the store reads them: a file's bytes are read when the
/// store asks for its entry, through `DaemonFs`, never through a link.
pub(crate) struct Reader {
    entries: VecDeque<HomeEntry>,
    states: Vec<(PathBuf, Vec<u8>)>,
}

impl Reader {
    pub(crate) fn new(prepared: &Prepared) -> Reader {
        Reader {
            entries: prepared.entries.iter().cloned().collect(),
            states: prepared.states.clone(),
        }
    }
}

impl HomeReader for Reader {
    fn next(
        &mut self,
    ) -> Option<Result<(HomeEntry, Option<Box<dyn std::io::Read + Send>>), SnapshotError>> {
        let entry = self.entries.pop_front()?;
        if matches!(entry.state, HomeState::State) {
            let at = self.states.iter().position(|(path, _)| *path == entry.path);
            let bytes = at.map(|at| self.states.swap_remove(at).1);
            let bytes = bytes.map(|bytes| {
                Box::new(std::io::Cursor::new(bytes)) as Box<dyn std::io::Read + Send>
            });
            return Some(Ok((entry, bytes)));
        }
        if !matches!(entry.state, HomeState::File { .. }) {
            return Some(Ok((entry, None)));
        }
        // NOTE: O_NONBLOCK, so a fifo planted after the walk cannot stall efrd.
        let opened =
            DaemonFs::open(&entry.path, OFlags::RDONLY | OFlags::NONBLOCK).and_then(|fd| {
                let stat = rustix::fs::fstat(&fd)?;
                if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                    return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
                }
                Ok(fs::File::from(fd))
            });
        match opened {
            Ok(file) => {
                let bytes: Box<dyn std::io::Read + Send> =
                    Box::new(file.take(MAX_HOME_FILE_BYTES + 1));
                Some(Ok((entry, Some(bytes))))
            }
            Err(source) => Some(Err(SnapshotError::Io { path: entry.path.clone(), source })),
        }
    }
}

/// The changed paths of a copy as the turn reports them, each with the path that the
/// user knows (`~/.zshrc` for the real file behind it) and why no copy covers it. A
/// path of a walked directory that no copy covers counts only when its `lstat` changed,
/// so an untouched large file there is not named at each copy.
/// The `backup` path is empty: the copy is in the home store, which has no path of its
/// own for one file (efr-conversation's `FileBackup`).
pub(crate) fn changed(
    deltas: &[HomeDelta],
    before: &Prepared,
    after: &Prepared,
) -> Vec<FileBackup> {
    let reason = |path: &Path| {
        before
            .not_covered
            .iter()
            .chain(&after.not_covered)
            .find(|(known, _)| known == path)
            .map(|(_, reason)| *reason)
            .unwrap_or(UndoReason::Unknown)
    };
    let stamp = |prepared: &Prepared, path: &Path| {
        prepared.stamps.iter().find(|(known, _)| known == path).map(|(_, stamp)| *stamp)
    };
    let unchanged = |delta: &HomeDelta| {
        !delta.covered
            && stamp(before, &delta.path).is_some()
            && stamp(before, &delta.path) == stamp(after, &delta.path)
    };
    deltas
        .iter()
        .filter(|delta| delta.changed && !unchanged(delta))
        .map(|delta| {
            let link =
                before.shown.iter().find(|(real, _)| *real == delta.path).map(|(_, link)| link);
            FileBackup {
                path: link.cloned().unwrap_or_else(|| delta.path.clone()),
                backup: PathBuf::new(),
                floor: delta.floor,
                real: link.map(|_| delta.path.clone()),
                reason: (!delta.covered).then(|| reason(&delta.path)),
            }
        })
        .collect()
}

/// Removes each undo of the undo trash `dir` older than `max_age` at `now`
/// (`snapshot.max_age_days`): its time is the time in its id (`u-<uuid v7>`), else the
/// time its directory last changed. A name that is not an undo id stays. Returns how
/// many it removed. It blocks.
pub(crate) fn collect_trash(dir: &Path, max_age: Duration, now: SystemTime) -> usize {
    let oldest = now.checked_sub(max_age).unwrap_or(SystemTime::UNIX_EPOCH);
    let Ok(undos) = fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for undo in undos.flatten() {
        let name = undo.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_prefix("u-")) else { continue };
        let Ok(id) = id.parse::<efr_protocol::UndoId>() else { continue };
        if !undo.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let time = id
            .as_uuid()
            .get_timestamp()
            .map(|stamp| stamp.to_unix())
            .and_then(|(seconds, nanos)| {
                SystemTime::UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
            })
            .or_else(|| undo.metadata().ok()?.modified().ok());
        if time.is_some_and(|time| time >= oldest) {
            continue;
        }
        match fs::remove_dir_all(undo.path()) {
            Ok(()) => removed += 1,
            Err(error) => {
                tracing::warn!(dir = %undo.path().display(), error = %error, "an old undo trash could not be removed");
            }
        }
    }
    removed
}

fn entry(path: &Path, state: HomeState, floor: bool) -> HomeEntry {
    HomeEntry { path: path.to_path_buf(), state, floor, chain: Vec::new() }
}

/// True when `path` lies at or below one of `masks`.
pub(crate) fn masked(path: &Path, masks: &[PathBuf]) -> bool {
    masks.iter().any(|mask| is_within(path, mask))
}

fn os_bytes(text: OsString) -> Vec<u8> {
    std::os::unix::ffi::OsStringExt::into_vec(text)
}

#[cfg(test)]
mod tests;
