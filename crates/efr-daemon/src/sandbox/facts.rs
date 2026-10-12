//! The facts about a shell line's files and programs that the engine reads in `auto`
//! (efr's auto spec, section 7.2): whether each target exists and what it is, how many
//! files git tracks below each `rm -r` directory, and where each program word leads and
//! whether that file changed in this turn: its modification time (mtime) or change
//! time (ctime) at or after the turn's start, or the start of a background job of the conversation that ran since
//! the previous turn started (`jobs/since.rs`). A word that the shell runs itself, a zsh
//! builtin or reserved word such as `:` or `cd` ([`FactRequest::builtins`]), leads to
//! [`BUILTIN`], not to a file. `efr-permissions` reads no file, so efrd collects them
//! before the engine decides.
//!
//! A fact that cannot be found is left out, and the engine assumes the stricter case:
//! the target exists, the directory holds tracked files. So a lost fact asks, and never
//! runs more.
//!
//! For a persistence form whose content is another file (plan PB19a), it also reads
//! each source that the engine names ([`FactRequest::sources`]): the real file behind
//! every link, whether it changed in this turn (its mtime or ctime at or after the
//! turn's start), and its text when it is UTF-8 and at most 8 KiB. A unit name
//! leads to its file in the user unit dirs (`sandbox/units.rs`). A mask or a secret is
//! never read.

use std::path::{Path, PathBuf};
use std::time::Duration;

use efr_permissions::{CallFacts, FactRequest, Source, SourceFact, TargetKind};
use efr_protocol::ActionFacts;
use efr_scope::{Git, Home};
use jiff::Timestamp;
use rustix::fs::{FileType, OFlags};

/// How long one git run for the facts may take; a slower one leaves its fact out.
const GIT_TIMEOUT: Duration = Duration::from_millis(200);

/// The flags of every git run of efrd on a repository that the model can write: a
/// planted fsmonitor or hook never runs.
pub(crate) const HARDENED: &[&str] = &[
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.untrackedCache=false",
];

/// Where a program word leads that the shell runs itself, a builtin or a reserved
/// word: a relative path, which names no file, as `ProgramFact::resolved` says.
pub(crate) const BUILTIN: &str = "builtin";

/// What the collection needs besides the line's request.
pub(crate) struct FactInput<'a> {
    /// What the engine reads for the line.
    pub(crate) request: &'a FactRequest,
    /// The paths that the tool declared as writes.
    pub(crate) writes: &'a [PathBuf],
    /// The directory the line starts in.
    pub(crate) command_dir: Option<&'a Path>,
    /// The hidden shell's `PATH`, where a program word resolves.
    pub(crate) shell_path: &'a str,
    /// The names of directories that builds make again, which hold no tracked file
    /// that matters (`sandbox.rebuildable`).
    pub(crate) rebuildable: &'a [String],
    /// When the turn's first call was judged, or a background job of the conversation
    /// that ran since the previous turn started, if earlier (`jobs/since.rs`); a
    /// program changed after it changed in this turn.
    pub(crate) turn_start: Timestamp,
    /// The secrets and masks that efrd never reads for a source.
    pub(crate) masks: &'a [PathBuf],
}

/// The facts of one call.
pub(crate) async fn collect(input: &FactInput<'_>, git: &Git, home: &Home) -> CallFacts {
    let mut facts = CallFacts::default();
    let targets: Vec<PathBuf> = input.request.targets.iter().chain(input.writes).cloned().collect();
    let shell_path = input.shell_path.to_owned();
    let programs = input.request.programs.clone();
    let builtins = input.request.builtins.clone();
    let command_dir = input.command_dir.map(Path::to_path_buf);
    let turn_start = input.turn_start;
    let sources = input.request.sources.clone();
    let (masks, home_dir) = (input.masks.to_vec(), home.path().to_path_buf());
    let read = tokio::task::spawn_blocking(move || {
        sources.iter().map(|source| source_fact(source, &home_dir, &masks, turn_start)).collect()
    });
    let looked = tokio::task::spawn_blocking(move || {
        let mut found: Vec<(PathBuf, Option<TargetKind>)> = Vec::new();
        for target in &targets {
            for (path, kind) in target_and_parents(target) {
                if !found.iter().any(|(known, _)| *known == path) {
                    found.push((path, kind));
                }
            }
        }
        let programs: Vec<(String, Option<PathBuf>, bool)> = programs
            .iter()
            .map(|word| {
                if builtins.contains(word) {
                    return (word.clone(), Some(PathBuf::from(BUILTIN)), false);
                }
                let resolved = resolve_program(word, command_dir.as_deref(), &shell_path);
                let changed =
                    resolved.as_deref().is_some_and(|path| changed_since(path, turn_start));
                (word.clone(), resolved, changed)
            })
            .collect();
        (found, programs)
    })
    .await;
    if let Ok((targets, programs)) = looked {
        facts.targets = targets;
        facts.programs = programs;
    }
    facts.sources = read.await.unwrap_or_default();
    let git = git.clone().with_timeout(GIT_TIMEOUT);
    for dir in &input.request.tracked {
        let rebuildable = dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| input.rebuildable.iter().any(|known| known == name));
        if rebuildable {
            facts.tracked_counts.push((dir.clone(), 0));
        } else if let Some(count) = tracked_files(&git, home, dir).await {
            facts.tracked_counts.push((dir.clone(), count));
        }
    }
    facts
}

/// `target` with its kind, then each parent up to the first one that exists.
fn target_and_parents(target: &Path) -> Vec<(PathBuf, Option<TargetKind>)> {
    let mut out = Vec::new();
    let mut current = Some(target);
    while let Some(path) = current {
        let kind = kind_of(path);
        out.push((path.to_path_buf(), kind));
        if kind.is_some() {
            break;
        }
        current = path.parent();
    }
    out
}

/// What `path` is, without following a link at its end; `None` when it is missing.
fn kind_of(path: &Path) -> Option<TargetKind> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let kind = meta.file_type();
    Some(if kind.is_dir() {
        TargetKind::Dir
    } else if kind.is_file() {
        TargetKind::File
    } else if kind.is_symlink() {
        // NOTE: a link counts as what it reaches, as a write through it would.
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => TargetKind::Dir,
            Ok(meta) if meta.is_file() => TargetKind::File,
            _ => TargetKind::Other,
        }
    } else {
        TargetKind::Other
    })
}

/// Where the program `word` leads: a path with a `/` from `dir`, else the first match
/// in the absolute entries of `path`, with links followed.
pub(crate) fn resolve_program(word: &str, dir: Option<&Path>, path: &str) -> Option<PathBuf> {
    if word.contains('/') {
        let candidate = match word.strip_prefix('/') {
            Some(_) => PathBuf::from(word),
            None => dir?.join(word),
        };
        return std::fs::canonicalize(candidate).ok().filter(|found| found.is_file());
    }
    path.split(':')
        .filter(|entry| entry.starts_with('/'))
        .map(|entry| Path::new(entry).join(word))
        .find(|candidate| candidate.is_file())
        .map(|found| std::fs::canonicalize(&found).unwrap_or(found))
}

/// True when `path` was changed at or after `since`: its modification time (mtime) or
/// its change time (ctime). A process can set the mtime of a file it wrote back
/// (`touch -d`), but no process without privilege can set its ctime.
pub(crate) fn changed_since(path: &Path, since: Timestamp) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return true };
    match meta.modified().ok().and_then(|time| Timestamp::try_from(time).ok()) {
        Some(modified) => modified >= since || status_changed_since(&meta, since),
        None => true,
    }
}

/// What efrd finds for one source of a persistence form: the real file, whether it
/// changed at or after `since`, and its text, never for a path at or below `masks`. A
/// unit's text holds the text of each unit of its `Also=` too, which `enable` links as
/// well, and a change of one of them counts as a change of the unit.
fn source_fact(source: &Source, home: &Path, masks: &[PathBuf], since: Timestamp) -> SourceFact {
    let mut fact = SourceFact { source: source.clone(), path: None, changed: true, text: None };
    let named = match source {
        Source::File(path) => Some(path.clone()),
        Source::Unit(name) => super::units::unit_file(name, home),
    };
    let Some(named) = named else { return fact };
    let Some((real, changed, text)) = read_source(&named, masks, since) else { return fact };
    fact.changed = changed;
    fact.text = text;
    fact.path = Some(real);
    if let Source::Unit(name) = source {
        let Some(also) = super::units::also_units(name, home) else {
            fact.changed = true;
            return fact;
        };
        for other in also {
            let found = super::units::unit_file(&other, home)
                .and_then(|named| read_source(&named, masks, since));
            let Some((path, changed, text)) = found else {
                fact.changed = true;
                return fact;
            };
            fact.changed |= changed;
            fact.text = match (fact.text.take(), text) {
                (Some(mine), Some(theirs)) => {
                    Some(format!("{mine}\n# Also={other} ({})\n{theirs}", path.display()))
                }
                _ => None,
            };
        }
    }
    fact
}

/// The real file of the source `named`, whether it or a link on the way to it changed
/// at or after `since`, and its text; `None` for a mask, a path that is no regular
/// file, or one that efrd cannot read.
fn read_source(
    named: &Path,
    masks: &[PathBuf],
    since: Timestamp,
) -> Option<(PathBuf, bool, Option<String>)> {
    let real = std::fs::canonicalize(named).ok()?;
    if crate::home::masked(named, masks) || crate::home::masked(&real, masks) {
        return None;
    }
    let meta = std::fs::metadata(&real).ok()?;
    if !meta.is_file() {
        return None;
    }
    let changed = changed_since(&real, since)
        || status_changed_since(&meta, since)
        || link_changed_since(named, since);
    Some((real.clone(), changed, read_text(&real)))
}

/// True when `path` or a link on the way from it to its file was made or changed at or
/// after `since`: a link in a unit dir that a call made in this turn names an old
/// file, and only the link's own times tell. `true` when efrd cannot tell.
fn link_changed_since(path: &Path, since: Timestamp) -> bool {
    /// As many links as the kernel follows.
    const MAX_LINKS: usize = 40;
    let mut at = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        let Ok(meta) = std::fs::symlink_metadata(&at) else { return true };
        if !meta.file_type().is_symlink() {
            return false;
        }
        let modified = meta.modified().ok().and_then(|time| Timestamp::try_from(time).ok());
        if modified.is_none_or(|modified| modified >= since) || status_changed_since(&meta, since) {
            return true;
        }
        let Ok(target) = std::fs::read_link(&at) else { return true };
        at = match at.parent() {
            Some(parent) => parent.join(target),
            None => target,
        };
    }
    true
}

/// True when the change time (ctime) of the file of `meta` is at or after `since`: a
/// copy that kept an old mtime still has a new ctime.
fn status_changed_since(meta: &std::fs::Metadata, since: Timestamp) -> bool {
    // NOTE: whole seconds, so a change in the turn's first second counts too.
    std::os::unix::fs::MetadataExt::ctime(meta) >= since.as_second()
}

/// The text of the regular file `path` when it is UTF-8 and at most
/// [`ActionFacts::MAX_EDIT_BYTES`], read with no link on the way and without waiting on
/// a fifo.
fn read_text(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let fd = super::fs::DaemonFs::open(path, OFlags::RDONLY | OFlags::NONBLOCK).ok()?;
    let stat = rustix::fs::fstat(&fd).ok()?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return None;
    }
    let mut bytes = Vec::new();
    let limit = u64::try_from(ActionFacts::MAX_EDIT_BYTES).unwrap_or(u64::MAX) + 1;
    std::fs::File::from(fd).take(limit).read_to_end(&mut bytes).ok()?;
    if bytes.len() > ActionFacts::MAX_EDIT_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// How many files git tracks at or below `dir`, through the hardened runner; 0 when no
/// repository holds it, `None` when git did not answer in time.
async fn tracked_files(git: &Git, home: &Home, dir: &Path) -> Option<u32> {
    if !dir.is_dir() {
        return Some(0);
    }
    let mut args: Vec<&str> = HARDENED.to_vec();
    args.extend(["ls-files", "-z"]);
    match git.run(dir, home, args).await {
        Ok(Some(listed)) => {
            let count = listed.split(|byte| *byte == 0).filter(|name| !name.is_empty()).count();
            Some(u32::try_from(count).unwrap_or(u32::MAX))
        }
        // NOTE: git fails outside a repository, where nothing is tracked.
        Ok(None) => Some(0),
        Err(error) => {
            tracing::debug!(error = %error, dir = %dir.display(), "the tracked files of a directory are not known");
            None
        }
    }
}

#[cfg(test)]
mod tests;
