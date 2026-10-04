//! Command receipts over a `TestDaemon`: a retried write answers from its receipt and
//! runs nothing; a final refusal stays refused; a command id is bound to its method.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helpers as test code, as it does for unit tests.
#![cfg(test)]

use efr_protocol::ErrorCode;
use efr_test_daemon::Replay;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn duplicate_command_id_receipt() {
    let replay = Replay::run("duplicate_command_id_receipt").await.unwrap();

    let first = replay.result(1).unwrap().clone().unwrap();
    assert_eq!(replay.result(2).unwrap().as_ref().unwrap(), &first, "the same prompt again");
    assert_eq!(
        replay.result(3).unwrap().as_ref().unwrap(),
        &first,
        "a retry is matched by its command id, not by its text"
    );
    let events = replay.events().await.unwrap();
    let queued = events.iter().filter(|e| e.event.kind() == "prompt_queued").count();
    let started = events.iter().filter(|e| e.event.kind() == "turn_started").count();
    assert_eq!((queued, started), (1, 1), "the prompt ran once");

    let steer = replay.result(4).unwrap().clone().unwrap_err();
    assert_eq!(steer.code, ErrorCode::Conflict, "nothing runs to steer");
    let retried = replay.result(5).unwrap().clone().unwrap_err();
    assert_eq!(retried, steer, "a final refusal is kept as a receipt");
    let reused = replay.result(6).unwrap().clone().unwrap_err();
    assert_eq!(reused.code, ErrorCode::Conflict, "a command id belongs to one method");
    replay.stop().await.unwrap();
}
