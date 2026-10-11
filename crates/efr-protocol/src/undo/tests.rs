use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{TurnId, TurnUndoInfo, UndoId, UndoRefused};

#[test]
fn an_undo_refusal_is_tagged_by_kind() {
    let turn_id: TurnId = "0190a8e2-7b1c-7d3e-8f00-1a2b3c4d5e6f".parse().unwrap();
    let refused = UndoRefused::LaterTurnsNotUndone { turn_id };
    assert_eq!(
        serde_json::to_value(refused).unwrap(),
        json!({ "kind": "later_turns_not_undone", "turn_id": turn_id })
    );
    assert_eq!(serde_json::to_value(UndoRefused::Off).unwrap(), json!({ "kind": "off" }));
}

#[test]
fn an_empty_turn_undo_info_is_an_empty_object() {
    assert_eq!(serde_json::to_value(TurnUndoInfo::default()).unwrap(), json!({}));
    let read: TurnUndoInfo = serde_json::from_value(json!({})).unwrap();
    assert_eq!(read, TurnUndoInfo::default());
}

#[test]
fn an_undo_id_is_shown_with_its_prefix_and_sent_without_it() {
    let undo_id: UndoId = "0190a8e2-7b1c-7d3e-8f00-1a2b3c4d5e6f".parse().unwrap();
    assert_eq!(undo_id.shown(), "u-0190a8e2-7b1c-7d3e-8f00-1a2b3c4d5e6f");
    assert_eq!(
        serde_json::to_value(undo_id).unwrap(),
        json!("0190a8e2-7b1c-7d3e-8f00-1a2b3c4d5e6f")
    );
}
