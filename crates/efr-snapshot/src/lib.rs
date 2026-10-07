//! efr's own snapshot store: what the agent changed in files, in every directory that a
//! call can write, git repository or not (efr's auto spec, section 10; phase 4 without
//! undo).
//!
//! - [`Snapshots`] ([`SnapshotParts`]): one bare repository per root in efr's data root,
//!   `$D/snapshots/<root-id>.git`, with a persistent index next to it, which keeps
//!   git's stat cache so a later snapshot hashes only changed files. `$D` is masked in
//!   the sandbox, so sandboxed code cannot reach a snapshot.
//! - [`Snapshots::before_call`] and [`Snapshots::after_call`] ([`CallSnapshot`]): a tree
//!   of each [`Root`] before and after a call that can write, and the
//!   [`FileChanges`](efr_protocol::FileChanges) of
//!   the call; [`Snapshots::before_write`] for a file tool, whose own diff lists its
//!   change; [`Snapshots::finish_turn`]: the turn's last tree, the refs
//!   `refs/efr/<conversation>/<turn>/pre` and `/post`, and the turn's changes.
//! - [`Snapshots::turn_diff`] ([`TurnDiff`]): a finished turn's changes and unified
//!   diff, for `conversation.diff`.
//! - [`Snapshots::gc`] ([`GcReport`]): the newest turns of each conversation stay, and
//!   a store unused for too long goes.
//! - [`Limits`], [`SKIPPED_DIRS`], [`MAX_IGNORED_BYTES`]: what a snapshot takes;
//!   [`shown_prefix`]: how a root's paths show.
//!
//! git runs through `efr_scope::Git::command`, hardened (`runner.rs`): the store's own
//! `GIT_DIR` and `GIT_INDEX_FILE`, no system or global config, no fsmonitor, no hooks,
//! no untracked cache. The project's `.git`, its config and its index are never read
//! or written, except that a project's `info/exclude` is copied to the store.
//!
//! Allowed dependencies: `efr-scope` (the hardened git command), `efr-protocol` (the
//! wire types of the changes) and `efr-stdx` (the clock, atomic writes). What does not
//! belong here: deciding which roots a call can write (the daemon does), permissions,
//! and undo, which comes later.

mod capture;
mod changes;
mod error;
mod gc;
mod history;
mod runner;
mod snapshots;
mod store;

pub use capture::{Limits, MAX_IGNORED_BYTES, SKIPPED_DIRS};
pub use changes::shown_prefix;
pub use error::SnapshotError;
pub use gc::GcReport;
pub use history::TurnDiff;
pub use snapshots::{
    CallSnapshot, DEFAULT_SNAPSHOT_TIMEOUT, Root, SNAPSHOTS_DIR, SnapshotParts, Snapshots,
};
