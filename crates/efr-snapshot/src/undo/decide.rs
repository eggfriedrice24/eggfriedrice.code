//! The rule for one path of a root: what undo (or redo) does with it, from its state in
//! the target tree, in the source tree (the state that undo expects) and on disk now.
//! Pure, so the order of the checks is tested alone.

use efr_protocol::UndoReason;

use crate::capture::LeftOutReason;
use crate::object::{ObjectId, link_id};
use crate::undo::current::Current;
use crate::undo::{Expect, PlanAction, PlanEntry};

/// A path's state in one tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Side {
    /// Not in the tree, and the snapshot did not leave it out.
    Missing,
    /// A regular file: its blob and git's mode (`0o100644` or `0o100755`).
    Blob { id: ObjectId, mode: u32 },
    /// A link whose target is the blob `id`.
    Link { id: ObjectId },
    /// The snapshot left the path out, for this reason.
    LeftOut(LeftOutReason),
}

impl Side {
    /// True when `current` is this state; the executable bit counts, the other mode
    /// bits do not, as in git.
    fn holds(&self, current: &Current) -> bool {
        match (self, current) {
            (Side::Missing, Current::Missing) => true,
            (Side::Blob { id, mode }, Current::File { id: found, mode: now }) => {
                id == found && (*mode == 0o100_755) == (now & 0o111 != 0)
            }
            (Side::Link { id }, Current::Link { target }) => link_id(target) == *id,
            _ => false,
        }
    }
}

/// What else decides the path.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Facts {
    /// A call of the turn changed the path (or, for a redo, the undo wrote it).
    pub(crate) touched: bool,
    /// Why a path that no call changed may still be one of the turn's: the record of
    /// the turn's paths is cut ([`UndoReason::OutsideTurnCalls`]) or a snapshot failed
    /// ([`UndoReason::SnapshotFailed`]). `force` acts on such a path.
    pub(crate) maybe_touched: Option<UndoReason>,
    /// The source is the state after the turn (`post`); without it undo needs `force`.
    pub(crate) has_post: bool,
    /// The root is another directory than the one that the snapshot took.
    pub(crate) replaced: bool,
    /// A later turn that is not undone changed the path.
    pub(crate) later: bool,
    pub(crate) force: bool,
}

/// The entry of `path`; `target_link` is the target of a link in the target tree.
/// The action's mode is filled in later for a file that undo makes again.
pub(crate) fn decide(
    path: &str,
    target: &Side,
    source: &Side,
    current: &Current,
    target_link: Option<&[u8]>,
    facts: Facts,
) -> PlanEntry {
    let skip = |reason: UndoReason| PlanEntry {
        path: path.to_owned(),
        action: PlanAction::Skip(reason),
        expect: expect_of(current),
        trash: None,
        forced: None,
    };
    for side in [target, source] {
        if let Side::LeftOut(reason) = side {
            return PlanEntry {
                path: path.to_owned(),
                action: PlanAction::NotCovered(*reason),
                expect: expect_of(current),
                trash: None,
                forced: None,
            };
        }
    }
    let mut forced = None;
    if !facts.touched {
        match facts.maybe_touched {
            Some(reason) if facts.force => forced = Some(reason),
            Some(reason) => return skip(reason),
            None => return skip(UndoReason::OutsideTurnCalls),
        }
    }
    if *current == Current::Unknown {
        return skip(UndoReason::Unknown);
    }
    if target.holds(current) {
        return skip(UndoReason::AlreadyDone);
    }
    let checks = [
        (facts.replaced, UndoReason::RootReplaced),
        (!facts.has_post, UndoReason::NoRecordAfterTurn),
        (
            facts.has_post && !source.holds(current),
            if facts.later { UndoReason::LaterTurnNotUndone } else { UndoReason::ChangedAfterTurn },
        ),
    ];
    for (hit, reason) in checks {
        if !hit {
            continue;
        }
        if !facts.force {
            return skip(reason);
        }
        forced = forced.or(Some(reason));
    }
    let action = match (target, target_link) {
        (Side::Missing, _) => PlanAction::Delete,
        (Side::Blob { id, mode }, _) => PlanAction::Write { blob: id.clone(), mode: mode & 0o777 },
        (Side::Link { .. }, Some(link)) => PlanAction::Link { target: link.to_vec() },
        (Side::Link { .. }, None) => return skip(UndoReason::Unknown),
        (Side::LeftOut(_), _) => return skip(UndoReason::LeftOut),
    };
    PlanEntry {
        path: path.to_owned(),
        action,
        expect: expect_of(current),
        trash: current.trash(),
        forced,
    }
}

fn expect_of(current: &Current) -> Expect {
    match current {
        Current::Missing => Expect::Missing,
        Current::File { id, .. } => Expect::Blob(id.clone()),
        Current::Link { target } => Expect::Link(target.clone()),
        Current::Unknown => Expect::Unknown,
    }
}
