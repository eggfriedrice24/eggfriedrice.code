use efr_store::conversations::TurnStatus;

use super::shown_by_default;

#[test]
fn the_default_turn_is_one_that_started_and_ended_and_was_not_cancelled() {
    assert!(shown_by_default(TurnStatus::Completed, true));
    assert!(shown_by_default(TurnStatus::Failed, true));
    assert!(shown_by_default(TurnStatus::Interrupted, true));
    assert!(!shown_by_default(TurnStatus::Cancelled, false), "a queued prompt that never ran");
    assert!(!shown_by_default(TurnStatus::Cancelled, true), "a restart cancelled it");
    assert!(!shown_by_default(TurnStatus::Running, true));
    assert!(!shown_by_default(TurnStatus::Queued, false));
}
