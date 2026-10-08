//! What the toolbox's calls changed in files: the snapshots before and after a call
//! that can write, the change of a file tool from its own diff, and the end of a turn.
//!
//! The roots of a call are the turn's registered project and `$SCRATCH`, in every
//! mode, and in `auto` the registered projects that the line names (the plan's project
//! roots). A file tool's write snapshots the roots that hold its targets before it
//! writes, so the turn's first snapshot is there; its own changes come from its diffs,
//! in every directory. Nothing outside these roots is snapshotted.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_config::{IgnoredFiles, Settings};
use efr_conversation::CallContext;
use efr_permissions::Engine;
use efr_protocol::{ChangeKind, ConversationId, FileChange, FileChanges, Scope, TurnId};
use efr_scope::Home;
use efr_snapshot::{CallSnapshot, Limits, Root, Snapshots, shown_prefix};
use efr_stdx::time::Stopwatch;
use efr_tools::{WrittenFile, WrittenKind};
use tokio::sync::watch;

/// One MiB.
const MIB: u64 = 1024 * 1024;

/// The snapshot store as the toolbox uses it.
#[derive(Debug, Clone)]
pub(crate) struct CallSnapshots {
    snapshots: Snapshots,
    engine: watch::Receiver<Arc<Engine>>,
    settings: watch::Receiver<Arc<Settings>>,
    home: Home,
}

impl CallSnapshots {
    pub(crate) fn new(
        snapshots: Snapshots,
        engine: watch::Receiver<Arc<Engine>>,
        settings: watch::Receiver<Arc<Settings>>,
        home: Home,
    ) -> Self {
        CallSnapshots { snapshots, engine, settings, home }
    }

    /// The limits of the latest settings, or `None` when `snapshot.enabled` is off.
    fn limits(&self) -> Option<Limits> {
        let settings = self.settings.borrow();
        let snapshot = &settings.snapshot;
        snapshot.enabled.then(|| Limits {
            max_file_bytes: u64::from(snapshot.max_file_mib) * MIB,
            ignored_small: snapshot.ignored == IgnoredFiles::Small,
            max_files: u64::from(snapshot.max_files),
        })
    }

    /// The root of the turn's registered project.
    fn turn_project(&self, call: &CallContext) -> Option<PathBuf> {
        match &call.scope {
            Scope::Project(id) => {
                self.engine.borrow().locations().project_root(id).map(Path::to_path_buf)
            }
            _ => None,
        }
    }

    /// `paths` as roots of `call`, with the prefixes that show their files.
    fn roots(&self, call: &CallContext, paths: Vec<PathBuf>) -> Vec<Root> {
        let project = self.turn_project(call);
        let mut roots: Vec<Root> = Vec::new();
        for path in paths {
            if roots.iter().any(|root| root.path() == path) {
                continue;
            }
            let shown =
                shown_prefix(&path, project.as_deref(), Some(&call.scratch), self.home.path());
            roots.push(Root::new(path, shown));
        }
        roots
    }

    /// The turn's project and `$SCRATCH`, then `named`.
    fn call_roots(&self, call: &CallContext, named: &[PathBuf]) -> Vec<Root> {
        let mut paths: Vec<PathBuf> = self.turn_project(call).into_iter().collect();
        paths.push(call.scratch.clone());
        paths.extend(named.iter().cloned());
        self.roots(call, paths)
    }

    /// The snapshots before a `shell` call; `named` adds the projects that a line of
    /// `auto` names. `None` when snapshots are off.
    pub(crate) async fn before_call(
        &self,
        call: &CallContext,
        named: &[PathBuf],
    ) -> Option<(CallSnapshot, Limits)> {
        let limits = self.limits()?;
        let roots = self.call_roots(call, named);
        let watch = Stopwatch::start();
        let taken =
            self.snapshots.before_call(call.conversation_id, call.turn_id, roots, limits).await;
        tracing::debug!(phase = "snapshot_before", elapsed_ms = %watch, "phase=snapshot_before elapsed_ms={}", watch);
        Some((taken, limits))
    }

    /// What the `shell` call changed since [`before_call`](Self::before_call).
    pub(crate) async fn after_call(
        &self,
        before: Option<(CallSnapshot, Limits)>,
    ) -> Option<FileChanges> {
        let (taken, limits) = before?;
        if taken.is_empty() {
            return None;
        }
        let watch = Stopwatch::start();
        let changes = self.snapshots.after_call(taken, limits).await;
        tracing::debug!(phase = "snapshot_after", elapsed_ms = %watch, "phase=snapshot_after elapsed_ms={}", watch);
        changes
    }

    /// Before a file tool writes `target`: the turn's first snapshot of each root that
    /// holds it, among the turn's project, `$SCRATCH` and `projects`.
    pub(crate) async fn before_write(
        &self,
        call: &CallContext,
        target: &Path,
        projects: &[PathBuf],
    ) {
        let Some(limits) = self.limits() else { return };
        let holders: Vec<PathBuf> = self
            .call_roots(call, projects)
            .into_iter()
            .map(|root| root.path().to_path_buf())
            .filter(|root| target.starts_with(root))
            .collect();
        if holders.is_empty() {
            return;
        }
        let roots = self.roots(call, holders);
        self.snapshots.before_write(call.conversation_id, call.turn_id, roots, limits).await;
    }

    /// The end of the turn `turn_id`: its last snapshots, its refs and its changes.
    pub(crate) async fn finish_turn(
        &self,
        _conversation_id: ConversationId,
        turn_id: TurnId,
    ) -> Option<FileChanges> {
        // NOTE: a turn whose snapshots were taken finishes even when the settings
        // switched them off since, so its refs are complete.
        let limits = self.limits().unwrap_or_default();
        let watch = Stopwatch::start();
        let changes = self.snapshots.finish_turn(turn_id, limits).await;
        tracing::debug!(phase = "snapshot_turn_end", elapsed_ms = %watch, "phase=snapshot_turn_end elapsed_ms={}", watch);
        changes
    }

    /// The changes and the diff of a file tool's call, with each file's path as a
    /// client shows it: the diffs of the files in the order the call changed them, one
    /// after the other. A file whose content did not change, and that was not deleted
    /// or moved, is left out; nothing at all when no file is left.
    pub(crate) fn written(
        &self,
        call: &CallContext,
        written: &[WrittenFile],
    ) -> (Option<FileChanges>, Option<String>) {
        let mut files = Vec::new();
        let mut text = String::new();
        for file in written {
            let shown = self.shown_path(call, &file.path);
            let (kind, from) = match &file.kind {
                WrittenKind::Created => (ChangeKind::Added, None),
                WrittenKind::Deleted => (ChangeKind::Deleted, None),
                WrittenKind::Moved { from } => (ChangeKind::Renamed, Some(from.as_path())),
                WrittenKind::Changed => (ChangeKind::Modified, None),
            };
            let from_shown = from.map(|from| self.shown_path(call, from));
            let change = |added: usize, removed: usize, binary: bool| FileChange {
                path: shown.clone(),
                kind,
                from: from_shown.clone(),
                added: u32::try_from(added).unwrap_or(u32::MAX),
                removed: u32::try_from(removed).unwrap_or(u32::MAX),
                binary,
            };
            if file.binary {
                files.push(change(0, 0, true));
                continue;
            }
            match &file.diff {
                Some(diff) => {
                    files.push(change(diff.added, diff.removed, false));
                    let old = (from.unwrap_or(&file.path), from_shown.as_deref().unwrap_or(&shown));
                    text.push_str(&with_shown_header(&diff.text, old, (&file.path, &shown)));
                }
                None if kind == ChangeKind::Modified => {}
                None => files.push(change(0, 0, false)),
            }
        }
        if files.is_empty() {
            return (None, None);
        }
        let diff = (!text.is_empty()).then_some(text);
        (Some(FileChanges::from_files(files)), diff)
    }

    /// The preview of an `apply_patch` call with each path as a client shows it, as
    /// the diff of the finished call names them: the `---` and `+++` lines of each
    /// file and the lines that mark a delete or a move.
    pub(crate) fn shown_preview(&self, call: &CallContext, text: &str) -> String {
        with_shown_paths(text, |path| self.shown_path(call, path))
    }

    /// `path` as a client shows it: relative to the turn's project root, under
    /// `$SCRATCH/`, under `~/`, else absolute.
    fn shown_path(&self, call: &CallContext, path: &Path) -> String {
        let project = self.turn_project(call);
        let bases = [(project.as_deref(), ""), (Some(call.scratch.as_path()), "$SCRATCH/")];
        for (base, prefix) in bases {
            if let Some(base) = base
                && let Ok(below) = path.strip_prefix(base)
                && !below.as_os_str().is_empty()
            {
                return format!("{prefix}{}", below.display());
            }
        }
        match path.strip_prefix(self.home.path()) {
            Ok(below) if !below.as_os_str().is_empty() => format!("~/{}", below.display()),
            _ => path.display().to_string(),
        }
    }
}

/// The diff `text` of a change with its two header lines naming the shown paths, in
/// git's `a/` and `b/` form: `old` is the file's path before the change and how a
/// client shows it, `new` the same after it. A `/dev/null` side stays as it is.
fn with_shown_header(text: &str, old: (&Path, &str), new: (&Path, &str)) -> String {
    let old_line = format!("--- a{}\n", old.0.display());
    let new_line = format!("+++ b{}\n", new.0.display());
    let mut rest = text;
    let mut out = String::with_capacity(text.len());
    if let Some(after) = rest.strip_prefix(old_line.as_str()) {
        out.push_str(&format!("--- a/{}\n", old.1));
        rest = after;
    } else if let Some(after) = rest.strip_prefix("--- /dev/null\n") {
        out.push_str("--- /dev/null\n");
        rest = after;
    }
    if let Some(after) = rest.strip_prefix(new_line.as_str()) {
        out.push_str(&format!("+++ b/{}\n", new.1));
        rest = after;
    } else if let Some(after) = rest.strip_prefix("+++ /dev/null\n") {
        out.push_str("+++ /dev/null\n");
        rest = after;
    }
    out.push_str(rest);
    out
}

/// `text`, a diff of several files from `efr-tools`, with every absolute path of its
/// file headers (`--- a/<path>`, `+++ b/<path>`) and of its marks (`delete <path>`,
/// `move <from> -> <to>`) as `shown` gives it. The lines of a hunk stay as they are,
/// even when one looks like a header: the counts of each `@@` line say how many
/// follow.
fn with_shown_paths(text: &str, shown: impl Fn(&Path) -> String) -> String {
    let absolute = |path: &str| path.starts_with('/').then(|| shown(Path::new(path)));
    let side = |line: &str, start: &str, prefix: &str| -> Option<String> {
        let path = line.strip_prefix(start)?;
        absolute(path).map(|path| format!("{prefix}{path}"))
    };
    let mut out = String::with_capacity(text.len());
    // The old and new lines that the current hunk still has.
    let mut left: Option<(u32, u32)> = None;
    for line in text.split_inclusive('\n') {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        if let Some((old, new)) = left {
            let rest = match bare.chars().next() {
                Some(' ') => Some((old.saturating_sub(1), new.saturating_sub(1))),
                Some('-') => Some((old.saturating_sub(1), new)),
                Some('+') => Some((old, new.saturating_sub(1))),
                Some('\\') => Some((old, new)),
                _ => None,
            };
            if let Some(rest) = rest {
                left = (rest != (0, 0)).then_some(rest);
                out.push_str(line);
                continue;
            }
            left = None;
        }
        let rewritten = if bare.starts_with("@@ ") {
            left = hunk_counts(bare).filter(|counts| *counts != (0, 0));
            None
        } else if let Some(path) = side(bare, "--- a", "--- a/") {
            Some(path)
        } else if let Some(path) = side(bare, "+++ b", "+++ b/") {
            Some(path)
        } else if let Some(path) = bare.strip_prefix("delete ").and_then(absolute) {
            Some(format!("delete {path}"))
        } else if let Some((from, to)) =
            bare.strip_prefix("move ").and_then(|paths| paths.split_once(" -> "))
        {
            absolute(from).zip(absolute(to)).map(|(from, to)| format!("move {from} -> {to}"))
        } else {
            None
        };
        match rewritten {
            Some(rewritten) => {
                out.push_str(&rewritten);
                if line.ends_with('\n') {
                    out.push('\n');
                }
            }
            None => out.push_str(line),
        }
    }
    out
}

/// The old and new line counts of a hunk header `@@ -a,b +c,d @@`; a range without a
/// count is one line.
fn hunk_counts(line: &str) -> Option<(u32, u32)> {
    let mut words = line.split(' ').skip(1);
    let count = |word: Option<&str>, sign: char| -> Option<u32> {
        let range = word?.strip_prefix(sign)?;
        match range.split_once(',') {
            Some((_, count)) => count.parse().ok(),
            None => range.parse::<u32>().ok().map(|_| 1),
        }
    };
    Some((count(words.next(), '-')?, count(words.next(), '+')?))
}

#[cfg(test)]
mod tests;
