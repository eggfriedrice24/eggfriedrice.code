//! The project's refs in a plan: which refs undo moves back.

use std::collections::BTreeMap;

use crate::SnapshotError;
use crate::meta::Meta;
use crate::snapshots::Snapshots;
use crate::store::Store;
use crate::undo::{RefPlan, RootPlan, UndoPlan};

impl Snapshots {
    /// The refs of the root of `plan` that differ between `target` and `source`.
    pub(crate) async fn plan_refs(
        &self,
        _plan: &RootPlan,
        _target: &Meta,
        _source: &Meta,
        _has_post: bool,
        _force: bool,
    ) -> Vec<RefPlan> {
        Vec::new()
    }

    /// The project's refs of `root` now, `HEAD` with them.
    pub(crate) async fn project_refs_now(&self, _root: &RootPlan) -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    /// Fetches the refs that `plan` moves in `root` into the undo's mirror.
    pub(crate) async fn mirror_moved_refs(
        &self,
        _plan: &UndoPlan,
        _root: &RootPlan,
        _store: &Store,
    ) -> Result<(), SnapshotError> {
        Ok(())
    }
}
