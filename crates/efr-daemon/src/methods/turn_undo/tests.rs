use efr_conversation::{UndoBusyReason, UndoCandidate};
use efr_protocol::{CommandId, ErrorCode, Method, Origin, TurnId, TurnUndo, UndoRefused};
use efr_store::conversations::TurnStatus;
use jiff::Timestamp;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{busy, candidate, phone_refusal, refusal};
use crate::DaemonError;
use crate::methods::authorize;
use crate::sandbox::peers::PeerSide;

fn turn(n: u128) -> TurnId {
    TurnId::from_uuid(uuid::Uuid::from_u128(n))
}

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + seconds).unwrap()
}

/// A refused undo is `conflict` with the refusal in its data, so a client can tell it
/// from a turn that it cannot find.
#[test]
fn a_refused_undo_is_a_conflict_with_the_refusal_in_its_data() {
    let body = refusal(UndoRefused::Off);
    assert_eq!(body.code, ErrorCode::Conflict);
    assert_eq!(body.message, "undo is off");
    assert_eq!(body.data, Some(json!({ "refused": { "kind": "off" } })));
    let later = refusal(UndoRefused::LaterTurnsNotUndone { turn_id: turn(7) });
    assert_eq!(
        later.data,
        Some(json!({ "refused": { "kind": "later_turns_not_undone", "turn_id": turn(7) } }))
    );
}

#[test]
fn a_busy_conversation_is_busy_and_names_what_runs() {
    let body = busy(UndoBusyReason::CallRuns);
    assert_eq!(body.code, ErrorCode::Busy);
    assert_eq!(body.data, Some(json!({ "busy": "call_runs" })));
    assert!(body.message.contains("still runs after its timeout"), "{}", body.message);
    assert_eq!(busy(UndoBusyReason::Turn).data, Some(json!({ "busy": "turn" })));
}

/// A job that may write in the turn's roots names itself and how to stop it: by its id
/// in the same conversation, by the list of every job in another one.
#[test]
fn a_busy_job_says_how_to_stop_it() {
    let own = busy(UndoBusyReason::Job { number: 3, same_conversation: true });
    assert_eq!(own.code, ErrorCode::Busy);
    assert_eq!(own.message, "job j3 runs; stop it first (efr jobs stop j3)");
    assert_eq!(own.data, Some(json!({ "busy": "job", "job_id": "j3", "same_conversation": true })));
    let other = busy(UndoBusyReason::Job { number: 1, same_conversation: false });
    assert_eq!(
        other.message,
        "a job of another conversation writes here; stop it first (efr jobs list --all)"
    );
    assert_eq!(other.data.unwrap()["busy"], "job");
}

/// A phone holds `operate`, so the scope check lets it through and the handler refuses
/// it itself. A model-side peer holds `read` only, so the scope check refuses it.
#[test]
fn turn_undo_refused_for_phone_and_model_side_peer() {
    let method = Method::TurnUndo(TurnUndo {
        command_id: CommandId::from_uuid(uuid::Uuid::from_u128(1)),
        conversation_id: None,
        turn_id: None,
        dry_run: true,
        force: false,
        redo: false,
        list: false,
    });
    assert!(authorize(Origin::Phone, PeerSide::User, &method).is_ok());
    let body = phone_refusal();
    assert_eq!(body.code, ErrorCode::Forbidden);
    assert_eq!(body.data, Some(json!({ "refused": { "kind": "phone_origin" } })));
    let refused = authorize(Origin::Shell, PeerSide::ModelSide, &method);
    assert!(
        matches!(refused, Err(DaemonError::ModelSidePeer { method: "turn.undo" })),
        "{refused:?}"
    );
}

/// The candidates are the turns that ran and ended. A turn that a restart stopped while
/// it ran is one, marked stopped and with no end; a prompt that never ran, or that the
/// user took back, is none.
#[test]
fn turn_undo_lists_a_stopped_turn() {
    let ended = candidate(
        turn(1),
        "fix the test\nand more",
        TurnStatus::Completed,
        Some(at(0)),
        Some(at(5)),
    );
    assert_eq!(
        ended,
        Some(UndoCandidate {
            turn_id: turn(1),
            title: Some("fix the test".to_owned()),
            ended: Some(at(5)),
            stopped: false,
            wake: false,
        })
    );
    let stopped =
        candidate(turn(2), "\n  build it", TurnStatus::Cancelled, Some(at(6)), Some(at(9)));
    assert_eq!(
        stopped,
        Some(UndoCandidate {
            turn_id: turn(2),
            title: Some("build it".to_owned()),
            ended: None,
            stopped: true,
            wake: false,
        })
    );
    for (status, started) in [
        (TurnStatus::Cancelled, None),
        (TurnStatus::Withdrawn, None),
        (TurnStatus::Queued, None),
        (TurnStatus::Running, Some(at(1))),
    ] {
        assert_eq!(candidate(turn(3), "x", status, started, None), None, "{status:?}");
    }
    let failed = candidate(turn(4), "", TurnStatus::Failed, Some(at(1)), Some(at(2))).unwrap();
    assert_eq!(failed.title, None);
}
