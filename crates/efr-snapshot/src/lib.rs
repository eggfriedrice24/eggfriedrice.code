//! efr's own snapshot store: what the agent changed in files, in every directory that a
//! call can write, git repository or not, and the plan of an undo of a turn (efr's auto
//! spec, section 10).
//!
//! - [`Snapshots`] ([`SnapshotParts`]): one bare repository per root in efr's data root,
//!   `$D/snapshots/<root-id>.git`, with a persistent index next to it, which keeps
//!   git's stat cache so a later snapshot hashes only changed files. `$D` is masked in
//!   the sandbox, so sandboxed code cannot reach a snapshot.
//! - [`Snapshots::before_call`] and [`Snapshots::after_call`] ([`CallSnapshot`]): a tree
//!   of each [`Root`] before and after a call that can write, and the
//!   [`FileChanges`](efr_protocol::FileChanges) of
//!   the call; the first snapshot of a root in a turn writes the ref
//!   `refs/efr/<conversation>/<turn>/pre`, a call that changes files moves `/last` and
//!   adds to the turn's `touched` paths; [`Snapshots::before_write`] and
//!   [`Snapshots::after_write`] for a file tool, whose own diff lists its change;
//!   [`Snapshots::turn_end`] ([`TurnEnd`], [`SnapshotInfo`]): `/post`, the turn's
//!   changes, what its snapshots left out ([`LeftOut`]) and the roots it does not
//!   cover. A root with trusted [`GitDirs`] gets a mirror of its project's refs, and
//!   [`Root::with_skip`] leaves directories at its top out, such as `$SCRATCH/jobs`.
//! - [`Snapshots::after_job`]: the files that a background job changed join the turn
//!   that started it at the job's end, without the paths that other turns changed
//!   meanwhile.
//! - [`Snapshots::turn_diff`] ([`TurnDiff`]): a finished turn's changes and unified
//!   diff, for `conversation.diff`.
//! - [`Snapshots::plan_undo`] and [`Snapshots::plan_redo`] ([`UndoPlan`], [`RootPlan`],
//!   [`PlanEntry`], [`Expect`]): what an undo or a redo of a turn writes, on the paths
//!   that the turn's calls changed; [`Snapshots::turns_with_refs`] ([`TurnRefs`]) and
//!   [`stack_blocker`]: the turns that undo can take, newest first;
//!   [`Snapshots::capture_before_undo`], [`Snapshots::blob`] and
//!   [`Snapshots::finish_undo`] around the daemon's apply.
//! - [`Snapshots::home_before`] and [`Snapshots::home_after`] ([`HomeReader`],
//!   [`HomeEntry`], [`HomeCopy`], [`HomeDelta`]): the home store
//!   `$D/snapshots/home.git`, copies of the paths outside the roots before and after a
//!   call, which the daemon reads; [`HomePlan`] and [`HomeStep`]: their part of an
//!   undo; [`Snapshots::home_undo_before`] and [`Snapshots::home_blob`].
//! - [`Snapshots::gc`], [`Snapshots::gc_capped`] and [`Snapshots::gc_keeping`]
//!   ([`GcReport`]): the newest turns of each conversation stay whole across every
//!   store, and the turns that the daemon names, the mirrors of a store stay within
//!   their limit, and a store unused for too long goes; [`Snapshots::forget_conversation`]
//!   ([`ForgetReport`]): every ref of a deleted conversation, and each root store that
//!   no other conversation uses.
//! - [`Limits`], [`SKIPPED_DIRS`], [`MAX_IGNORED_BYTES`]: what a snapshot takes;
//!   [`shown_prefix`]: how a root's paths show; [`blob_id`]: git's id of a file.
//!
//! git runs through `efr_scope::Git::command`, hardened (`runner.rs`): the store's own
//! `GIT_DIR` and `GIT_INDEX_FILE`, no system or global config, no fsmonitor, no hooks,
//! no untracked cache, no attribute that changes bytes. A snapshot never writes the
//! project's `.git`; it reads the project's `info/exclude`, and with trusted git dirs
//! its refs and objects through a second hardened mode (`project_git.rs`). Only an
//! applied undo writes the project's refs and objects ([`Snapshots::restore_refs`]),
//! by compare and swap, never its config, hooks or index.
//!
//! Allowed dependencies: `efr-scope` (the hardened git command), `efr-protocol` (the
//! wire types of the changes) and `efr-stdx` (the clock, atomic writes). What does not
//! belong here: deciding which roots a call can write (the daemon does), permissions,
//! and the writes of an undo, which the daemon makes without following links.

mod capture;
mod changes;
mod cover;
mod error;
mod gc;
mod history;
mod home;
mod job;
mod meta;
mod mirror;
mod object;
mod project_git;
mod runner;
mod snapshots;
mod store;
mod turn;
mod undo;

pub use capture::{LeftOut, LeftOutReason, Limits, MAX_IGNORED_BYTES, SKIPPED_DIRS};
pub use changes::shown_prefix;
pub use cover::{CoverResult, RootCover};
pub use error::SnapshotError;
pub use gc::{ForgetReport, GcReport};
pub use history::TurnDiff;
pub use home::{
    HomeCopy, HomeDelta, HomeEntries, HomeEntry, HomePlan, HomeReader, HomeState, HomeStep,
    MAX_HOME_FILE_BYTES, git_refuses,
};
pub use object::{ObjectId, blob_id, link_id};
pub use project_git::GitDirs;
pub use snapshots::{
    CallSnapshot, DEFAULT_SNAPSHOT_TIMEOUT, Root, SNAPSHOTS_DIR, SnapshotParts, Snapshots,
};
pub use turn::{
    NOT_COVERED_FAILED, NOT_COVERED_REFS, NOT_COVERED_STORE_CHANGED, NOT_COVERED_UNFINISHED, Phase,
    SnapshotInfo, TurnEnd,
};
pub use undo::{
    BeforeUndo, Expect, PlanAction, PlanEntry, RefPlan, RootPlan, TurnRefs, UndoPlan, stack_blocker,
};
