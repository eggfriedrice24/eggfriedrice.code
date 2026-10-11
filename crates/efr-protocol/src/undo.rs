//! Undo of a turn (efr's auto spec, sections 10.5 to 10.8, phase B): what a turn's
//! snapshots cover, what `turn.undo` does with each path and ref, and why it refuses.
//!
//! `turn.undo` (`methods/turn_undo.rs`) takes back the files and refs that one turn's
//! calls changed: in the turn's project, in `$SCRATCH`, in the other projects that the
//! turn wrote, and the files outside the roots that efr copied before the write.

use std::fmt;

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::TurnId;
use crate::compaction::is_zero_u32;

/// The most entries of a [`TurnUndoResult`](crate::TurnUndoResult); the rest are
/// counted in its `entries_more`.
pub const MAX_UNDO_ENTRIES: usize = 500;

/// The most names in [`TurnUndoInfo::not_covered`]; the rest are counted in its
/// `not_covered_more`.
pub const MAX_NOT_COVERED: usize = 20;

/// Which snapshot of a turn a [`Event::SnapshotTaken`](crate::Event::SnapshotTaken)
/// reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SnapshotPhase {
    /// Before the turn's first call that can write the root, or the copy of a file
    /// outside the roots before the call that writes it.
    Pre,
    /// At the end of the turn, or the copy of that file after the call.
    Post,
}

/// How many files of one root a turn changed, for the line at the end of the turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RootFiles {
    /// The root as a client shows it: the project's name (`eggfriedrice.code`),
    /// `$SCRATCH`, or the name of another project that the turn wrote.
    pub shown: String,
    /// The files of the root that undo can bring back.
    pub files: u32,
}

/// What `efr undo` can take back of a turn, at the turn's end: the roots with the
/// files that changed, the files outside the roots, and what no snapshot covers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnUndoInfo {
    /// True when `turn.undo` can take back the turn now: a snapshot from before the
    /// turn is kept, `undo.enabled` is true and no call of the turn still runs. False
    /// when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub undoable: bool,
    /// The roots whose files the turn changed. Absent when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<RootFiles>,
    /// The files outside the roots that the turn changed and that efr copied before
    /// the write. Absent when none.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub home_files: u32,
    /// What undo cannot bring back, each with the reason in a few words, at most
    /// [`MAX_NOT_COVERED`]: `target/ (ignored, large)`, `refs of app (shallow)`,
    /// `app (a snapshot failed)`. Absent when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_covered: Vec<String>,
    /// How many names `not_covered` leaves out. Absent when none.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub not_covered_more: u32,
}

/// What `turn.undo` does with one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UndoAction {
    /// Writes the bytes from before the turn back. The current file goes to the undo
    /// trash first.
    Restore,
    /// Moves a file that the turn made to the undo trash.
    Trash,
    /// Removes a link or an empty entry that the turn made, after a copy in the undo
    /// trash.
    Delete,
    /// Leaves the path as it is; the entry's `reason` says why.
    Skip,
    /// Leaves the path as it is, because no snapshot holds it; the entry's `reason`
    /// says why.
    NotCovered,
    /// Runs a command that undoes a state change, such as `crontab` for the crontab.
    Command,
}

impl UndoAction {
    /// Every action, in the order of the enum.
    pub const ALL: [UndoAction; 6] = [
        UndoAction::Restore,
        UndoAction::Trash,
        UndoAction::Delete,
        UndoAction::Skip,
        UndoAction::NotCovered,
        UndoAction::Command,
    ];
}

/// Why `turn.undo` skips a path or a ref, or why no snapshot covers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UndoReason {
    /// The path changed after the turn; `force` overwrites it.
    ChangedAfterTurn,
    /// The turn has no snapshot from its end (efrd stopped in the turn), so undo cannot
    /// tell whether the path changed after it; `force` acts on the turn's last
    /// snapshot.
    NoRecordAfterTurn,
    /// The snapshot left the path out, such as a large or an ignored file.
    LeftOut,
    /// The sandbox hides the path, such as a credential store, so efr never copies it.
    Masked,
    /// A directory over `snapshot.home_max_files` files or `snapshot.home_max_mib` MiB,
    /// or a file over the size limit.
    OverLimit,
    /// The path is new in the turn.
    NewInTurn,
    /// A later turn of the conversation is not undone; `force` overwrites what it
    /// changed.
    LaterTurnNotUndone,
    /// The project's refs have no mirror in the store, so undo cannot move them.
    NoMirror,
    /// The objects of a ref are no longer in the store.
    ObjectsMissing,
    /// The path changed between two calls of the turn, not in a call.
    OutsideTurnCalls,
    /// The path already has its state from before the turn.
    AlreadyDone,
    /// The root is another directory than the one that the snapshot took; `force`
    /// writes it anyway.
    RootReplaced,
    /// No fact says what the path was, so undo never writes or deletes it.
    Unknown,
    /// The call that changed the path did not finish, so no copy from after it exists.
    CallUnfinished,
    /// A snapshot of the root failed in the turn.
    SnapshotFailed,
    /// The snapshot store changed under an approved command that ran outside the
    /// sandbox.
    StoreChanged,
    /// The store dropped the mirror of the turn to keep within its size limit.
    StoreFull,
    /// Content that was only staged, in neither the work tree nor `HEAD`.
    StagedOnly,
}

impl UndoReason {
    /// Every reason, in the order of the enum.
    pub const ALL: [UndoReason; 18] = [
        UndoReason::ChangedAfterTurn,
        UndoReason::NoRecordAfterTurn,
        UndoReason::LeftOut,
        UndoReason::Masked,
        UndoReason::OverLimit,
        UndoReason::NewInTurn,
        UndoReason::LaterTurnNotUndone,
        UndoReason::NoMirror,
        UndoReason::ObjectsMissing,
        UndoReason::OutsideTurnCalls,
        UndoReason::AlreadyDone,
        UndoReason::RootReplaced,
        UndoReason::Unknown,
        UndoReason::CallUnfinished,
        UndoReason::SnapshotFailed,
        UndoReason::StoreChanged,
        UndoReason::StoreFull,
        UndoReason::StagedOnly,
    ];
}

/// One path of an undo: what undo does or did with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UndoEntry {
    /// What undo does with the path.
    pub action: UndoAction,
    /// The path as a client shows it: `src/a.rs` (relative to its root), `$SCRATCH/x`,
    /// `~/.zshrc`, or a state such as `@crontab`.
    pub path: String,
    /// Why, for a skip or a path that no snapshot covers. Absent for the other actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<UndoReason>,
}

/// One ref of a project that an undo moves, such as a branch that the turn moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RefChange {
    /// The root of the ref, as a client shows it.
    pub root: String,
    /// The ref, such as `main`, `HEAD` or `refs/stash`.
    pub name: String,
    /// The short id that the ref has now. Absent: the ref does not exist now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The short id that undo gives it. Absent: undo removes the ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// True when undo moved the ref, false when it did not. Absent in a dry run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<bool>,
    /// Why undo did not move it. Absent when it did, or will.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<UndoReason>,
}

/// One turn that `turn.undo` with `list` shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UndoableTurn {
    /// The turn.
    pub turn_id: TurnId,
    /// The first line of the turn's prompt. Absent when the log has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// True when the turn ran in `auto`. False when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
    /// True when an undo took the turn back and no redo returned it. False when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub undone: bool,
    /// True when efrd stopped while the turn ran, so its undo needs `force`. False
    /// when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    /// When the turn ended. Absent for a turn that efrd stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Timestamp>,
}

/// Why `turn.undo` changes nothing. The daemon answers it as a `conflict` error.
///
/// On the wire: `{"kind": "off"}`, `{"kind": "later_turns_not_undone", "turn_id": ...}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum UndoRefused {
    /// Undo is off: `undo.enabled` is false, or this efrd keeps no snapshot store.
    Off,
    /// No turn of the conversation has a snapshot from before it that is not undone.
    NothingToUndo,
    /// The conversation has no turn with this id, or the turn has no snapshot.
    TurnNotFound,
    /// A newer turn of the conversation is not undone; `force` undoes this one anyway.
    LaterTurnsNotUndone {
        /// The newest such turn.
        turn_id: TurnId,
    },
    /// `redo` found no undo to take back.
    NoRedo,
    /// A phone cannot undo: undo writes files on this machine.
    PhoneOrigin,
    /// efr could not read its snapshot store, such as after a git failure or a
    /// timeout; the log of efrd says why.
    StoreFailed,
}

impl fmt::Display for UndoRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UndoRefused::Off => f.write_str("undo is off"),
            UndoRefused::NothingToUndo => f.write_str("no turn of the conversation can be undone"),
            UndoRefused::TurnNotFound => f.write_str("the turn has no snapshot to undo"),
            UndoRefused::LaterTurnsNotUndone { turn_id } => {
                write!(f, "the later turn {turn_id} is not undone")
            }
            UndoRefused::NoRedo => f.write_str("no undo can be taken back"),
            UndoRefused::PhoneOrigin => f.write_str("a phone cannot undo a turn"),
            UndoRefused::StoreFailed => {
                f.write_str("efr could not read its snapshot store; the log of efrd says why")
            }
        }
    }
}

#[cfg(test)]
mod tests;
