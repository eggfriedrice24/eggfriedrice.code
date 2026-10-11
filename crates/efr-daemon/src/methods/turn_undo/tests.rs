use efr_protocol::{ErrorCode, UndoRefused};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::refusal;

/// Until undo exists, `turn.undo` answers `conflict` with the refusal `off`, so a
/// client can tell it from a turn that it cannot find.
#[test]
fn a_refused_undo_is_a_conflict_with_the_refusal_in_its_data() {
    let body = refusal(UndoRefused::Off);
    assert_eq!(body.code, ErrorCode::Conflict);
    assert_eq!(body.message, "undo is off");
    assert_eq!(body.data, Some(json!({ "refused": { "kind": "off" } })));
}
