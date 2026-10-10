//! The wire forms of the exit kinds and sources, and the defaults of the approvers.

use pretty_assertions::assert_eq;
use serde_json::json;

use super::{ExitKind, ExitSource};

#[test]
fn each_exit_source_has_its_wire_form() {
    for (source, wire) in [
        (ExitSource::Predicted, "predicted"),
        (ExitSource::Needs, "needs"),
        (ExitSource::CacheDelete, "cache_delete"),
    ] {
        assert_eq!(serde_json::to_value(source).unwrap(), json!(wire));
        let back: ExitSource = serde_json::from_value(json!(wire)).unwrap();
        assert_eq!(back, source);
    }
}

#[test]
fn the_default_approver_of_a_kind_stays_as_strict_as_in_phase_1() {
    // The engine starts every exit with this default and relaxes it only at the places
    // that spec 9.1 names, so no new exit is looser by accident.
    let user_only: Vec<ExitKind> =
        ExitKind::ALL.into_iter().filter(|kind| kind.user_only()).collect();
    assert_eq!(
        user_only,
        [
            ExitKind::DesktopIpc,
            ExitKind::MaskedRead,
            ExitKind::Privilege,
            ExitKind::Persistence,
            ExitKind::Upload,
            ExitKind::SyncedWrite,
            ExitKind::AboveRoot,
        ]
    );
    assert!(!ExitKind::Write.user_only() && !ExitKind::Outside.user_only());
}
