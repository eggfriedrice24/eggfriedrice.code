//! `Toolbox::undo` of efrd (efr's auto spec 10.5 to 10.8, phase B): the plan of
//! `efr-snapshot`, and its writes, which efrd makes itself.
//!
//! `efr-snapshot` plans an undo or a redo on its trees and never writes the user's
//! files (plan PB7). efrd applies the plan under the plan lock of each root (PB26):
//! the "before" snapshot and the journal ref `started` first (PB37), then for each path
//! a fresh read through `DaemonFs` and a compare with what the plan expects (by git's
//! blob id, PB32), a copy of what is there now into the undo trash
//! `$XDG_DATA_HOME/efr/undo-trash/<undo-id>/` from the store's blob (PB8), and the write
//! to a new name in the same directory with a rename over the path. Nothing on the way
//! is a link: a root and every path below it open with `RESOLVE_BENEATH |
//! RESOLVE_NO_SYMLINKS`. A file that undo makes again gets its recorded mode without
//! set-id bits (PB11); one that exists keeps its mode. Then the project's refs by
//! compare and swap, the home paths (each checked again against the masks, PB14), and
//! the end marker. A write that fails stops the undo and reports what it did; the next
//! `efr undo` resumes it.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};

use efr_conversation::{UndoCandidate, UndoRequest};
use efr_protocol::{
    MAX_UNDO_ENTRIES, Origin, RefChange, TurnId, TurnUndoResult, UndoAction, UndoEntry, UndoId,
    UndoReason, UndoRefused, UndoableTurn,
};
use efr_snapshot::{
    Expect, HomeEntry, HomePlan, HomeState, HomeStep, Limits, ObjectId, PlanAction, PlanEntry,
    Root, RootPlan, SnapshotError, Snapshots, TurnRefs, UndoPlan, blob_id, stack_blocker,
};
use sha2::{Digest as _, Sha256};

use crate::home;
use crate::home::state::{self, StateCopy};
use crate::sandbox::fs::{Beneath, Node};
use crate::sandbox::units;
use crate::tools::DaemonToolbox;

/// The largest file that undo reads again before it writes over it.
const MAX_READ_BYTES: usize = 512 * 1024 * 1024;

/// The mode of each directory that undo makes in the undo trash.
const TRASH_DIR_MODE: u32 = 0o700;

/// The mode of each directory that undo makes again in a root or the home directory.
const DIR_MODE: u32 = 0o755;

/// What the result says about the index of a git root.
pub(crate) const NOTE_INDEX: &str =
    "your git index is unchanged; git status can show staged changes";

/// What the result says about what a snapshot never holds.
pub(crate) const NOTE_SPECIAL: &str = "empty directories and special files do not come back";

/// What the result says when it changes a link or a file of the user's units.
pub(crate) const NOTE_UNITS: &str =
    "run systemctl --user daemon-reload so that systemd reads the units again";

/// What the result says after a write error.
pub(crate) const NOTE_STOPPED: &str = "efr undo stopped at a write that failed; run efr undo \
     again to finish it, or efr undo --redo to take back what it did";

/// What one path of the plan came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Done {
    /// The plan's action ran: undo wrote or removed the path, or ran the command.
    Applied,
    /// There was nothing to do: the path has no action, or it already is what the plan
    /// wants.
    Unneeded,
    /// The path is not what the plan expects any more.
    Changed,
    /// A mask covers the path now.
    Masked,
    /// The undo stopped before it.
    NotReached,
}

/// The outcome of the writes of a plan.
#[derive(Debug, Default)]
struct Applied {
    roots: Vec<Vec<Done>>,
    refs: Vec<Vec<(String, Result<(), UndoReason>)>>,
    home: Vec<Done>,
    /// True when something went to the undo trash.
    trashed: bool,
    /// True when undo wrote or removed a path, moved a ref or ran a command.
    wrote: bool,
    /// The path and error of the write that stopped the undo.
    failed: Option<(PathBuf, std::io::Error)>,
}

impl DaemonToolbox {
    /// Undoes or redoes one turn of `request.candidates`, or lists them, or with
    /// `dry_run` only says what it would do.
    pub(super) async fn undo_turn(
        &self,
        request: UndoRequest,
    ) -> Result<TurnUndoResult, UndoRefused> {
        if request.origin == Origin::Phone {
            return Err(UndoRefused::PhoneOrigin);
        }
        let enabled = self.settings.borrow().undo.enabled;
        let (Some(snapshots), true) = (&self.snapshots, enabled) else {
            return Err(UndoRefused::Off);
        };
        let store = snapshots.store();
        let conversation = request.conversation_id;
        let params = &request.params;
        let turns = store.turns_with_refs(conversation).await;
        let mut result = TurnUndoResult {
            conversation_id: conversation,
            turn_id: None,
            title: None,
            undo_id: None,
            applied: false,
            stopped: false,
            resumed: false,
            entries: Vec::new(),
            entries_more: 0,
            refs: Vec::new(),
            trash: None,
            notes: Vec::new(),
            turns: Vec::new(),
        };
        if params.list {
            result.turns = self.listed(&request.candidates, &turns);
            return Ok(result);
        }
        let limits = snapshots.limits().unwrap_or_default();
        let paths: Vec<PathBuf> = unique_roots(&turns);
        let force = params.force;
        let plan = if params.redo {
            self.planned(&paths, |roots| async move {
                store.plan_redo(conversation, &roots, force, limits).await
            })
            .await
            .map_err(|error| match error {
                SnapshotError::NoRedo => UndoRefused::NoRedo,
                other => failed(&other),
            })?
        } else {
            let turn = pick(&request, &turns)?;
            let undo_id = request.undo_id.shown();
            self.planned(&paths, |roots| {
                let undo_id = undo_id.clone();
                async move {
                    store.plan_undo(conversation, turn, &roots, force, &undo_id, limits).await
                }
            })
            .await
            .map_err(|error| failed(&error))?
        };
        let candidate = request.candidates.iter().find(|candidate| candidate.turn_id == plan.turn);
        result.turn_id = Some(plan.turn);
        result.title = candidate.and_then(|candidate| candidate.title.clone());
        result.undo_id = parse_undo_id(&plan.undo_id);
        result.resumed = plan.resumed;
        if params.dry_run {
            self.describe(&mut result, &plan, None);
            return Ok(result);
        }
        if !plan_acts(&plan) {
            // NOTE: nothing to write, such as only paths that changed after the turn:
            // the rows say why, and `--force` takes them. An undo that stopped half way
            // and has nothing left ends here, so its journal does not stay open.
            if plan.resumed {
                result.applied = self.finish(store, &plan, limits).await;
            }
            self.describe(&mut result, &plan, None);
            return Ok(result);
        }
        let applied = self.apply(&plan, limits).await;
        // NOTE: an earlier attempt of a resumed undo wrote, so it ends like one that
        // wrote now. An undo that wrote nothing is no undo: the turn stays as it is and
        // its journal goes, so the next `efr undo` plans it again.
        let wrote = applied.wrote || plan.resumed;
        let stopped = applied.failed.is_some();
        if !stopped && wrote {
            result.applied = self.finish(store, &plan, limits).await;
        } else if !wrote
            && !plan.redo
            && let Err(error) = store.drop_undo(&plan).await
        {
            tracing::warn!(error = %efr_stdx::with_causes(&error), undo = %plan.undo_id, "the journal of an undo that wrote nothing could not be removed");
        }
        result.stopped = stopped && wrote;
        self.describe(&mut result, &plan, Some(&applied));
        Ok(result)
    }

    /// Records the end of the undo or redo `plan`; false, with a line in the log, when
    /// the store could not.
    async fn finish(&self, store: &Snapshots, plan: &UndoPlan, limits: Limits) -> bool {
        match store.finish_undo(plan, limits).await {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), undo = %plan.undo_id, "the end of an undo could not be recorded");
                false
            }
        }
    }

    /// The ended turns of `candidates` that a store holds, newest first.
    fn listed(&self, candidates: &[UndoCandidate], turns: &[TurnRefs]) -> Vec<UndoableTurn> {
        let auto = self.auto_turns();
        candidates
            .iter()
            .filter_map(|candidate| {
                let refs = turns.iter().find(|refs| refs.turn == candidate.turn_id)?;
                Some(UndoableTurn {
                    turn_id: candidate.turn_id,
                    title: candidate.title.clone(),
                    auto: auto.contains(&candidate.turn_id),
                    undone: refs.undone,
                    stopped: candidate.stopped || (!refs.has_post && !refs.roots.is_empty()),
                    ended: candidate.ended,
                })
            })
            .collect()
    }

    /// A plan of `make`, with the git dirs that efrd trusts for each root that has
    /// them, under the prefix that the turn's snapshot recorded.
    async fn planned<F, Fut>(&self, paths: &[PathBuf], make: F) -> Result<UndoPlan, SnapshotError>
    where
        F: Fn(Vec<Root>) -> Fut,
        Fut: Future<Output = Result<UndoPlan, SnapshotError>>,
    {
        let Some(snapshots) = &self.snapshots else { return Err(SnapshotError::TaskFailed) };
        let unnamed: Vec<(PathBuf, String)> =
            paths.iter().map(|path| (path.clone(), String::new())).collect();
        let trusted = snapshots.undo_roots(unnamed).await;
        let first = make(Vec::new()).await?;
        if trusted.iter().all(|root| root.git().is_none()) {
            return Ok(first);
        }
        // NOTE: a root that the daemon names replaces the prefix of the turn's record,
        // so the second plan names each root by the prefix of the first.
        let named: Vec<(PathBuf, String)> =
            first.roots.iter().map(|root| (root.root.clone(), root.shown.clone())).collect();
        let roots = snapshots.undo_roots(named).await;
        make(roots).await
    }

    /// Writes `plan` under the plan lock of its roots.
    async fn apply(&self, plan: &UndoPlan, limits: Limits) -> Applied {
        let mut applied = Applied::default();
        let Some(snapshots) = &self.snapshots else { return applied };
        let store = snapshots.store();
        let locked: Vec<PathBuf> = plan.roots.iter().map(|root| root.root.clone()).collect();
        let _guard = match &self.sandbox {
            Some((sandbox, _)) => Some(sandbox.locks().lock(&locked).await),
            None => None,
        };
        let captured = match store.capture_before_undo(plan, limits).await {
            Ok(captured) => captured,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), "the snapshot before an undo failed");
                let at = plan.roots.first().map(|root| root.root.clone()).unwrap_or_default();
                applied.failed = Some((at, std::io::Error::other(error.to_string())));
                return applied;
            }
        };
        if !plan.redo && plan.home.steps.iter().any(HomeStep::acts) {
            let entries = undo_before_entries(&plan.home);
            let mut prepared = home::Prepared { entries, ..home::Prepared::default() };
            let states = plan.home.steps.iter().filter(|step| step.acts());
            for copy in states.filter_map(|step| StateCopy::from_path(&step.path)) {
                let (entry, bytes) = state::read(self.state_runner(), copy).await;
                prepared.push_state(entry, bytes);
            }
            let reader = home::Reader::new(&prepared);
            let copied = store.home_undo_before(plan.conversation, &plan.undo_id, reader).await;
            if let Err(error) = copied {
                tracing::warn!(error = %efr_stdx::with_causes(&error), "the copy of the home files before an undo failed");
                applied.failed = Some((
                    self.home.path().to_path_buf(),
                    std::io::Error::other(error.to_string()),
                ));
                return applied;
            }
        }
        let trash = self.trash_dir(&plan.undo_id);
        for root in &plan.roots {
            let done = self.apply_root(store, root, trash.as_deref(), &mut applied).await;
            applied.roots.push(done);
        }
        let named: Vec<(PathBuf, String)> =
            plan.roots.iter().map(|root| (root.root.clone(), root.shown.clone())).collect();
        let roots = snapshots.undo_roots(named).await;
        for (root, named) in plan.roots.iter().zip(&roots) {
            let moving = root.refs.iter().any(|ref_plan| ref_plan.reason.is_none());
            if applied.failed.is_some() || !moving {
                applied.refs.push(Vec::new());
                continue;
            }
            // NOTE: a ref moves only while the store keeps the value it moves away from,
            // so a redo can give it back (plan PB7, S19).
            if captured.unmirrored.contains(&root.root) {
                let kept = root.refs.iter().map(|ref_plan| {
                    (ref_plan.name.clone(), Err(ref_plan.reason.unwrap_or(UndoReason::NoMirror)))
                });
                applied.refs.push(kept.collect());
                continue;
            }
            let moved = store.restore_refs(named, &root.refs).await;
            applied.wrote |= moved.iter().any(|(_, result)| result.is_ok());
            applied.refs.push(moved);
        }
        applied.home = self.apply_home(store, &plan.home, trash.as_deref(), &mut applied).await;
        applied
    }

    /// Writes the entries of `root` that act.
    async fn apply_root(
        &self,
        store: &Snapshots,
        root: &RootPlan,
        trash: Option<&Path>,
        applied: &mut Applied,
    ) -> Vec<Done> {
        let mut done = Vec::with_capacity(root.entries.len());
        let opened = Beneath::open(&root.root);
        // NOTE: a root that is another directory now than at the turn's first
        // snapshot is written only where the plan says that force allows it (S22).
        let replaced = match &opened {
            Ok(dir) => dir.dev_ino().is_ok_and(|now| root.dev_ino != (0, 0) && now != root.dev_ino),
            Err(_) => true,
        };
        for entry in &root.entries {
            if !entry.acts() {
                done.push(Done::Unneeded);
                continue;
            }
            if applied.failed.is_some() {
                done.push(Done::NotReached);
                continue;
            }
            let dir = match &opened {
                Ok(dir) if !replaced || entry.forced == Some(UndoReason::RootReplaced) => dir,
                _ => {
                    done.push(Done::Changed);
                    continue;
                }
            };
            match self.apply_entry(store, root, dir, entry, trash).await {
                Ok((step, trashed)) => {
                    applied.trashed |= trashed;
                    applied.wrote |= step == Done::Applied;
                    done.push(step);
                }
                Err(error) => {
                    applied.failed = Some((root.root.join(&entry.path), error));
                    done.push(Done::NotReached);
                }
            }
        }
        done
    }

    /// Checks one entry again and writes it; true when something went to the trash.
    async fn apply_entry(
        &self,
        store: &Snapshots,
        root: &RootPlan,
        dir: &Beneath,
        entry: &PlanEntry,
        trash: Option<&Path>,
    ) -> std::io::Result<(Done, bool)> {
        let relative = Path::new(&entry.path);
        let make = matches!(entry.action, PlanAction::Write { .. } | PlanAction::Link { .. });
        let parent = match dir.parent(relative, make.then_some(DIR_MODE)) {
            Err(error) if moved_away(&error) => return Ok((Done::Changed, false)),
            parent => parent?,
        };
        let Some((parent, name)) = parent else {
            // NOTE: no parent means no file: only a plan that expects none goes on.
            let done = if entry.expect == Expect::Missing { Done::Unneeded } else { Done::Changed };
            return Ok((done, false));
        };
        let now = parent.node(&name, MAX_READ_BYTES).unwrap_or(Node::Other);
        if !matches_expect(&now, &entry.expect) {
            return Ok((Done::Changed, false));
        }
        let trashed = now != Node::Missing;
        if trashed {
            // NOTE: every overwrite and removal goes to the undo trash first (PB8), from
            // the store's copy, which equals the file by the check above.
            let (Some((id, mode)), Some(trash)) = (&entry.trash, trash) else {
                return Ok((Done::Changed, false));
            };
            let bytes = store.blob(&root.root, id).await.map_err(std::io::Error::other)?;
            let at = Path::new("root").join(root_id(&root.root)).join(relative);
            put_trash(trash, &at, &bytes, *mode)?;
        }
        match &entry.action {
            PlanAction::Write { blob, mode } => {
                let bytes = store.blob(&root.root, blob).await.map_err(std::io::Error::other)?;
                let mode = match &now {
                    Node::File { mode: kept, .. } => *kept,
                    _ => *mode,
                };
                parent.replace(&name, &bytes, mode)?;
            }
            PlanAction::Link { target } => parent.link(&name, target)?,
            PlanAction::Delete => parent.remove(&name)?,
            _ => {}
        }
        Ok((Done::Applied, trashed))
    }

    /// Writes the home steps of `plan` that act, each checked again against the masks.
    async fn apply_home(
        &self,
        store: &Snapshots,
        plan: &HomePlan,
        trash: Option<&Path>,
        applied: &mut Applied,
    ) -> Vec<Done> {
        let mut masks = self.masks();
        masks.extend(self.efr_roots.iter().cloned());
        let mut done = Vec::with_capacity(plan.steps.len());
        for step in &plan.steps {
            if !step.acts() {
                done.push(Done::Unneeded);
                continue;
            }
            if applied.failed.is_some() {
                done.push(Done::NotReached);
                continue;
            }
            if let Some(copy) = StateCopy::from_path(&step.path) {
                match self.apply_state(store, step, copy, trash).await {
                    Ok((outcome, trashed)) => {
                        applied.trashed |= trashed;
                        applied.wrote |= outcome == Done::Applied;
                        done.push(outcome);
                    }
                    Err(error) => {
                        applied.failed = Some((step.path.clone(), error));
                        done.push(Done::NotReached);
                    }
                }
                continue;
            }
            if !step.path.is_absolute() {
                done.push(Done::Unneeded);
                continue;
            }
            if home::masked(&step.path, &masks) {
                done.push(Done::Masked);
                continue;
            }
            match self.apply_step(store, step, trash).await {
                Ok((outcome, trashed)) => {
                    applied.trashed |= trashed;
                    applied.wrote |= outcome == Done::Applied;
                    done.push(outcome);
                }
                Err(error) => {
                    applied.failed = Some((step.path.clone(), error));
                    done.push(Done::NotReached);
                }
            }
        }
        done
    }

    /// Sets the state of one step back with its program (a command undo of the crontab
    /// or of linger, plan PB20), when it is still the state after the turn; true when
    /// the crontab of now went to the undo trash first.
    async fn apply_state(
        &self,
        store: &Snapshots,
        step: &HomeStep,
        copy: StateCopy,
        trash: Option<&Path>,
    ) -> std::io::Result<(Done, bool)> {
        let runner = self.state_runner();
        let now = state::read(runner, copy).await;
        let bytes = match &step.blob {
            Some(blob) => Some(store.home_blob(blob).await.map_err(std::io::Error::other)?),
            None => None,
        };
        let mut trashed = false;
        let keep = |text: &[u8]| -> std::io::Result<()> {
            let Some(trash) = trash else { return Ok(()) };
            trashed = true;
            put_trash(trash, &Path::new("state").join(copy.name()), text, 0o100_600)
        };
        let outcome = match state::undo(runner, step, &now, bytes, keep).await? {
            state::Undone::Applied => Done::Applied,
            state::Undone::Changed => Done::Changed,
        };
        Ok((outcome, trashed))
    }

    /// Checks one home step again and writes it; true when something went to the trash.
    async fn apply_step(
        &self,
        store: &Snapshots,
        step: &HomeStep,
        trash: Option<&Path>,
    ) -> std::io::Result<(Done, bool)> {
        let relative = step
            .path
            .strip_prefix("/")
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let top = Beneath::open(Path::new("/"))?;
        let make = matches!(step.write, HomeState::File { .. } | HomeState::Link { .. });
        let parent = match top.parent(relative, make.then_some(DIR_MODE)) {
            Err(error) if moved_away(&error) => return Ok((Done::Changed, false)),
            parent => parent?,
        };
        let Some((parent, name)) = parent else {
            let done = if step.expect == Expect::Missing { Done::Unneeded } else { Done::Changed };
            return Ok((done, false));
        };
        let now = parent.node(&name, MAX_READ_BYTES).unwrap_or(Node::Other);
        if !matches_expect(&now, &step.expect) {
            return Ok((Done::Changed, false));
        }
        let trashed = now != Node::Missing;
        if trashed {
            let Some(trash) = trash else { return Ok((Done::Changed, false)) };
            let at = Path::new("home").join(relative);
            match (&now, &step.trash) {
                (_, Some((id, mode))) => {
                    let bytes = store.home_blob(id).await.map_err(std::io::Error::other)?;
                    put_trash(trash, &at, &bytes, *mode)?;
                }
                (Node::File { bytes, mode }, None) => {
                    put_trash(trash, &at, bytes, 0o100_000 | mode)?;
                }
                (Node::Link(target), None) => put_trash(trash, &at, target, 0o120_000)?,
                _ => return Ok((Done::Changed, false)),
            }
        }
        match &step.write {
            HomeState::File { mode } => {
                let Some(blob) = &step.blob else { return Ok((Done::Changed, trashed)) };
                let bytes = store.home_blob(blob).await.map_err(std::io::Error::other)?;
                let mode = match &now {
                    Node::File { mode: kept, .. } => *kept,
                    _ => *mode,
                };
                parent.replace(&name, &bytes, mode)?;
            }
            HomeState::Link { target } => parent.link(&name, target)?,
            HomeState::Missing => parent.remove(&name)?,
            _ => return Ok((Done::Changed, trashed)),
        }
        Ok((Done::Applied, trashed))
    }

    /// `$XDG_DATA_HOME/efr/undo-trash/<undo-id>`, or `None` without a data root.
    fn trash_dir(&self, undo_id: &str) -> Option<PathBuf> {
        let valid = !undo_id.is_empty()
            && undo_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        self.undo_trash.as_ref().filter(|_| valid).map(|dir| dir.join(undo_id))
    }

    /// Fills `result` with what `plan` does, and with `applied`, what it did.
    fn describe(&self, result: &mut TurnUndoResult, plan: &UndoPlan, applied: Option<&Applied>) {
        let home = self.home.path().to_path_buf();
        let mut entries: Vec<UndoEntry> = Vec::new();
        let mut git = false;
        let mut writes = false;
        // The plan holds the roots in the order of their store ids, which come from a
        // hash of the path; the report lists them by what it shows, so the project of
        // the conversation (no prefix) comes first and the order is the same each time.
        let mut order: Vec<(usize, &RootPlan)> = plan.roots.iter().enumerate().collect();
        order.sort_by(|(_, a), (_, b)| a.shown.cmp(&b.shown).then_with(|| a.root.cmp(&b.root)));
        for (n, root) in order {
            let done = applied.and_then(|applied| applied.roots.get(n));
            for (m, entry) in root.entries.iter().enumerate() {
                let path = format!("{}{}", root.shown, entry.path);
                writes |= entry.acts();
                let step = done.and_then(|done| done.get(m)).copied();
                entries.push(entry_of(&entry.action, path, step));
            }
            let ref_done = applied.and_then(|applied| applied.refs.get(n));
            let name = root_name(&root.root, &root.shown);
            for ref_plan in &root.refs {
                git = true;
                let outcome = ref_done
                    .and_then(|done| done.iter().find(|(known, _)| *known == ref_plan.name));
                let (applied_ref, reason) = match (applied, outcome) {
                    (None, _) => (None, ref_plan.reason),
                    (Some(_), Some((_, Ok(())))) => (Some(true), None),
                    (Some(_), Some((_, Err(reason)))) => (Some(false), Some(*reason)),
                    (Some(_), None) => (Some(false), ref_plan.reason.or(Some(UndoReason::Unknown))),
                };
                result.refs.push(RefChange {
                    root: name.clone(),
                    name: ref_plan.name.clone(),
                    from: ref_plan.expect.as_ref().map(short),
                    to: ref_plan.to.as_ref().map(short),
                    applied: applied_ref,
                    reason,
                });
            }
        }
        for (n, step) in plan.home.steps.iter().enumerate() {
            let done = applied.and_then(|applied| applied.home.get(n)).copied();
            writes |= step.acts();
            let path = match step.path.strip_prefix(&home) {
                Ok(below) if step.path.is_absolute() => format!("~/{}", below.display()),
                _ => step.path.display().to_string(),
            };
            entries.push(home_entry_of(step, path, done));
        }
        let more = entries.len().saturating_sub(MAX_UNDO_ENTRIES);
        entries.truncate(MAX_UNDO_ENTRIES);
        result.entries = entries;
        result.entries_more = u32::try_from(more).unwrap_or(u32::MAX);
        let planned = plan
            .roots
            .iter()
            .flat_map(|root| &root.entries)
            .any(|entry| entry.acts() && entry.trash.is_some())
            || plan.home.steps.iter().any(|step| step.acts() && step.trash.is_some());
        let trashed = applied.map_or(planned, |applied| applied.trashed);
        result.trash = if trashed { self.trash_dir(&plan.undo_id) } else { None };
        if git {
            result.notes.push(NOTE_INDEX.to_owned());
        }
        if writes {
            result.notes.push(NOTE_SPECIAL.to_owned());
        }
        // NOTE: systemd keeps a unit enabled in its manager until it reads its links
        // again (plan PB34, KB14).
        let unit_dir = units::config_dir(&home);
        if plan.home.steps.iter().any(|step| step.acts() && step.path.starts_with(&unit_dir)) {
            result.notes.push(NOTE_UNITS.to_owned());
        }
        if let Some((path, error)) = applied.and_then(|applied| applied.failed.as_ref()) {
            tracing::warn!(path = %path.display(), error = %error, "an undo stopped at a write that failed");
            result.notes.push(format!("{NOTE_STOPPED} ({}: {error})", path.display()));
        }
    }

    /// The turns that ran in `auto` since efrd started.
    fn auto_turns(&self) -> HashSet<TurnId> {
        self.auto_turns.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

/// True for the error of a path that leads through a link or a file now: it is not
/// what the plan expects, so undo leaves it alone and goes on.
fn moved_away(error: &std::io::Error) -> bool {
    let codes = [rustix::io::Errno::LOOP, rustix::io::Errno::NOTDIR, rustix::io::Errno::XDEV];
    codes.iter().any(|code| error.raw_os_error() == Some(code.raw_os_error()))
}

/// The refusal for a store that failed, with a line in the log.
fn failed(error: &SnapshotError) -> UndoRefused {
    tracing::warn!(error = %efr_stdx::with_causes(error), "an undo could not be planned");
    UndoRefused::TurnNotFound
}

/// The turn that `request` undoes: an undo that stopped half way first (PB37), else
/// the named turn, else the newest turn of the candidates that no undo undid. Undo is a
/// stack: a newer turn that is not undone refuses, unless `force`.
fn pick(request: &UndoRequest, turns: &[TurnRefs]) -> Result<TurnId, UndoRefused> {
    let candidates: HashSet<TurnId> =
        request.candidates.iter().map(|candidate| candidate.turn_id).collect();
    if let Some(open) = turns.iter().find(|refs| refs.started_undo.is_some()) {
        return Ok(open.turn);
    }
    let turn = match request.params.turn_id {
        Some(turn) => {
            let refs =
                turns.iter().find(|refs| refs.turn == turn).ok_or(UndoRefused::TurnNotFound)?;
            if refs.undone {
                return Err(UndoRefused::NothingToUndo);
            }
            turn
        }
        None => {
            turns
                .iter()
                .find(|refs| !refs.undone && candidates.contains(&refs.turn))
                .ok_or(UndoRefused::NothingToUndo)?
                .turn
        }
    };
    if let Some(newer) = stack_blocker(turns, turn)
        && !request.params.force
    {
        return Err(UndoRefused::LaterTurnsNotUndone { turn_id: newer });
    }
    Ok(turn)
}

/// Every root that a store of the conversation holds a turn of, each once.
fn unique_roots(turns: &[TurnRefs]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for root in turns.iter().flat_map(|refs| &refs.roots) {
        if !roots.contains(root) {
            roots.push(root.clone());
        }
    }
    roots
}

/// True when the plan writes a path or moves a ref.
fn plan_acts(plan: &UndoPlan) -> bool {
    plan.roots.iter().any(|root| {
        root.entries.iter().any(PlanEntry::acts)
            || root.refs.iter().any(|ref_plan| ref_plan.reason.is_none())
    }) || plan.home.steps.iter().any(HomeStep::acts)
}

/// True when `now` is what `expect` says.
fn matches_expect(now: &Node, expect: &Expect) -> bool {
    match (now, expect) {
        (Node::Missing, Expect::Missing) => true,
        (Node::File { bytes, .. }, Expect::Blob(id)) => blob_id(bytes) == *id,
        (Node::Link(target), Expect::Link(expected)) => target == expected,
        (Node::Missing | Node::File { .. } | Node::Link(_), Expect::Any) => true,
        _ => false,
    }
}

/// The entries of the copy of the home paths that `plan` changes, as they are now.
fn undo_before_entries(plan: &HomePlan) -> Vec<HomeEntry> {
    plan.steps
        .iter()
        .filter(|step| step.acts() && step.path.is_absolute())
        .map(|step| {
            let state = match std::fs::symlink_metadata(&step.path) {
                Err(_) => HomeState::Missing,
                Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(&step.path) {
                    Ok(to) => HomeState::Link {
                        target: std::os::unix::ffi::OsStringExt::into_vec(to.into_os_string()),
                    },
                    Err(_) => HomeState::NotCovered(UndoReason::Unknown),
                },
                Ok(meta) if meta.is_file() => HomeState::File {
                    mode: std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o7777,
                },
                Ok(_) => HomeState::NotCovered(UndoReason::Unknown),
            };
            HomeEntry { path: step.path.clone(), state, floor: step.floor, chain: Vec::new() }
        })
        .collect()
}

/// Writes `bytes` into the undo trash `trash` at `at` (relative to it) with `mode`: a
/// link for `0o120000`, else a file with `mode & 0o777`. Each directory has mode 0700,
/// and every path opens below the trash's own, with no link on the way.
fn put_trash(trash: &Path, at: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let (Some(base), Some(undo)) = (trash.parent(), trash.file_name()) else {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    };
    std::fs::create_dir_all(base)?;
    std::fs::set_permissions(base, std::os::unix::fs::PermissionsExt::from_mode(TRASH_DIR_MODE))?;
    let top = Beneath::open(base)?;
    let relative = Path::new(undo).join(at);
    let Some((dir, name)) = top.parent(&relative, Some(TRASH_DIR_MODE))? else {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    };
    let link = mode & 0o170_000 == 0o120_000;
    dir.add(&name, bytes, mode & 0o777, link)?;
    Ok(())
}

/// The name of a root's directory in the undo trash: the first 16 hex characters of
/// the SHA-256 of its path, as the store names it.
fn root_id(root: &Path) -> String {
    let digest = Sha256::digest(root.as_os_str().as_encoded_bytes());
    digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect()
}

/// The name of a root that a person sees.
fn root_name(root: &Path, shown: &str) -> String {
    if shown.starts_with("$SCRATCH") {
        return "$SCRATCH".to_owned();
    }
    root.file_name()
        .map_or_else(|| root.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// The undo id of a plan as the wire names it.
fn parse_undo_id(shown: &str) -> Option<UndoId> {
    shown.strip_prefix("u-")?.parse::<UndoId>().ok()
}

/// A short object id.
fn short(id: &ObjectId) -> String {
    id.as_str().chars().take(12).collect()
}

/// One row of the result for a root path.
fn entry_of(action: &PlanAction, path: String, done: Option<Done>) -> UndoEntry {
    let planned = match action {
        PlanAction::Write { .. } | PlanAction::Link { .. } => (UndoAction::Restore, None),
        PlanAction::Delete => (UndoAction::Trash, None),
        PlanAction::Skip(reason) => (UndoAction::Skip, Some(*reason)),
        PlanAction::NotCovered(_) => (UndoAction::NotCovered, Some(UndoReason::LeftOut)),
        _ => (UndoAction::Skip, Some(UndoReason::Unknown)),
    };
    with_done(planned, path, done)
}

/// One row of the result for a home path.
fn home_entry_of(step: &HomeStep, path: String, done: Option<Done>) -> UndoEntry {
    let planned = match (&step.write, &step.expect) {
        (HomeState::NotCovered(reason), _) => (UndoAction::NotCovered, Some(*reason)),
        (_, Expect::Unknown) => (UndoAction::Skip, Some(UndoReason::Unknown)),
        (HomeState::State, _) => (UndoAction::Command, None),
        (HomeState::Missing, _) if StateCopy::from_path(&step.path).is_some() => {
            (UndoAction::Command, None)
        }
        (HomeState::Missing, _) => (UndoAction::Trash, None),
        _ => (UndoAction::Restore, None),
    };
    with_done(planned, path, done)
}

/// A planned row with what the apply did to it.
fn with_done(
    planned: (UndoAction, Option<UndoReason>),
    path: String,
    done: Option<Done>,
) -> UndoEntry {
    let acts = matches!(
        planned.0,
        UndoAction::Restore | UndoAction::Trash | UndoAction::Delete | UndoAction::Command
    );
    let (action, reason) = match done {
        Some(Done::Changed) if acts => (UndoAction::Skip, Some(UndoReason::ChangedAfterTurn)),
        Some(Done::Masked) if acts => (UndoAction::NotCovered, Some(UndoReason::Masked)),
        Some(Done::NotReached) if acts => (UndoAction::Skip, Some(UndoReason::Unknown)),
        _ => planned,
    };
    UndoEntry { action, path, reason }
}

#[cfg(test)]
mod tests;
