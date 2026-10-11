//! The home paths of an undo: what undo writes outside the roots.

use std::path::PathBuf;

use efr_protocol::{ConversationId, TurnId, UndoReason};

use crate::SnapshotError;
use crate::object::ObjectId;
use crate::snapshots::Snapshots;
use crate::store::Store;
use crate::undo::{Expect, RootPlan, Scan, UndoPlan};

/// The id of the home store in the store directory: `home.git`.
pub(crate) const HOME_ID: &str = "home";

/// The state of a home path in a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HomeState {
    /// A regular file with this mode (the whole `st_mode`, `& 0o7777`).
    File {
        /// The mode.
        mode: u32,
    },
    /// A symbolic link to `target`, stored as a link, never followed.
    Link {
        /// The link's target.
        target: Vec<u8>,
    },
    /// The path does not exist.
    Missing,
    /// The text of a state that a command reads and sets (`@crontab`, `@linger`).
    State,
    /// No copy holds the path, for this reason.
    NotCovered(UndoReason),
}

/// What undo does with one home path: it writes `write` when the path is `expect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeStep {
    /// The absolute path, or `@crontab` or `@linger`.
    pub path: PathBuf,
    /// The state that undo gives the path; [`HomeState::NotCovered`] leaves it as it is.
    pub write: HomeState,
    /// The blob of a file or a state that undo writes.
    pub blob: Option<ObjectId>,
    /// What the path must be just before undo writes it.
    pub expect: Expect,
    /// The blob and mode of what is there now, which go to the undo trash first.
    pub trash: Option<(ObjectId, u32)>,
    /// True for a floor or persistence file.
    pub floor: bool,
}

impl HomeStep {
    /// True when undo writes or removes the path.
    pub fn acts(&self) -> bool {
        !matches!(self.write, HomeState::NotCovered(_)) && self.expect != Expect::Unknown
    }
}

/// The home paths of a plan, one step per path: the state before the turn's first copy
/// of the path, and the state after its last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HomePlan {
    /// The steps, sorted by path.
    pub steps: Vec<HomeStep>,
}

impl Snapshots {
    /// The home store, `$D/snapshots/home.git`.
    pub(crate) fn home_store(&self) -> Store {
        Store::with_id(self.dir(), HOME_ID, self.dir().join(format!("{HOME_ID}.git")))
    }

    pub(crate) async fn plan_home_undo(
        &self,
        _conversation: ConversationId,
        _turn: TurnId,
        _scan: &Scan,
        _roots: &[RootPlan],
        _force: bool,
    ) -> Result<HomePlan, SnapshotError> {
        Ok(HomePlan::default())
    }

    pub(crate) async fn plan_home_redo(
        &self,
        _conversation: ConversationId,
        _undo: &str,
        _turn: TurnId,
        _scan: &Scan,
        _roots: &[RootPlan],
    ) -> Result<HomePlan, SnapshotError> {
        Ok(HomePlan::default())
    }

    pub(crate) async fn home_started(&self, _plan: &UndoPlan) -> Result<(), SnapshotError> {
        Ok(())
    }
}
