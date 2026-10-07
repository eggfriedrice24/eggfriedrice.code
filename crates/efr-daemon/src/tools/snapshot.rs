//! What the toolbox's calls changed in files: the snapshots before and after a call
//! that can write, the change of a file tool from its own diff, and the end of a turn.
//!
//! The roots of a call are the turn's registered project and `$SCRATCH`, in every
//! mode, and in `auto` the registered projects that the line names (the plan's project
//! roots). A file tool's write snapshots the root that holds its target before it
//! writes, so the turn's first snapshot is there; its own change comes from its diff,
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
use efr_tools::WrittenFile;
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

    /// The change and the diff of a file tool's write, with the file's path as a
    /// client shows it. Nothing when the content did not change.
    pub(crate) fn written(
        &self,
        call: &CallContext,
        written: &WrittenFile,
    ) -> (Option<FileChanges>, Option<String>) {
        let shown = self.shown_path(call, &written.path);
        let kind = if written.created { ChangeKind::Added } else { ChangeKind::Modified };
        let change = |added: usize, removed: usize, binary: bool| FileChange {
            path: shown.clone(),
            kind,
            from: None,
            added: u32::try_from(added).unwrap_or(u32::MAX),
            removed: u32::try_from(removed).unwrap_or(u32::MAX),
            binary,
        };
        if written.binary {
            return (Some(FileChanges::from_files(vec![change(0, 0, true)])), None);
        }
        let Some(diff) = &written.diff else { return (None, None) };
        let file = change(diff.added, diff.removed, false);
        let text = with_shown_header(&diff.text, &written.path, &shown);
        (Some(FileChanges::from_files(vec![file])), Some(text))
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

/// The diff `text` of a write of `path` with its two header lines naming `shown`, in
/// git's `a/` and `b/` form.
fn with_shown_header(text: &str, path: &Path, shown: &str) -> String {
    let name = shown.trim_start_matches('/');
    let old = format!("--- a{}\n", path.display());
    let new = format!("+++ b{}\n", path.display());
    let mut rest = text;
    let mut out = String::with_capacity(text.len());
    if let Some(after) = rest.strip_prefix(old.as_str()) {
        out.push_str(&format!("--- a/{name}\n"));
        rest = after;
    } else if let Some(after) = rest.strip_prefix("--- /dev/null\n") {
        out.push_str("--- /dev/null\n");
        rest = after;
    }
    if let Some(after) = rest.strip_prefix(new.as_str()) {
        out.push_str(&format!("+++ b/{name}\n"));
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests;
