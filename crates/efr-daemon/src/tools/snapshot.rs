//! What the toolbox's calls changed in files: the snapshots before and after a call
//! that can write, the change of a file tool from its own diff, and the end of a turn.
//!
//! The roots of a call are the turn's registered project and `$SCRATCH`, in every
//! mode, and in `auto` the registered projects that the line names (the plan's project
//! roots). A file tool's write snapshots the roots that hold its targets before it
//! writes, so the turn's first snapshot is there; its own changes come from its diffs,
//! in every directory. Nothing outside these roots is snapshotted.
//!
//! A root's git dirs come from efrd, never from a `.git` file that the store reads
//! (plan PB6): the worktree record when it still matches, else `<root>/.git` when it is
//! a directory. Only then does the store mirror the project's refs.
//!
//! [`CallSnapshots::covers`] answers the snapshot asks of a destructive line
//! ([`CoverAsk::Path`], `Tracked`, `Untracked`, `Refs`, `Stash`) from a snapshot taken
//! now; the caller holds the plan lock. [`CallSnapshots::turn_end`] ends a turn and
//! says what `efr undo` can take back of it ([`TurnUndoInfo`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use efr_config::{IgnoredFiles, Settings};
use efr_conversation::{CallContext, SnapshotRecord};
use efr_permissions::{CoverAsk, Coverage, Engine};
use efr_protocol::{
    ChangeKind, FileChange, FileChanges, MAX_NOT_COVERED, RootFiles, Scope, SnapshotPhase, TurnId,
    TurnUndoInfo,
};
use efr_sandbox::WorktreeCheck;
use efr_scope::Home;
use efr_snapshot::{
    CallSnapshot, CoverResult, GitDirs, LeftOutReason, Limits, NOT_COVERED_REFS, Phase, Root,
    RootCover, Snapshots, TurnEnd, shown_prefix,
};
use efr_stdx::time::Stopwatch;
use efr_tools::{WrittenFile, WrittenKind};
use tokio::sync::watch;

use crate::sandbox::projects;

/// One MiB.
const MIB: u64 = 1024 * 1024;

/// The snapshot store as the toolbox uses it.
#[derive(Debug, Clone)]
pub(crate) struct CallSnapshots {
    snapshots: Snapshots,
    engine: watch::Receiver<Arc<Engine>>,
    settings: watch::Receiver<Arc<Settings>>,
    home: Home,
    /// efr's state root, which holds the worktree records; `None` trusts no git dir.
    state: Option<PathBuf>,
    /// The roots that each running turn snapshotted, and whether it runs in `auto`.
    turns: Arc<Mutex<HashMap<TurnId, Seen>>>,
}

/// What a running turn snapshotted.
#[derive(Debug, Clone, Default)]
struct Seen {
    /// Each root with the prefix of its paths.
    roots: Vec<(PathBuf, String)>,
    /// True when a call of the turn ran in `auto`.
    auto: bool,
}

/// The end of a turn as [`CallSnapshots::turn_end`] reads it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Ended {
    /// What the store found.
    pub(crate) end: TurnEnd,
    /// True when a call of the turn ran in `auto`.
    pub(crate) auto: bool,
    /// The kept roots of the turn: the name that a person sees, the prefix of its paths
    /// and its canonical path.
    pub(crate) roots: Vec<(String, String, PathBuf)>,
}

impl CallSnapshots {
    pub(crate) fn new(
        snapshots: Snapshots,
        engine: watch::Receiver<Arc<Engine>>,
        settings: watch::Receiver<Arc<Settings>>,
        home: Home,
    ) -> Self {
        CallSnapshots { snapshots, engine, settings, home, state: None, turns: Arc::default() }
    }

    /// Trusts the git dirs that the worktree records below efr's state root `state`
    /// name, and `<root>/.git` when it is a directory.
    pub(crate) fn with_state(mut self, state: PathBuf) -> Self {
        self.state = Some(state);
        self
    }

    /// The store.
    pub(crate) fn store(&self) -> &Snapshots {
        &self.snapshots
    }

    /// The limits of the latest settings, or `None` when `snapshot.enabled` is off.
    pub(crate) fn limits(&self) -> Option<Limits> {
        let settings = self.settings.borrow();
        settings.snapshot.enabled.then(|| limits_of(&settings))
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

    /// `paths` as roots of `call`, with the prefixes that show their files, the git dirs
    /// that efrd trusts and the turn's mode. They join the turn's roots.
    async fn roots(&self, call: &CallContext, paths: Vec<PathBuf>) -> Vec<Root> {
        let project = self.turn_project(call);
        let mut shown: Vec<(PathBuf, String)> = Vec::new();
        for path in paths {
            if shown.iter().any(|(known, _)| *known == path) {
                continue;
            }
            let prefix =
                shown_prefix(&path, project.as_deref(), Some(&call.scratch), self.home.path());
            shown.push((path, prefix));
        }
        let mut roots = self.undo_roots(shown).await;
        if call.sandboxed() {
            roots = roots.into_iter().map(|root| root.with_mode("auto")).collect();
        }
        let mut turns = self.turns.lock().unwrap_or_else(PoisonError::into_inner);
        let seen = turns.entry(call.turn_id).or_default();
        seen.auto |= call.sandboxed();
        for root in &roots {
            if !seen.roots.iter().any(|(path, _)| path == root.path()) {
                seen.roots.push((root.path().to_path_buf(), root.shown().to_owned()));
            }
        }
        roots
    }

    /// The turn's project and `$SCRATCH`, then `named`.
    /// The roots that efrd snapshots before every call of `call`: the turn's project and
    /// `$SCRATCH`. Empty when snapshots are off.
    pub(crate) fn base_roots(&self, call: &CallContext) -> Vec<PathBuf> {
        if self.limits().is_none() {
            return Vec::new();
        }
        let mut paths: Vec<PathBuf> = self.turn_project(call).into_iter().collect();
        paths.push(call.scratch.clone());
        paths
    }

    async fn call_roots(&self, call: &CallContext, named: &[PathBuf]) -> Vec<Root> {
        let mut paths: Vec<PathBuf> = self.turn_project(call).into_iter().collect();
        paths.push(call.scratch.clone());
        paths.extend(named.iter().cloned());
        self.roots(call, paths).await
    }

    /// The roots that `asks` of `call` need, among the turn's project, `$SCRATCH` and
    /// `projects`: the paths that the caller locks before [`covers`](Self::covers).
    pub(crate) fn cover_roots(
        &self,
        call: &CallContext,
        asks: &[CoverAsk],
        projects: &[PathBuf],
    ) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.turn_project(call).into_iter().collect();
        paths.push(call.scratch.clone());
        paths.extend(projects.iter().cloned());
        let mut needed: Vec<PathBuf> = Vec::new();
        for ask in asks {
            if let Some((root, _)) = root_of(ask, &paths)
                && !needed.contains(&root)
            {
                needed.push(root);
            }
        }
        needed
    }

    /// The answer to each snapshot ask of `asks` (paths, tracked and untracked files,
    /// refs, stash) from a snapshot of its root taken now, the turn's `pre` when the
    /// turn has none yet. The caller holds the plan lock of the roots of
    /// [`cover_roots`](Self::cover_roots), so no call writes in between. An ask of
    /// another kind gets no answer; one outside every root is never covered.
    pub(crate) async fn covers(
        &self,
        call: &CallContext,
        asks: &[CoverAsk],
        projects: &[PathBuf],
    ) -> Vec<(CoverAsk, Coverage)> {
        let asks: Vec<&CoverAsk> = asks.iter().filter(|ask| root_ask(ask).is_some()).collect();
        if asks.is_empty() {
            return Vec::new();
        }
        let Some(limits) = self.limits() else {
            return asks.into_iter().map(|ask| (ask.clone(), Coverage::NoSnapshot)).collect();
        };
        let mut paths: Vec<PathBuf> = self.turn_project(call).into_iter().collect();
        paths.push(call.scratch.clone());
        paths.extend(projects.iter().cloned());
        let mut answers: Vec<(CoverAsk, Coverage)> = Vec::new();
        let mut by_root: Vec<(PathBuf, Vec<(CoverAsk, RootCover)>)> = Vec::new();
        for ask in asks {
            match root_of(ask, &paths) {
                Some((root, cover)) => match by_root.iter_mut().find(|(known, _)| *known == root) {
                    Some((_, list)) => list.push((ask.clone(), cover)),
                    None => by_root.push((root, vec![(ask.clone(), cover)])),
                },
                None => answers.push((ask.clone(), Coverage::NotAllowed)),
            }
        }
        for (path, list) in by_root {
            let Some(root) = self.roots(call, vec![path]).await.into_iter().next() else {
                continue;
            };
            let covers: Vec<RootCover> = list.iter().map(|(_, cover)| cover.clone()).collect();
            let results = self
                .snapshots
                .covers(call.conversation_id, call.turn_id, &root, &covers, limits)
                .await;
            for ((ask, _), result) in list.into_iter().zip(results) {
                answers.push((ask, coverage_of(result)));
            }
        }
        answers
    }

    /// The snapshots before a `shell` call; `named` adds the projects that a line of
    /// `auto` names. `None` when snapshots are off.
    pub(crate) async fn before_call(
        &self,
        call: &CallContext,
        named: &[PathBuf],
    ) -> Option<(CallSnapshot, Limits)> {
        let limits = self.limits()?;
        let roots = self.call_roots(call, named).await;
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
            .await
            .into_iter()
            .map(|root| root.path().to_path_buf())
            .filter(|root| target.starts_with(root))
            .collect();
        if holders.is_empty() {
            return;
        }
        let roots = self.roots(call, holders).await;
        self.snapshots.before_write(call.conversation_id, call.turn_id, roots, limits).await;
    }

    /// After a file tool of `call` wrote `written` (absolute paths): they join the
    /// turn's paths that undo takes back, and the turn's `last` ref of their roots moves
    /// to a snapshot taken now. The caller holds the plan lock of the roots.
    pub(crate) async fn after_write(&self, call: &CallContext, written: Vec<PathBuf>) {
        let Some(limits) = self.limits() else { return };
        if written.is_empty() {
            return;
        }
        self.snapshots.after_write(call.turn_id, written, limits).await;
    }

    /// The end of the turn `turn_id`: its last snapshots, its refs and its changes, the
    /// roots that it kept and whether it ran in `auto`.
    pub(crate) async fn turn_end(&self, turn_id: TurnId) -> Ended {
        // NOTE: a turn whose snapshots were taken finishes even when the settings
        // switched them off since, so its refs are complete.
        let limits = limits_of(&self.settings.borrow());
        let seen = self.turns.lock().unwrap_or_else(PoisonError::into_inner).remove(&turn_id);
        let seen = seen.unwrap_or_default();
        let watch = Stopwatch::start();
        let end = self.snapshots.turn_end(turn_id, limits).await;
        tracing::debug!(phase = "snapshot_turn_end", elapsed_ms = %watch, "phase=snapshot_turn_end elapsed_ms={}", watch);
        let kept: Vec<PathBuf> = end.snapshots.iter().map(|info| info.root.clone()).collect();
        let named = seen.roots.clone();
        let home = self.home.path().to_path_buf();
        let roots = tokio::task::spawn_blocking(move || {
            let mut roots: Vec<(String, String, PathBuf)> = Vec::new();
            for root in kept {
                if roots.iter().any(|(_, _, known)| *known == root) {
                    continue;
                }
                let prefix = named
                    .iter()
                    .find(|(path, _)| std::fs::canonicalize(path).ok().as_ref() == Some(&root))
                    .map(|(_, shown)| shown.clone())
                    .unwrap_or_else(|| shown_prefix(&root, None, None, &home));
                roots.push((root_name(&root, &prefix), prefix, root));
            }
            roots
        })
        .await
        .unwrap_or_default();
        Ended { end, auto: seen.auto, roots }
    }

    /// The end of the turn `turn_id`, with only its changes.
    #[cfg(test)]
    pub(crate) async fn finish_turn(
        &self,
        _conversation_id: efr_protocol::ConversationId,
        turn_id: TurnId,
    ) -> Option<FileChanges> {
        self.turn_end(turn_id).await.end.changes
    }

    /// The roots of `paths` with the git dirs that efrd trusts and their prefixes as
    /// `shown` gives them, for an undo. `$SCRATCH` leaves its `jobs` dir out: efrd
    /// writes the job outputs there and keeps them itself.
    pub(crate) async fn undo_roots(&self, paths: Vec<(PathBuf, String)>) -> Vec<Root> {
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || {
            paths
                .into_iter()
                .map(|(path, shown)| {
                    let dirs = state.as_deref().and_then(|state| git_dirs(state, &path));
                    let scratch = shown.starts_with("$SCRATCH");
                    let root = Root::new(path, shown).with_git(dirs);
                    if scratch { root.with_skip([efr_sandbox::JOB_DIR]) } else { root }
                })
                .collect()
        })
        .await
        .unwrap_or_default()
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

    /// The preview of an `apply_patch` or `edit` call with each path as a client shows it, as
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

/// The limits of `settings`, also when `snapshot.enabled` is off.
fn limits_of(settings: &Settings) -> Limits {
    let snapshot = &settings.snapshot;
    Limits {
        max_file_bytes: u64::from(snapshot.max_file_mib) * MIB,
        ignored_small: snapshot.ignored == IgnoredFiles::Small,
        max_files: u64::from(snapshot.max_files),
        // NOTE: 0 turns the mirror off.
        mirror_max_bytes: u64::from(snapshot.mirror_max_mib) * MIB,
    }
}

/// The git dirs of the project at `root` that efrd trusts (plan PB6): those of its
/// worktree record while the `.git` file still names them, else `<root>/.git` when it
/// is a directory and no link; `None` for a `.git` file that changed or has no record.
/// It blocks.
pub(crate) fn git_dirs(state: &Path, root: &Path) -> Option<GitDirs> {
    match projects::check(state, root) {
        WorktreeCheck::Matches(record) => {
            Some(GitDirs { git_dir: record.git_dir, common_dir: record.common_dir })
        }
        WorktreeCheck::NotWorktree => {
            let dot = root.join(".git");
            let is_dir = std::fs::symlink_metadata(&dot).is_ok_and(|meta| meta.is_dir());
            is_dir.then(|| GitDirs { git_dir: dot.clone(), common_dir: dot })
        }
        _ => None,
    }
}

/// The repository dir or path of a snapshot ask, and what it asks of a root.
fn root_ask(ask: &CoverAsk) -> Option<(&Path, RootCover)> {
    match ask {
        CoverAsk::Path(path) => Some((path, RootCover::Path(path.clone()))),
        CoverAsk::Tracked(dir) => Some((dir, RootCover::Tracked)),
        CoverAsk::Untracked(dir) => Some((dir, RootCover::Untracked)),
        CoverAsk::Refs(dir) => Some((dir, RootCover::Refs)),
        CoverAsk::Stash(dir) => Some((dir, RootCover::Stash)),
        _ => None,
    }
}

/// The root of `roots` that answers `ask`: for a path the deepest root that holds it,
/// for the files and refs of a repository the root that is its work tree.
fn root_of(ask: &CoverAsk, roots: &[PathBuf]) -> Option<(PathBuf, RootCover)> {
    let (at, cover) = root_ask(ask)?;
    let root = match cover {
        RootCover::Path(_) => roots
            .iter()
            .filter(|root| at.starts_with(root))
            .max_by_key(|root| root.as_os_str().len()),
        _ => roots.iter().find(|root| root.as_path() == at),
    }?;
    Some((root.clone(), cover))
}

/// The engine's word for what the store found (plan 3.3.2).
fn coverage_of(result: CoverResult) -> Coverage {
    match result {
        CoverResult::Covered => Coverage::Covered,
        CoverResult::LeftOut(_) => Coverage::LeftOut,
        CoverResult::NoSnapshot | CoverResult::Failed => Coverage::NoSnapshot,
        CoverResult::NoMirror | CoverResult::StashEntries => Coverage::NoMirror,
        CoverResult::OutsideRoot
        | CoverResult::IsRoot
        | CoverResult::GitInternal
        | CoverResult::StagedOnly => Coverage::NotAllowed,
        _ => Coverage::Unknown,
    }
}

/// The name of a root that a person sees: `$SCRATCH`, else the name of its directory.
fn root_name(root: &Path, prefix: &str) -> String {
    if prefix.starts_with("$SCRATCH") {
        return "$SCRATCH".to_owned();
    }
    root.file_name()
        .map_or_else(|| root.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// What the end of a turn says `efr undo` can take back: the files that the turn changed
/// in each kept root, `home_files` copies of files outside the roots, and what no copy
/// covers. `undoable` only for an `auto` turn with a kept `pre`, while `undo.enabled`
/// holds and no call of the conversation runs (plan PB23, PB39). `None` when the turn
/// kept no snapshot and copied no file.
pub(crate) fn undo_info(
    ended: &Ended,
    home_files: u32,
    home_not_covered: &[String],
    enabled: bool,
    call_runs: bool,
) -> Option<TurnUndoInfo> {
    let end = &ended.end;
    if !end.kept && home_files == 0 && home_not_covered.is_empty() && end.not_covered.is_empty() {
        return None;
    }
    let mut not_covered: Vec<String> = Vec::new();
    let shown_root = |root: &Path| {
        ended
            .roots
            .iter()
            .find(|(_, _, path)| path == root)
            .map_or_else(|| root.display().to_string(), |(name, _, _)| name.clone())
    };
    let shown_path = |path: &Path| {
        for (_, prefix, root) in &ended.roots {
            if let Ok(below) = path.strip_prefix(root)
                && !below.as_os_str().is_empty()
            {
                return format!("{prefix}{}", below.display());
            }
        }
        path.display().to_string()
    };
    for (root, reason) in &end.not_covered {
        let text = match reason.strip_prefix(NOT_COVERED_REFS) {
            Some(why) => format!("refs of {} ({why})", shown_root(root)),
            None => format!("{} ({reason})", shown_root(root)),
        };
        not_covered.push(text);
    }
    if call_runs {
        for (name, _, _) in &ended.roots {
            not_covered.push(format!("{name} (a call still runs)"));
        }
    }
    for (path, reason) in &end.left_out {
        let mut shown = shown_path(path);
        if *reason == LeftOutReason::SkippedDir && !shown.ends_with('/') {
            shown.push('/');
        }
        not_covered.push(format!("{shown} ({})", left_out_text(*reason)));
    }
    not_covered.extend(home_not_covered.iter().cloned());
    let more = not_covered.len().saturating_sub(MAX_NOT_COVERED);
    not_covered.truncate(MAX_NOT_COVERED);
    let roots: Vec<RootFiles> = ended
        .roots
        .iter()
        .map(|(name, prefix, path)| RootFiles {
            shown: name.clone(),
            files: changed_in(end.changes.as_ref(), prefix, &ended.roots),
            refs: end
                .refs_moved
                .iter()
                .find(|(root, _)| root == path)
                .map_or(0, |(_, moved)| *moved),
        })
        .filter(|root| root.files > 0 || root.refs > 0)
        .collect();
    let has_pre = end.snapshots.iter().any(|info| info.phase == Phase::Pre);
    Some(TurnUndoInfo {
        undoable: ended.auto && enabled && !call_runs && (has_pre || home_files > 0),
        roots,
        home_files,
        not_covered,
        not_covered_more: u32::try_from(more).unwrap_or(u32::MAX),
    })
}

/// How many files of `changes` lie below the root of `prefix`: the paths with that
/// prefix, and for the turn's project (no prefix) those of no other root.
fn changed_in(
    changes: Option<&FileChanges>,
    prefix: &str,
    roots: &[(String, String, PathBuf)],
) -> u32 {
    let Some(changes) = changes else { return 0 };
    let others: Vec<&str> = roots
        .iter()
        .map(|(_, other, _)| other.as_str())
        .filter(|other| !other.is_empty() && *other != prefix)
        .collect();
    let mine = |file: &&FileChange| {
        if prefix.is_empty() {
            !others.iter().any(|other| file.path.starts_with(other))
                && !file.path.starts_with('/')
                && !file.path.starts_with("~/")
        } else {
            file.path.starts_with(prefix)
        }
    };
    let count = changes.files.iter().filter(mine).count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Why a snapshot left a path out, in a few words for the end of a turn.
fn left_out_text(reason: LeftOutReason) -> &'static str {
    match reason {
        LeftOutReason::TooLarge => "over snapshot.max_file_mib",
        LeftOutReason::IgnoredTooLarge | LeftOutReason::SkippedDir => "ignored, large",
        LeftOutReason::IgnoredCap => "over 2000 ignored files",
        LeftOutReason::IgnoredOff => "ignored",
        LeftOutReason::Unreadable => "unreadable",
        LeftOutReason::NotUtf8 => "name is not UTF-8",
        LeftOutReason::NestedRepo => "nested repository",
        LeftOutReason::RootTooLarge => "over snapshot.max_files",
        _ => "left out",
    }
}

/// The turn's snapshots for its `snapshot_taken` events: both phases of each root,
/// under the name that a person sees.
pub(crate) fn records(ended: &Ended) -> Vec<SnapshotRecord> {
    ended
        .end
        .snapshots
        .iter()
        .map(|info| SnapshotRecord {
            root: ended
                .roots
                .iter()
                .find(|(_, _, path)| *path == info.root)
                .map_or_else(|| info.root.display().to_string(), |(name, _, _)| name.clone()),
            phase: match info.phase {
                Phase::Pre => SnapshotPhase::Pre,
                _ => SnapshotPhase::Post,
            },
            commit: info.commit.clone(),
            files: info.files,
            skipped: info.skipped,
            ms: info.ms,
        })
        .collect()
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
