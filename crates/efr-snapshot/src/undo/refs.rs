//! The project's refs in an undo: which refs move back ([`Snapshots::plan_refs`]), the
//! copy of the refs that an undo moves, for a later redo ([`Snapshots::mirror_moved_refs`]),
//! and the move itself ([`Snapshots::restore_refs`]): compare and swap through
//! [`ProjectGit`], with the objects that the project lost brought back from the store
//! first.

use std::collections::BTreeSet;
use std::ffi::OsString;

use efr_protocol::UndoReason;

use crate::meta::Meta;
use crate::mirror::MIRROR_FETCH_FAILED;
use crate::object::ObjectId;
use crate::project_git::{GitDirs, Pins, ProjectGit, ProjectRefs, Refusal};
use crate::runner::Run;
use crate::snapshots::{Root, Snapshots};
use crate::store::Store;
use crate::turn::{MIRROR_TAKEN, refs_with_head};
use crate::undo::{RefPlan, RootPlan, UndoPlan, undo_ref};

/// The two records of a root's refs that a plan compares.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RefSides<'a> {
    /// The refs that undo gives back (`pre`, or `before` for a redo).
    pub(crate) target: &'a Meta,
    /// The refs that it expects now (`post`, `after`, or `pre` for a half undo).
    pub(crate) source: &'a Meta,
    /// False when the turn has no record from its end.
    pub(crate) has_post: bool,
    /// True when the collector dropped the mirror that `target` took.
    pub(crate) mirror_gone: bool,
}

/// What `HEAD` names in a record or now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Head {
    Branch(String),
    Detached(String),
    Unknown,
}

impl Head {
    fn of_meta(meta: &Meta) -> Head {
        match meta.head.as_deref() {
            Some(name) if name.starts_with("refs/") => Head::Branch(name.to_owned()),
            Some(commit) if ObjectId::parse(commit).is_some() => Head::Detached(commit.to_owned()),
            _ => Head::Unknown,
        }
    }

    fn of_refs(refs: &ProjectRefs) -> Head {
        match (&refs.head_ref, &refs.head_commit) {
            (Some(name), _) => Head::Branch(name.clone()),
            (None, Some(commit)) => Head::Detached(commit.clone()),
            (None, None) => Head::Unknown,
        }
    }
}

impl Snapshots {
    /// The refs of the root of `plan` that differ between the two sides, with what
    /// undo does with each. `trusted` says whether the daemon named the git dirs that
    /// the snapshot recorded; without it, or without the side's mirror, no ref moves.
    pub(crate) async fn plan_refs(
        &self,
        store: &Store,
        plan: &RootPlan,
        sides: RefSides<'_>,
        trusted: bool,
        force: bool,
    ) -> Vec<RefPlan> {
        let RefSides { target, source, has_post, mirror_gone } = sides;
        let names: BTreeSet<&String> = target
            .refs
            .keys()
            .chain(source.refs.keys())
            .filter(|name| name.as_str() != "HEAD")
            .collect();
        let changed: Vec<&String> = names
            .into_iter()
            .filter(|name| target.refs.get(*name) != source.refs.get(*name))
            .collect();
        let head_moves = Head::of_meta(target) != Head::of_meta(source)
            && Head::of_meta(target) != Head::Unknown;
        if changed.is_empty() && !head_moves {
            return Vec::new();
        }
        let pins = plan.git.as_ref().map(|(_, pins)| *pins);
        let blocked = if !trusted || target.mirror.as_deref() != Some(MIRROR_TAKEN) {
            Some(UndoReason::NoMirror)
        } else if mirror_gone {
            Some(UndoReason::StoreFull)
        } else {
            None
        };
        let now = match (blocked, &plan.git) {
            (None, Some((dirs, pins))) => match self.open_project(dirs, *pins).await {
                Ok(project) => project.refs().await.ok(),
                Err(_) => None,
            },
            _ => None,
        };
        let blocked = blocked.or_else(|| now.is_none().then_some(UndoReason::RootReplaced));
        let wanted: Vec<ObjectId> = changed
            .iter()
            .filter_map(|name| target.refs.get(*name))
            .filter_map(|id| ObjectId::parse(id))
            .collect();
        let missing = self.missing_objects(store, &wanted).await.unwrap_or_default();
        let mut refs = Vec::new();
        for name in changed {
            let to = target.refs.get(name).and_then(|id| ObjectId::parse(id));
            let after = source.refs.get(name).and_then(|id| ObjectId::parse(id));
            let current =
                now.as_ref().and_then(|now| now.refs.get(name)).and_then(|id| ObjectId::parse(id));
            let mut ref_plan = RefPlan {
                name: name.clone(),
                objects_missing: to.as_ref().is_some_and(|id| missing.contains(id)),
                to: to.clone(),
                expect: after.clone(),
                symbolic: None,
                expect_symbolic: None,
                reason: blocked,
                pins,
            };
            if blocked.is_none() {
                ref_plan.reason = if current == to {
                    Some(UndoReason::AlreadyDone)
                } else if !has_post && !force {
                    Some(UndoReason::NoRecordAfterTurn)
                } else if current != after && !force {
                    Some(UndoReason::ChangedAfterTurn)
                } else {
                    None
                };
                if force {
                    ref_plan.expect = current;
                }
            }
            refs.push(ref_plan);
        }
        if head_moves {
            refs.push(head_plan(target, source, now.as_ref(), blocked, has_post, force, pins));
        }
        refs
    }

    /// Sets the project's refs and `HEAD` of `root` now in `meta`, when its git dirs
    /// still are the ones that the turn pinned.
    pub(crate) async fn set_project_refs(&self, meta: &mut Meta, root: &RootPlan) {
        let Some((dirs, pins)) = &root.git else { return };
        let Ok(project) = self.open_project(dirs, *pins).await else { return };
        if let Ok(refs) = project.refs().await {
            meta.head = refs.head_ref.clone().or_else(|| refs.head_commit.clone());
            meta.refs = refs_with_head(&refs);
        }
    }

    /// Fetches the refs that `plan` moves away from in `root` (their values now) into
    /// `refs/efr-undo/<conversation>/<undo>/mirror/`, so a redo can give them back
    /// after the project lost their objects. `None` when the store holds them, else
    /// why not.
    pub(crate) async fn mirror_moved_refs(
        &self,
        plan: &UndoPlan,
        root: &RootPlan,
        store: &Store,
    ) -> Option<&'static str> {
        let Some((dirs, pins)) = &root.git else { return Some(MIRROR_FETCH_FAILED) };
        let prefix = undo_ref(plan.conversation, &plan.undo_id, "mirror/");
        let specs: Vec<String> = root
            .refs
            .iter()
            .filter(|ref_plan| ref_plan.reason.is_none() && ref_plan.expect.is_some())
            .map(|ref_plan| {
                let below = ref_plan.name.strip_prefix("refs/").unwrap_or(&ref_plan.name);
                format!("+{}:{prefix}{below}", ref_plan.name)
            })
            .collect();
        if specs.is_empty() {
            return None;
        }
        let Ok(project) = self.open_project(dirs, *pins).await else {
            return Some(MIRROR_FETCH_FAILED);
        };
        let mut args: Vec<OsString> = [
            "-c",
            "fetch.unpackLimit=1",
            "-c",
            "fetch.writeCommitGraph=false",
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            "--no-auto-gc",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let mut upload = OsString::from("--upload-pack=");
        upload.push(ProjectGit::upload_pack());
        args.push(upload);
        args.push(project.dirs().git_dir.as_os_str().to_owned());
        args.extend(specs.into_iter().map(OsString::from));
        let slot = self.slot(store.root());
        let _turn = slot.enter().await;
        match self.runner().run(store, &args, Run { stderr: true, ..Run::default() }).await {
            Ok(output) if output.success => None,
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let error =
                    stderr.lines().find(|line| line.starts_with("fatal:")).unwrap_or_default();
                tracing::warn!(root = %store.root().display(), error, "the refs that an undo moves could not be kept");
                Some(MIRROR_FETCH_FAILED)
            }
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), root = %store.root().display(), "the refs that an undo moves could not be kept");
                Some(MIRROR_FETCH_FAILED)
            }
        }
    }

    /// Moves the project's refs of `root` as `refs` say, each by compare and swap: a
    /// ref that is no longer what the plan expects stays
    /// ([`UndoReason::ChangedAfterTurn`]). The objects that the project lacks come from
    /// the store first (`pack-objects` into the project's `index-pack`). `HEAD` comes
    /// last, and names a branch again only while it still names the branch or commit
    /// of the plan. `root` must name the git dirs that the turn's snapshot pinned.
    pub async fn restore_refs(
        &self,
        root: &Root,
        refs: &[RefPlan],
    ) -> Vec<(String, Result<(), UndoReason>)> {
        let all = |reason: UndoReason| -> Vec<(String, Result<(), UndoReason>)> {
            refs.iter()
                .map(|ref_plan| (ref_plan.name.clone(), Err(ref_plan.reason.unwrap_or(reason))))
                .collect()
        };
        let Some(dirs) = root.git() else { return all(UndoReason::NoMirror) };
        let Some(pins) = refs.iter().find_map(|ref_plan| ref_plan.pins) else {
            return all(UndoReason::NoMirror);
        };
        let project = match self.open_project(dirs, pins).await {
            Ok(project) => project,
            Err(Some(Refusal::Changed)) => return all(UndoReason::RootReplaced),
            Err(_) => return all(UndoReason::NoMirror),
        };
        let Some(path) = self.canonical(vec![root.clone()]).await.into_iter().next() else {
            return all(UndoReason::RootReplaced);
        };
        let store = Store::new(self.dir(), path.path());
        let lost = self.bring_objects(&store, &project, refs).await;
        let mut results = Vec::new();
        let (heads, others): (Vec<&RefPlan>, Vec<&RefPlan>) =
            refs.iter().partition(|ref_plan| ref_plan.name == "HEAD");
        for ref_plan in others.into_iter().chain(heads) {
            let result = match ref_plan.reason {
                Some(reason) => Err(reason),
                None if ref_plan.to.as_ref().is_some_and(|id| lost.contains(id)) => {
                    Err(UndoReason::ObjectsMissing)
                }
                None if ref_plan.name == "HEAD" => self.move_head(&project, ref_plan).await,
                None => {
                    let to = ref_plan.to.as_ref().map(ObjectId::as_str);
                    let from = ref_plan.expect.as_ref().map(ObjectId::as_str);
                    match project.update_ref(&ref_plan.name, to, from).await {
                        Ok(true) => Ok(()),
                        Ok(false) => Err(UndoReason::ChangedAfterTurn),
                        Err(error) => {
                            tracing::warn!(error = %efr_stdx::with_causes(&error), name = %ref_plan.name, "a ref of an undo could not be moved");
                            Err(UndoReason::ChangedAfterTurn)
                        }
                    }
                }
            };
            results.push((ref_plan.name.clone(), result));
        }
        results
    }

    /// Brings the objects of the refs that `refs` moves into the project when it lacks
    /// them; returns the ids that neither the project nor the store holds.
    async fn bring_objects(
        &self,
        store: &Store,
        project: &ProjectGit,
        refs: &[RefPlan],
    ) -> BTreeSet<ObjectId> {
        let mut absent = Vec::new();
        for id in refs
            .iter()
            .filter(|ref_plan| ref_plan.reason.is_none())
            .filter_map(|ref_plan| ref_plan.to.clone())
        {
            if !matches!(project.has_object(id.as_str()).await, Ok(true)) && !absent.contains(&id) {
                absent.push(id);
            }
        }
        if absent.is_empty() {
            return BTreeSet::new();
        }
        let lost = self
            .missing_objects(store, &absent)
            .await
            .unwrap_or_else(|_| absent.iter().cloned().collect());
        let wanted: String =
            absent.iter().filter(|id| !lost.contains(id)).map(|id| format!("{id}\n")).collect();
        if wanted.is_empty() {
            return lost;
        }
        let pack = self
            .runner()
            .checked(
                store,
                "pack-objects",
                &["-c", "pack.threads=1", "pack-objects", "--revs", "--stdout", "--quiet"],
                Run { stdin: Some(wanted.into_bytes()), ..Run::default() },
            )
            .await;
        let taken = match pack {
            Ok(pack) => project.index_pack(pack).await,
            Err(error) => Err(error),
        };
        match taken {
            Ok(()) => lost,
            Err(error) => {
                tracing::warn!(error = %efr_stdx::with_causes(&error), "the objects of an undo's refs could not be brought back");
                absent.into_iter().collect()
            }
        }
    }

    /// Moves `HEAD` as `plan` says, when it still names what the plan expects.
    async fn move_head(&self, project: &ProjectGit, plan: &RefPlan) -> Result<(), UndoReason> {
        let now = project.refs().await.map_err(|_| UndoReason::ChangedAfterTurn)?;
        let expected = match (&plan.expect_symbolic, &plan.expect) {
            (Some(branch), _) => Head::Branch(branch.clone()),
            (None, Some(commit)) => Head::Detached(commit.to_string()),
            (None, None) => Head::Unknown,
        };
        if Head::of_refs(&now) != expected {
            return Err(UndoReason::ChangedAfterTurn);
        }
        let moved = match (&plan.symbolic, &plan.to) {
            (Some(branch), _) => project.set_head(branch).await,
            (None, Some(commit)) => project.detach_head(commit.as_str()).await,
            (None, None) => return Err(UndoReason::Unknown),
        };
        match moved {
            Ok(true) => Ok(()),
            Ok(false) | Err(_) => Err(UndoReason::ChangedAfterTurn),
        }
    }

    /// git on the project's dirs `dirs` when they still have the `(dev, ino)` of
    /// `pins`; the refusal when not (`None`: the look failed).
    async fn open_project(
        &self,
        dirs: &GitDirs,
        pins: Pins,
    ) -> Result<ProjectGit, Option<Refusal>> {
        let runner = self.runner().clone();
        let looked = dirs.clone();
        let opened = self
            .runner()
            .blocking(&dirs.git_dir, move || ProjectGit::open(&runner, &looked, Some(pins)))
            .await;
        match opened {
            Ok(Ok(project)) => Ok(project),
            Ok(Err(refusal)) => Err(Some(refusal)),
            Err(_) => Err(None),
        }
    }
}

/// The plan of `HEAD`, which names another branch or commit in `target` than in
/// `source`.
fn head_plan(
    target: &Meta,
    source: &Meta,
    now: Option<&ProjectRefs>,
    blocked: Option<UndoReason>,
    has_post: bool,
    force: bool,
    pins: Option<Pins>,
) -> RefPlan {
    let goal = Head::of_meta(target);
    let after = Head::of_meta(source);
    let current = now.map_or(Head::Unknown, Head::of_refs);
    let (symbolic, to) = match &goal {
        Head::Branch(branch) => (Some(branch.clone()), None),
        Head::Detached(commit) => (None, ObjectId::parse(commit)),
        Head::Unknown => (None, None),
    };
    let expected = if force { current.clone() } else { after.clone() };
    let (expect_symbolic, expect) = match &expected {
        Head::Branch(branch) => (Some(branch.clone()), None),
        Head::Detached(commit) => (None, ObjectId::parse(commit)),
        Head::Unknown => (None, None),
    };
    let reason = blocked.or(if current == goal {
        Some(UndoReason::AlreadyDone)
    } else if !has_post && !force {
        Some(UndoReason::NoRecordAfterTurn)
    } else if current != after && !force {
        Some(UndoReason::ChangedAfterTurn)
    } else {
        None
    });
    RefPlan {
        name: "HEAD".to_owned(),
        to,
        expect,
        symbolic,
        expect_symbolic,
        objects_missing: false,
        reason,
        pins,
    }
}
